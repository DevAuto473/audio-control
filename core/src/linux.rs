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

struct Sink {
    id: String,
    name: String,
    /// false when the active port reports "not available" (e.g. HDMI with nothing plugged in)
    usable: bool,
    hdmi: bool,
}

fn parse_sinks() -> Vec<Sink> {
    match pactl(&["list", "sinks"]) {
        Ok(t) => parse_sinks_text(&t),
        Err(_) => vec![],
    }
}

fn parse_sinks_text(txt: &str) -> Vec<Sink> {
    let mut out: Vec<Sink> = vec![];
    let (mut name, mut desc, mut active) = (None::<String>, None::<String>, None::<String>);
    let mut ports: Vec<(String, bool)> = vec![]; // (port name, available)

    let mut flush = |name: &mut Option<String>, desc: &mut Option<String>, active: &mut Option<String>, ports: &mut Vec<(String, bool)>| {
        if let (Some(n), Some(d)) = (name.take(), desc.take()) {
            let usable = match active.as_ref() {
                Some(a) => ports.iter().find(|(p, _)| p == a).map(|(_, ok)| *ok).unwrap_or(true),
                None => true,
            };
            let low = format!("{n} {d}").to_lowercase();
            if n != VSINK {
                out.push(Sink { id: n, name: d, usable, hdmi: low.contains("hdmi") || low.contains("displayport") });
            }
        }
        *active = None;
        ports.clear();
    };

    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with("Sink #") {
            flush(&mut name, &mut desc, &mut active, &mut ports);
        } else if let Some(n) = l.strip_prefix("Name: ") {
            name = Some(n.to_string());
        } else if let Some(d) = l.strip_prefix("Description: ") {
            desc = Some(d.to_string());
        } else if let Some(a) = l.strip_prefix("Active Port: ") {
            active = Some(a.to_string());
        } else if l.contains("(type:") || l.contains("priority:") {
            // port line: "<port>: <label> (type: ..., priority: ..., [availability ...])"
            if let Some((pn, rest)) = l.split_once(": ") {
                if rest.contains("priority:") {
                    ports.push((pn.to_string(), !rest.contains("not available")));
                }
            }
        }
    }
    flush(&mut name, &mut desc, &mut active, &mut ports);
    out
}

/// Best real output: the system default if it works, else a working
/// non-HDMI device (laptop speakers), else any working device.
fn pick_real(sinks: &[Sink], default: &str) -> Option<String> {
    sinks.iter().find(|s| s.id == default && s.usable)
        .or_else(|| sinks.iter().find(|s| s.usable && !s.hdmi && !s.id.contains("auto_null")))
        .or_else(|| sinks.iter().find(|s| s.usable && !s.id.contains("auto_null")))
        .or_else(|| sinks.first())
        .map(|s| s.id.clone())
}

fn list_sinks() -> Vec<Device> {
    let sinks = parse_sinks();
    let default = pactl(&["get-default-sink"]).unwrap_or_default();
    let best = pick_real(&sinks, &default);
    let any_usable = sinks.iter().any(|s| s.usable);
    let mut devs: Vec<&Sink> = sinks.iter().filter(|s| s.usable || !any_usable).collect();
    // the device the app would pick goes first
    devs.sort_by_key(|s| Some(&s.id) != best.as_ref());
    devs.into_iter().map(|s| Device { id: s.id.clone(), name: s.name.clone() }).collect()
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
    /// Working output devices; the one the app would pick is first.
    pub fn outputs() -> Vec<Device> {
        list_sinks()
    }

    pub fn start(output: Option<String>, gain: f32) -> Result<Self, String> {
        let default = pactl(&["get-default-sink"])?;
        let sinks = parse_sinks();
        // If we crashed earlier the default is still our virtual sink.
        let real_sink = match output {
            Some(o) if sinks.iter().any(|d| d.id == o) => o,
            _ => pick_real(&sinks, &default).ok_or("no output device found")?,
        };
        // restore target: the old default if it works, otherwise the device we picked
        let prev_default = if default != VSINK && sinks.iter().any(|d| d.id == default && d.usable) {
            default
        } else {
            real_sink.clone()
        };

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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Sink #50
\tState: SUSPENDED
\tName: alsa_output.pci-0000_00_1f.3.hdmi-stereo-extra2
\tDescription: 700 Series Chipset Family HD Audio HDMI / DisplayPort 3 Output (Stereo)
\tPorts:
\t\thdmi-output-2: HDMI / DisplayPort 3 (type: HDMI, priority: 5700, availability group: Legacy 3, not available)
\tActive Port: hdmi-output-2
Sink #51
\tState: RUNNING
\tName: alsa_output.pci-0000_00_1f.3.analog-stereo
\tDescription: 700 Series Chipset Family HD Audio Speaker
\tPorts:
\t\tanalog-output-speaker: Speakers (type: Speaker, priority: 10000, availability unknown)
\t\tanalog-output-headphones: Headphones (type: Headphones, priority: 9900, availability group: Legacy 1, not available)
\tActive Port: analog-output-speaker
";

    #[test]
    fn picks_working_speaker_over_unplugged_hdmi() {
        let sinks = parse_sinks_text(SAMPLE);
        assert_eq!(sinks.len(), 2);
        assert!(!sinks[0].usable && sinks[0].hdmi);
        assert!(sinks[1].usable && !sinks[1].hdmi);
        // even when the system default is the dead HDMI output
        let picked = pick_real(&sinks, &sinks[0].id).unwrap();
        assert!(picked.contains("analog-stereo"));
    }
}
