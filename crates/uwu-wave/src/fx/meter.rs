//! Metering: sample peak, RMS and ITU-R BS.1770-4 / EBU R128 loudness (K-weighted, gated LUFS).

use crate::dsp::Frame;

/// Direct-form-I biquad in f64 (metering precision).
#[derive(Clone, Copy, Debug, Default)]
struct Bq {
    b: [f64; 3],
    a: [f64; 2],
    x: [[f64; 2]; 2],
    y: [[f64; 2]; 2],
}
impl Bq {
    #[inline]
    fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let [x1, x2] = self.x[ch];
        let [y1, y2] = self.y[ch];
        let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[0] * y1 - self.a[1] * y2;
        let y = if y.abs() < 1e-30 { 0.0 } else { y };
        self.x[ch] = [x, x1];
        self.y[ch] = [y, y1];
        y
    }
}

/// K-weighting filters (BS.1770 "pre-filter" high shelf + RLB high-pass) for any sample rate.
fn k_weighting(sr: f64) -> (Bq, Bq) {
    use std::f64::consts::PI;
    let f0 = 1681.974450955533;
    let g = 3.999843853973347;
    let q = 0.7071752369554196;
    let k = (PI * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Bq {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        ..Default::default()
    };
    let f0 = 38.13547087602444;
    let q = 0.5003270373238773;
    let k = (PI * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Bq {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        ..Default::default()
    };
    (shelf, hp)
}

#[inline]
fn loudness(ms: f64) -> f64 {
    if ms <= 0.0 {
        -160.0
    } else {
        -0.691 + 10.0 * ms.log10()
    }
}

const SHORT_SUBS: usize = 30; // 3 s of 100 ms sub-blocks

/// Peak/RMS/loudness meter fed with blocks of audio.
///
/// Loudness follows ITU-R BS.1770-4: K-weighted mean square summed over both channels, 400 ms
/// blocks with 75% overlap, absolute gate at -70 LUFS and relative gate at -10 LU.
///
/// ```ignore
/// let mut m = Meter::new(48_000.0);
/// m.process(&render.audio);
/// println!("{:.1} LUFS, peak {:.1} dBFS", m.lufs(), m.peak_db());
/// ```
#[derive(Clone, Debug)]
pub struct Meter {
    /// Highest absolute sample value seen so far.
    pub peak: f32,
    sum_sq: f64,
    count: u64,
    sr: f32,
    shelf: Bq,
    hp: Bq,
    sub_len: usize,
    sub_acc: f64,
    sub_n: usize,
    subs: [f64; SHORT_SUBS],
    sub_count: u64,
    blocks: Vec<f64>,
    k_sum: f64,
    max_m: f64,
    max_s: f64,
}

impl Meter {
    pub fn new(sr: f32) -> Meter {
        let (shelf, hp) = k_weighting(sr as f64);
        Meter {
            peak: 0.0,
            sum_sq: 0.0,
            count: 0,
            sr,
            shelf,
            hp,
            sub_len: ((sr as f64) * 0.1).round().max(1.0) as usize,
            sub_acc: 0.0,
            sub_n: 0,
            subs: [0.0; SHORT_SUBS],
            sub_count: 0,
            blocks: Vec::with_capacity(4096),
            k_sum: 0.0,
            max_m: 0.0,
            max_s: 0.0,
        }
    }

    pub fn process(&mut self, buf: &[Frame]) {
        for f in buf {
            self.peak = self.peak.max(f.peak());
            let (l, r) = (f.l as f64, f.r as f64);
            self.sum_sq += (l * l + r * r) * 0.5;
            let kl = self.hp.tick(0, self.shelf.tick(0, l));
            let kr = self.hp.tick(1, self.shelf.tick(1, r));
            let e = kl * kl + kr * kr;
            self.k_sum += e;
            self.sub_acc += e;
            self.sub_n += 1;
            if self.sub_n == self.sub_len {
                self.finish_sub();
            }
        }
        self.count += buf.len() as u64;
    }

    fn finish_sub(&mut self) {
        let ms = self.sub_acc / self.sub_len as f64;
        self.sub_acc = 0.0;
        self.sub_n = 0;
        self.subs[(self.sub_count as usize) % SHORT_SUBS] = ms;
        self.sub_count += 1;
        let avg = |n: usize, subs: &[f64; SHORT_SUBS], count: u64| -> f64 {
            (0..n)
                .map(|k| subs[((count as usize) + SHORT_SUBS - 1 - k) % SHORT_SUBS])
                .sum::<f64>()
                / n as f64
        };
        if self.sub_count >= 4 {
            let block = avg(4, &self.subs, self.sub_count);
            self.blocks.push(block);
            self.max_m = self.max_m.max(block);
        }
        if self.sub_count >= SHORT_SUBS as u64 {
            self.max_s = self.max_s.max(avg(SHORT_SUBS, &self.subs, self.sub_count));
        }
    }

    pub fn peak_db(&self) -> f32 {
        crate::units::gain_to_db(self.peak)
    }
    pub fn rms_db(&self) -> f32 {
        if self.count == 0 {
            return -160.0;
        }
        crate::units::gain_to_db((self.sum_sq / self.count as f64).sqrt() as f32)
    }
    /// Integrated (gated) loudness in LUFS. Programmes shorter than one 400 ms block report the
    /// ungated K-weighted loudness of what was seen.
    pub fn lufs(&self) -> f32 {
        if self.blocks.is_empty() {
            if self.count == 0 {
                return -160.0;
            }
            return loudness(self.k_sum / self.count as f64) as f32;
        }
        let abs_gated: Vec<f64> = self
            .blocks
            .iter()
            .copied()
            .filter(|&b| loudness(b) > -70.0)
            .collect();
        if abs_gated.is_empty() {
            return -160.0;
        }
        let rel = loudness(abs_gated.iter().sum::<f64>() / abs_gated.len() as f64) - 10.0;
        let (s, n) = abs_gated
            .iter()
            .filter(|&&b| loudness(b) > rel)
            .fold((0.0, 0usize), |(s, n), &b| (s + b, n + 1));
        if n == 0 {
            -160.0
        } else {
            loudness(s / n as f64) as f32
        }
    }
    /// Loudest momentary (400 ms) loudness seen, LUFS.
    pub fn max_momentary(&self) -> f32 {
        loudness(self.max_m) as f32
    }
    /// Loudest short-term (3 s) loudness seen, LUFS.
    pub fn max_short_term(&self) -> f32 {
        loudness(self.max_s) as f32
    }
    pub fn sample_rate(&self) -> f32 {
        self.sr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: f32, hz: f32, amp: f32, secs: f32) -> Vec<Frame> {
        (0..(sr * secs) as usize)
            .map(|i| Frame::mono(amp * (i as f32 * std::f32::consts::TAU * hz / sr).sin()))
            .collect()
    }

    #[test]
    fn reference_tone_reads_correctly() {
        // EBU Tech 3341: a 997 Hz sine of peak level X dBFS in both channels reads X LUFS (±0.1)
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut m = Meter::new(sr);
            for chunk in sine(sr, 997.0, 0.1, 10.0).chunks(64) {
                m.process(chunk);
            }
            assert!((m.lufs() + 20.0).abs() < 0.1, "{sr}: {}", m.lufs());
            assert!((m.max_short_term() + 20.0).abs() < 0.1);
            assert!((m.peak_db() + 20.0).abs() < 0.01);
        }
    }

    #[test]
    fn gating_ignores_silence() {
        let sr = 48_000.0;
        let mut m = Meter::new(sr);
        m.process(&sine(sr, 997.0, 0.1, 5.0));
        m.process(&vec![Frame::ZERO; 48_000 * 20]);
        assert!((m.lufs() + 20.0).abs() < 0.2, "{}", m.lufs());
        // ...while RMS is diluted by it
        assert!(m.rms_db() < -29.0);
    }

    #[test]
    fn k_weighting_lifts_highs_cuts_lows() {
        let sr = 48_000.0;
        let lufs = |hz: f32| {
            let mut m = Meter::new(sr);
            m.process(&sine(sr, hz, 0.1, 3.0));
            m.lufs()
        };
        assert!(lufs(10_000.0) > -20.0 + 3.0);
        assert!(lufs(30.0) < -20.0 - 1.0);
    }
}
