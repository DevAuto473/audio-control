//! Linux (PipeWire / PulseAudio) backend.
//!
//! Creates a virtual sink "Audio Boost", makes it the default output, then
//! pipes `parec` (virtual sink monitor) -> DSP -> `pacat` (real output).
//! Only needs `pactl`, `parec`, `pacat` (package `pulseaudio-utils`, which
//! Fedora ships with PipeWire).

use crate::{dsp::Processor, Device, Meter, Shared, MAX_GAIN, SAMPLE_RATE};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;

const VSINK: &str = "audio_boost";
const FRAMES: usize = 480; // 10 ms
const CH: usize = 2;

fn pactl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("pactl")
        .args(args)
        .output()
        .map_err(|e| format!("could not run pactl ({e}). Install pulseaudio-utils"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn list_sinks() -> Vec<Device> {
    // `pactl list sinks` is verbose but gives us the human readable description
    let Ok(txt) = pactl(&["list", "sinks"]) else { return vec![] };
    let (mut out, mut name): (Vec<Device>, Option<String>) = (vec![], None);
    for line in txt.lines() {
        let l = line.trim();
        if let Some(n) = l.strip_prefix("Name: ") {
            name = Some(n.to_string());
        } else if let Some(d) = l.strip_prefix("Description: ") {
            if let Some(n) = name.take() {
                if n != VSINK {
                    out.push(Device { id: n, name: d.to_string() });
                }
            }
        }
    }
    out
}

fn sink_volume_percent(sink: &str) -> Option<u32> {
    let s = pactl(&["get-sink-volume", sink]).ok()?;
    let pct = s.split('/').nth(1)?.trim().trim_end_matches('%').trim();
    pct.parse().ok()
}

/// Give audio back to the real device (idempotent).
fn restore_audio(prev_default: &str) {
    let _ = pactl(&["set-default-sink", prev_default]);
    if let Ok(list) = pactl(&["list", "short", "sink-inputs"]) {
        for id in list.lines().filter_map(|l| l.split_whitespace().next()) {
            let _ = pactl(&["move-sink-input", id, prev_default]);
        }
    }
}

pub struct Engine {
    stopping: Arc<std::sync::atomic::AtomicBool>,
    shared: Arc<Shared>,
    children: Vec<Child>,
    thread: Option<JoinHandle<()>>,
    module_id: Option<String>,
    real_sink: String,
    prev_default: String,
    prev_volume: Option<u32>,
}

impl Engine {
    /// Real output devices, the system default first.
    pub fn outputs() -> Vec<Device> {
        let mut list = list_sinks();
        if let Ok(def) = pactl(&["get-default-sink"]) {
            if let Some(i) = list.iter().position(|d| d.id == def) {
                let d = list.remove(i);
                list.insert(0, d);
            }
        }
        list
    }

    pub fn start(output: Option<String>, gain: f32) -> Result<Self, String> {
        let default = pactl(&["get-default-sink"])?;
        let sinks = list_sinks();
        // If we crashed earlier the default is still our virtual sink.
        let real_sink = match output {
            Some(o) if sinks.iter().any(|d| d.id == o) => o,
            _ if default != VSINK && sinks.iter().any(|d| d.id == default) => default.clone(),
            _ => sinks
                .iter()
                .find(|d| !d.id.contains("auto_null"))
                .map(|d| d.id.clone())
                .ok_or("no output device found")?,
        };
        let prev_default = if default == VSINK { real_sink.clone() } else { default };

        let have_vsink = pactl(&["list", "short", "sinks"])?
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(VSINK));
        let module_id = if have_vsink {
            None
        } else {
            Some(pactl(&[
                "load-module",
                "module-null-sink",
                &format!("sink_name={VSINK}"),
                "sink_properties=device.description=AudioBoost",
                &format!("rate={SAMPLE_RATE}"),
                "channels=2",
            ])?)
        };
        let _ = pactl(&["set-sink-volume", VSINK, "100%"]);

        // The real device runs at 100 %; all volume comes from this app.
        let prev_volume = sink_volume_percent(&real_sink);
        let _ = pactl(&["set-sink-volume", &real_sink, "100%"]);
        let _ = pactl(&["set-sink-mute", &real_sink, "0"]);

        let fmt = [
            "--raw".to_string(),
            "--format=float32le".to_string(),
            format!("--rate={SAMPLE_RATE}"),
            "--channels=2".to_string(),
        ];
        let mut rec = Command::new("parec")
            .arg(format!("--device={VSINK}.monitor"))
            .args(&fmt)
            .arg("--latency-msec=20")
            .stdout(Stdio::piped())
            .stderr(if std::env::var_os("AB_DEBUG").is_some() { Stdio::inherit() } else { Stdio::null() })
            .spawn()
            .map_err(|e| format!("could not run parec ({e}). Install pulseaudio-utils"))?;
        let mut play = Command::new("pacat")
            .arg("--playback")
            .arg(format!("--device={real_sink}"))
            .arg("--client-name=Audio Boost")
            .args(&fmt)
            .arg("--latency-msec=40")
            .stdin(Stdio::piped())
            .stderr(if std::env::var_os("AB_DEBUG").is_some() { Stdio::inherit() } else { Stdio::null() })
            .spawn()
            .map_err(|e| format!("could not run pacat ({e}). Install pulseaudio-utils"))?;

        let mut input = rec.stdout.take().unwrap();
        let mut output_pipe = play.stdin.take().unwrap();
        let shared = Arc::new(Shared::new(gain.clamp(0.0, MAX_GAIN)));
        let sh = shared.clone();
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = stopping.clone();
        let fallback = prev_default.clone();
        let thread = std::thread::spawn(move || {
            let mut p = Processor::new(SAMPLE_RATE, CH);
            p.set_gain(sh.effective_gain());
            let mut bytes = vec![0u8; FRAMES * CH * 4];
            let mut samples = vec![0f32; FRAMES * CH];
            while sh.running.load(Ordering::Relaxed) {
                if input.read_exact(&mut bytes).is_err() {
                    break;
                }
                for (s, b) in samples.iter_mut().zip(bytes.chunks_exact(4)) {
                    *s = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                }
                p.set_gain(sh.effective_gain());
                p.process(&mut samples);
                let (g, pk) = p.take_stats();
                sh.publish(g, pk);
                for (s, b) in samples.iter().zip(bytes.chunks_exact_mut(4)) {
                    b.copy_from_slice(&s.to_le_bytes());
                }
                if output_pipe.write_all(&bytes).is_err() {
                    break;
                }
            }
            sh.running.store(false, Ordering::Relaxed);
            // pipeline died on its own: never leave the system silent
            if !stop_flag.load(Ordering::Relaxed) {
                restore_audio(&fallback);
            }
        });

        // Route everything through the virtual sink.
        let _ = pactl(&["set-default-sink", VSINK]);
        if let Ok(list) = pactl(&["list", "short", "sink-inputs"]) {
            for id in list.lines().filter_map(|l| l.split_whitespace().next()) {
                let _ = pactl(&["move-sink-input", id, VSINK]);
            }
        }

        Ok(Self {
            stopping,
            shared,
            children: vec![rec, play],
            thread: Some(thread),
            module_id,
            real_sink,
            prev_default,
            prev_volume,
        })
    }

    pub fn set_gain(&self, g: f32) {
        self.shared.set_gain(g);
    }
    pub fn set_muted(&self, m: bool) {
        self.shared.set_muted(m);
    }
    pub fn meter(&self) -> Meter {
        self.shared.take_meter()
    }
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }
    pub fn output_id(&self) -> &str {
        &self.real_sink
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.shared.running.store(false, Ordering::Relaxed);
        // give audio back to the real device first so nothing goes silent
        restore_audio(&self.prev_default);
        if let Some(v) = self.prev_volume.take() {
            let _ = pactl(&["set-sink-volume", &self.real_sink, &format!("{v}%")]);
        }
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.children.clear();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(id) = self.module_id.take() {
            let _ = pactl(&["unload-module", &id]);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}
