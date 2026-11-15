//! Filters.

use super::Frame;
use std::f32::consts::PI;

/// RBJ cookbook filter types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BiquadKind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    AllPass,
    /// Peaking EQ with gain in dB.
    Peak(f32),
    LowShelf(f32),
    HighShelf(f32),
}

/// Stereo biquad (transposed direct form II). Coefficients are shared, state is per channel.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: [0.0; 2], z2: [0.0; 2] }
    }
}

impl Biquad {
    pub fn new(kind: BiquadKind, freq: f32, q: f32, sr: f32) -> Biquad {
        let mut b = Biquad::default();
        b.set(kind, freq, q, sr);
        b
    }
    /// Recompute coefficients (state is kept, so this is safe to call every block).
    pub fn set(&mut self, kind: BiquadKind, freq: f32, q: f32, sr: f32) {
        let freq = freq.clamp(5.0, sr * 0.49);
        let q = q.max(0.01);
        let w0 = 2.0 * PI * freq / sr;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2);
        match kind {
            BiquadKind::LowPass => {
                b1 = 1.0 - cs;
                b0 = b1 * 0.5;
                b2 = b0;
                a0 = 1.0 + alpha;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha;
            }
            BiquadKind::HighPass => {
                b1 = -(1.0 + cs);
                b0 = (1.0 + cs) * 0.5;
                b2 = b0;
                a0 = 1.0 + alpha;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha;
            }
            BiquadKind::BandPass => {
                b0 = alpha;
                b1 = 0.0;
                b2 = -alpha;
                a0 = 1.0 + alpha;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha;
            }
            BiquadKind::Notch => {
                b0 = 1.0;
                b1 = -2.0 * cs;
                b2 = 1.0;
                a0 = 1.0 + alpha;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha;
            }
            BiquadKind::AllPass => {
                b0 = 1.0 - alpha;
                b1 = -2.0 * cs;
                b2 = 1.0 + alpha;
                a0 = 1.0 + alpha;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha;
            }
            BiquadKind::Peak(db) => {
                let a = 10f32.powf(db / 40.0);
                b0 = 1.0 + alpha * a;
                b1 = -2.0 * cs;
                b2 = 1.0 - alpha * a;
                a0 = 1.0 + alpha / a;
                a1 = -2.0 * cs;
                a2 = 1.0 - alpha / a;
            }
            BiquadKind::LowShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                let sq = 2.0 * a.sqrt() * alpha;
                b0 = a * ((a + 1.0) - (a - 1.0) * cs + sq);
                b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cs);
                b2 = a * ((a + 1.0) - (a - 1.0) * cs - sq);
                a0 = (a + 1.0) + (a - 1.0) * cs + sq;
                a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cs);
                a2 = (a + 1.0) + (a - 1.0) * cs - sq;
            }
            BiquadKind::HighShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                let sq = 2.0 * a.sqrt() * alpha;
                b0 = a * ((a + 1.0) + (a - 1.0) * cs + sq);
                b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cs);
                b2 = a * ((a + 1.0) + (a - 1.0) * cs - sq);
                a0 = (a + 1.0) - (a - 1.0) * cs + sq;
                a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cs);
                a2 = (a + 1.0) - (a - 1.0) * cs - sq;
            }
        }
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }
    /// Process one channel (`ch` = 0 or 1).
    #[inline]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = super::undenormal(self.b1 * x - self.a1 * y + self.z2[ch]);
        self.z2[ch] = super::undenormal(self.b2 * x - self.a2 * y);
        y
    }
    #[inline]
    pub fn process(&mut self, f: Frame) -> Frame {
        Frame::new(self.tick(0, f.l), self.tick(1, f.r))
    }
    pub fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }
}

/// Output of [`Svf::tick`]: all responses at once.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfOut {
    pub lp: f32,
    pub bp: f32,
    pub hp: f32,
}
impl SvfOut {
    pub fn notch(&self) -> f32 {
        self.lp + self.hp
    }
}

/// Mono TPT state-variable filter (Cytomic / Simper). Stable under fast modulation — ideal
/// for synth filters with envelopes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Svf {
    pub fn new(freq: f32, q: f32, sr: f32) -> Svf {
        let mut s = Svf::default();
        s.set(freq, q, sr);
        s
    }
    /// `q` ≈ 0.5 (soft) .. 20+ (screaming). 0.707 = Butterworth.
    #[inline]
    pub fn set(&mut self, freq: f32, q: f32, sr: f32) {
        let g = (PI * freq.clamp(5.0, sr * 0.49) / sr).tan();
        self.k = 1.0 / q.max(0.05);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
    #[inline]
    pub fn tick(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = super::undenormal(2.0 * v1 - self.ic1);
        self.ic2 = super::undenormal(2.0 * v2 - self.ic2);
        SvfOut { lp: v2, bp: v1, hp: x - self.k * v1 - v2 }
    }
    #[inline]
    pub fn lp(&mut self, x: f32) -> f32 {
        self.tick(x).lp
    }
    #[inline]
    pub fn hp(&mut self, x: f32) -> f32 {
        self.tick(x).hp
    }
    #[inline]
    pub fn bp(&mut self, x: f32) -> f32 {
        self.tick(x).bp
    }
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// One-pole lowpass (6 dB/oct); `hp()` gives the complementary highpass.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    a: f32,
    y: f32,
}
impl OnePole {
    pub fn new(freq: f32, sr: f32) -> OnePole {
        let mut o = OnePole::default();
        o.set(freq, sr);
        o
    }
    pub fn set(&mut self, freq: f32, sr: f32) {
        self.a = 1.0 - (-2.0 * PI * freq.clamp(1.0, sr * 0.49) / sr).exp();
    }
    #[inline]
    pub fn lp(&mut self, x: f32) -> f32 {
        self.y = super::undenormal(self.y + self.a * (x - self.y));
        self.y
    }
    #[inline]
    pub fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
    pub fn reset(&mut self) {
        self.y = 0.0;
    }
}

/// DC blocker (≈ 10 Hz highpass).
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}
impl DcBlock {
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.9985 * self.y1;
        self.x1 = x;
        self.y1 = super::undenormal(y);
        y
    }
}
