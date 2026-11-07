//! Audio engine: captures the system mix, applies gain + limiter, plays it
//! to the real output device.

pub mod dsp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::Engine;
#[cfg(windows)]
pub use windows::Engine;

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_GAIN: f32 = 3.0;

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Meter {
    /// Limiter gain reduction in dB (0 = none, negative = reducing).
    pub reduction_db: f32,
    /// Output peak, 0..1
    pub peak: f32,
}

/// State shared between the control side and the audio thread.
pub struct Shared {
    gain: AtomicU32,
    muted: AtomicBool,
    pub running: AtomicBool,
    reduction: AtomicU32,
    peak: AtomicU32,
}

impl Shared {
    pub fn new(gain: f32) -> Self {
        Self {
            gain: AtomicU32::new(gain.to_bits()),
            muted: AtomicBool::new(false),
            running: AtomicBool::new(true),
            reduction: AtomicU32::new(1.0f32.to_bits()),
            peak: AtomicU32::new(0),
        }
    }
    pub fn set_gain(&self, g: f32) {
        self.gain.store(g.clamp(0.0, MAX_GAIN).to_bits(), Ordering::Relaxed);
    }
    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }
    /// Effective gain for the audio thread.
    pub fn effective_gain(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.gain.load(Ordering::Relaxed))
        }
    }
    pub fn publish(&self, min_gain: f32, peak: f32) {
        // keep the worst reduction until the UI reads it
        let prev = f32::from_bits(self.reduction.load(Ordering::Relaxed));
        self.reduction.store(prev.min(min_gain).to_bits(), Ordering::Relaxed);
        let pp = f32::from_bits(self.peak.load(Ordering::Relaxed));
        self.peak.store(pp.max(peak).to_bits(), Ordering::Relaxed);
    }
    pub fn take_meter(&self) -> Meter {
        let g = f32::from_bits(self.reduction.swap(1.0f32.to_bits(), Ordering::Relaxed));
        let p = f32::from_bits(self.peak.swap(0, Ordering::Relaxed));
        Meter { reduction_db: 20.0 * g.max(1e-4).log10(), peak: p }
    }
}
