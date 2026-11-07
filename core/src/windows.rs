//! Windows backend (WASAPI through cpal).
//!
//! System audio is sent to a virtual cable ("CABLE Input", VB-Audio Virtual
//! Cable, free). This engine records from "CABLE Output", applies gain +
//! limiter and plays the result on the real speakers/headphones.

use crate::{dsp::Processor, Device, Meter, Shared, MAX_GAIN};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

const CABLE: &str = "CABLE Output";

fn is_cable(name: &str) -> bool {
    name.contains("CABLE")
}

pub struct Engine {
    shared: Arc<Shared>,
    _streams: (Stream, Stream),
    output: String,
}

// cpal streams are not Send on Windows, but we only keep them alive here and
// drop them from the thread that owns the engine (guarded by a Mutex in the app).
unsafe impl Send for Engine {}

impl Engine {
    pub fn outputs() -> Vec<Device> {
        let host = cpal::default_host();
        host.output_devices()
            .map(|it| {
                it.filter_map(|d| d.name().ok())
                    .filter(|n| !is_cable(n))
                    .map(|n| Device { id: n.clone(), name: n })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn start(output: Option<String>, gain: f32) -> Result<Self, String> {
        let host = cpal::default_host();
        let input = host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n.contains(CABLE)).unwrap_or(false))
            .ok_or("Virtual cable not found. Install VB-Audio Virtual Cable, then set \"CABLE Input\" as your Windows default playback device.")?;

        let out_dev = {
            let wanted = output.clone();
            let mut found = None;
            for d in host.output_devices().map_err(|e| e.to_string())? {
                let Ok(n) = d.name() else { continue };
                if is_cable(&n) {
                    continue;
                }
                if wanted.as_deref() == Some(n.as_str()) {
                    found = Some(d);
                    break;
                }
                if found.is_none() {
                    found = Some(d);
                }
            }
            found.ok_or("no output device found")?
        };
        let out_name = out_dev.name().unwrap_or_default();

        let out_cfg = out_dev.default_output_config().map_err(|e| e.to_string())?;
        let rate = out_cfg.sample_rate();
        let out_ch = out_cfg.channels() as usize;

        // pick an f32 input config with the same sample rate
        let in_cfg = input
            .supported_input_configs()
            .map_err(|e| e.to_string())?
            .filter(|c| c.sample_format() == SampleFormat::F32)
            .find_map(|c| c.try_with_sample_rate(rate))
            .ok_or("virtual cable does not support the output sample rate; set both to 48000 Hz in Windows sound settings")?;
        let in_ch = in_cfg.channels() as usize;
        if out_cfg.sample_format() != SampleFormat::F32 {
            return Err("output device is not f32 capable".into());
        }

        let shared = Arc::new(Shared::new(gain.clamp(0.0, MAX_GAIN)));
        let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let max_samples = (rate.0 as usize / 5) * in_ch; // keep at most ~200 ms

        let ring_in = ring.clone();
        let in_stream = input
            .build_input_stream(
                &in_cfg.config(),
                move |data: &[f32], _| {
                    let mut r = ring_in.lock().unwrap();
                    r.extend(data.iter().copied());
                    while r.len() > max_samples {
                        r.pop_front();
                    }
                },
                |e| eprintln!("input error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        let sh = shared.clone();
        let mut proc = Processor::new(rate.0, out_ch);
        let out_stream = out_dev
            .build_output_stream(
                &out_cfg.config(),
                move |data: &mut [f32], _| {
                    {
                        let mut r = ring.lock().unwrap();
                        for frame in data.chunks_exact_mut(out_ch) {
                            if r.len() >= in_ch {
                                let src: Vec<f32> = r.drain(..in_ch).collect();
                                for (c, s) in frame.iter_mut().enumerate() {
                                    *s = *src.get(c).unwrap_or(&0.0);
                                }
                            } else {
                                frame.fill(0.0);
                            }
                        }
                    }
                    proc.set_gain(sh.effective_gain());
                    proc.process(data);
                    let (g, p) = proc.take_stats();
                    sh.publish(g, p);
                },
                |e| eprintln!("output error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        in_stream.play().map_err(|e| e.to_string())?;
        out_stream.play().map_err(|e| e.to_string())?;

        Ok(Self { shared, _streams: (in_stream, out_stream), output: out_name })
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
        &self.output
    }
    pub fn stop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
    }
}
