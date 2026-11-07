//! Gain + look-ahead peak limiter.
//!
//! The user gain (up to 3x) is applied first, then a look-ahead limiter pulls
//! the level down *before* a peak arrives (smooth ramp instead of hard
//! clipping), so boosted audio stays clean instead of crackling.

use std::collections::VecDeque;

pub const LOOKAHEAD_MS: f32 = 5.0;
pub const RELEASE_MS: f32 = 120.0;
pub const CEILING: f32 = 0.97; // about -0.26 dBFS

pub struct Processor {
    ch: usize,
    la: usize,
    gain_cur: f32,
    gain_tgt: f32,
    gain_coef: f32,
    delay: Vec<f32>, // (la - 1) frames
    pos: usize,
    deque: VecDeque<(u64, f32)>,
    idx: u64,
    env: f32,
    rel_coef: f32,
    avg: Vec<f32>,
    avg_pos: usize,
    avg_sum: f64,
    /// Lowest limiter gain seen since the last `take_stats` (1.0 = no reduction).
    min_gain: f32,
    /// Highest output peak since the last `take_stats`.
    peak: f32,
}

impl Processor {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let sr = sample_rate as f32;
        let la = ((LOOKAHEAD_MS / 1000.0 * sr) as usize).max(2);
        Self {
            ch: channels,
            la,
            gain_cur: 1.0,
            gain_tgt: 1.0,
            gain_coef: 1.0 - (-1.0 / (0.010 * sr)).exp(),
            delay: vec![0.0; (la - 1) * channels],
            pos: 0,
            deque: VecDeque::new(),
            idx: 0,
            env: 1.0,
            rel_coef: 1.0 - (-1.0 / (RELEASE_MS / 1000.0 * sr)).exp(),
            avg: vec![1.0; la],
            avg_pos: 0,
            avg_sum: la as f64,
            min_gain: 1.0,
            peak: 0.0,
        }
    }

    pub fn set_gain(&mut self, g: f32) {
        self.gain_tgt = g.max(0.0);
    }

    /// Returns (min limiter gain, output peak) since last call and resets them.
    pub fn take_stats(&mut self) -> (f32, f32) {
        let s = (self.min_gain, self.peak);
        self.min_gain = 1.0;
        self.peak = 0.0;
        s
    }

    /// Process interleaved samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let ch = self.ch;
        let dframes = self.la - 1;
        for frame in buf.chunks_exact_mut(ch) {
            self.gain_cur += (self.gain_tgt - self.gain_cur) * self.gain_coef;

            let mut peak = 0.0f32;
            for s in frame.iter_mut() {
                *s *= self.gain_cur;
                peak = peak.max(s.abs());
            }
            let req = if peak > CEILING { CEILING / peak } else { 1.0 };

            // sliding minimum over the last `la` requests
            while matches!(self.deque.back(), Some(&(_, v)) if v >= req) {
                self.deque.pop_back();
            }
            self.deque.push_back((self.idx, req));
            while matches!(self.deque.front(), Some(&(i, _)) if i + self.la as u64 <= self.idx) {
                self.deque.pop_front();
            }
            self.idx += 1;
            let m = self.deque.front().map(|x| x.1).unwrap_or(1.0);

            // instant attack (the look-ahead provides the ramp), slow release
            self.env = if m < self.env { m } else { self.env + (m - self.env) * self.rel_coef };

            // box-car smoothing over the look-ahead window
            self.avg_sum += (self.env - self.avg[self.avg_pos]) as f64;
            self.avg[self.avg_pos] = self.env;
            self.avg_pos = (self.avg_pos + 1) % self.la;
            let g = (self.avg_sum / self.la as f64) as f32;
            self.min_gain = self.min_gain.min(g);

            let base = self.pos * ch;
            for (c, s) in frame.iter_mut().enumerate() {
                let delayed = self.delay[base + c];
                self.delay[base + c] = *s;
                let y = (delayed * g).clamp(-1.0, 1.0);
                self.peak = self.peak.max(y.abs());
                *s = y;
            }
            self.pos = (self.pos + 1) % dframes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: u32, freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (sr as f32 * secs) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin();
            v.push(s);
            v.push(s);
        }
        v
    }

    /// THD of the left channel at `freq`, using a plain DFT over whole periods.
    fn thd(x: &[f32], sr: u32, freq: f32) -> f32 {
        let mono: Vec<f32> = x.iter().step_by(2).copied().collect();
        let period = (sr as f32 / freq) as usize;
        let n = (mono.len() / period) * period;
        let mag = |k: f32| -> f32 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, s) in mono[..n].iter().enumerate() {
                let a = 2.0 * std::f64::consts::PI * (k * freq) as f64 * i as f64 / sr as f64;
                re += *s as f64 * a.cos();
                im += *s as f64 * a.sin();
            }
            ((re * re + im * im).sqrt() * 2.0 / n as f64) as f32
        };
        let f = mag(1.0);
        let h: f32 = (2..9).map(|k| mag(k as f32).powi(2)).sum::<f32>().sqrt();
        h / f
    }

    #[test]
    fn unity_is_transparent_after_latency() {
        let sr = 48000;
        let mut p = Processor::new(sr, 2);
        let input = sine(sr, 440.0, 0.3, 0.5);
        let mut out = input.clone();
        p.process(&mut out);
        let d = (p.la - 1) * 2;
        for i in (sr as usize / 2)..(out.len() - d) {
            assert!((out[i + d] - input[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn boosted_never_exceeds_ceiling_and_is_clean() {
        let sr = 48000;
        let mut p = Processor::new(sr, 2);
        p.set_gain(3.0);
        let mut out = sine(sr, 220.0, 0.8, 2.0); // 0.8 * 3 = 2.4 (way over full scale)
        p.process(&mut out);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak <= 1.0);
        let settled = &out[out.len() / 2..];
        let t = thd(settled, sr, 220.0);
        // hard clipping the same signal for comparison
        let clipped: Vec<f32> = sine(sr, 220.0, 0.8 * 3.0, 1.0).iter().map(|v| v.clamp(-1.0, 1.0)).collect();
        let tc = thd(&clipped, sr, 220.0);
        println!("limiter THD {t:.4}  hard clip THD {tc:.4}");
        assert!(t < 0.02, "THD too high: {t}");
        assert!(tc > 5.0 * t);
    }

    #[test]
    fn attenuation_works() {
        let sr = 48000;
        let mut p = Processor::new(sr, 2);
        p.set_gain(0.25);
        let mut out = sine(sr, 440.0, 0.8, 1.0);
        p.process(&mut out);
        let peak = out[sr as usize..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((peak - 0.2).abs() < 0.01, "{peak}");
    }
}
