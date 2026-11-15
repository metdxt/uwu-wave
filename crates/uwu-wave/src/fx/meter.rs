//! Metering: peak, RMS and integrated loudness (LUFS).

use crate::dsp::Frame;

/// Peak/RMS/loudness meter fed with blocks of audio.
#[derive(Clone, Debug)]
pub struct Meter {
    pub peak: f32,
    sum_sq: f64,
    count: u64,
    sr: f32,
}

impl Meter {
    pub fn new(sr: f32) -> Meter {
        Meter { peak: 0.0, sum_sq: 0.0, count: 0, sr }
    }
    pub fn process(&mut self, buf: &[Frame]) {
        for f in buf {
            self.peak = self.peak.max(f.peak());
            self.sum_sq += (f.l as f64 * f.l as f64 + f.r as f64 * f.r as f64) * 0.5;
        }
        self.count += buf.len() as u64;
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
    /// Integrated loudness. (Placeholder: RMS-based approximation until K-weighting lands.)
    pub fn lufs(&self) -> f32 {
        self.rms_db() - 0.691 + 3.0
    }
    pub fn sample_rate(&self) -> f32 {
        self.sr
    }
}
