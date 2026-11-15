//! Low-level DSP building blocks shared by instruments and effects.
//!
//! * [`Frame`] – a stereo sample with arithmetic ops
//! * [`filter`] – RBJ [`Biquad`](filter::Biquad), TPT state-variable [`Svf`](filter::Svf), [`OnePole`](filter::OnePole), [`DcBlock`](filter::DcBlock)
//! * [`delay`] – fractional [`DelayLine`](delay::DelayLine)
//! * [`env`] – [`Adsr`](env::Adsr) envelope
//! * [`osc`] – band-limited (PolyBLEP) [`Osc`](osc::Osc)
//! * [`smooth`] – parameter smoothing
//! * [`Rng`] – tiny deterministic PRNG, [`hash01`] stateless hash noise

pub mod delay;
pub mod env;
pub mod filter;
pub mod osc;
pub mod smooth;

pub use delay::DelayLine;
pub use env::{Adsr, AdsrStage};
pub use filter::{Biquad, BiquadKind, DcBlock, OnePole, Svf, SvfOut};
pub use osc::{Osc, Wave};
pub use smooth::{Smoothed, Ramp};

use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

pub const TAU: f32 = std::f32::consts::TAU;
pub const PI: f32 = std::f32::consts::PI;

/// One stereo sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub l: f32,
    pub r: f32,
}

impl Frame {
    pub const ZERO: Frame = Frame { l: 0.0, r: 0.0 };
    #[inline]
    pub const fn new(l: f32, r: f32) -> Frame {
        Frame { l, r }
    }
    /// Same value on both channels.
    #[inline]
    pub const fn mono(x: f32) -> Frame {
        Frame { l: x, r: x }
    }
    /// (L+R)/2
    #[inline]
    pub fn mid(self) -> f32 {
        0.5 * (self.l + self.r)
    }
    /// (L-R)/2
    #[inline]
    pub fn side(self) -> f32 {
        0.5 * (self.l - self.r)
    }
    #[inline]
    pub fn from_mid_side(m: f32, s: f32) -> Frame {
        Frame { l: m + s, r: m - s }
    }
    /// max(|L|, |R|)
    #[inline]
    pub fn peak(self) -> f32 {
        self.l.abs().max(self.r.abs())
    }
    #[inline]
    pub fn map(self, mut f: impl FnMut(f32) -> f32) -> Frame {
        Frame { l: f(self.l), r: f(self.r) }
    }
    /// Constant-power pan of a mono value (`pan` in -1..1).
    #[inline]
    pub fn panned(x: f32, pan: f32) -> Frame {
        let (gl, gr) = pan_gains(pan);
        Frame { l: x * gl, r: x * gr }
    }
    /// Linear crossfade: `self*(1-t) + o*t`.
    #[inline]
    pub fn lerp(self, o: Frame, t: f32) -> Frame {
        self * (1.0 - t) + o * t
    }
}

/// Constant-power pan gains; unity at center.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let th = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (th.cos() * std::f32::consts::SQRT_2, th.sin() * std::f32::consts::SQRT_2)
}

impl Add for Frame { type Output = Frame; #[inline] fn add(self, o: Frame) -> Frame { Frame::new(self.l + o.l, self.r + o.r) } }
impl Sub for Frame { type Output = Frame; #[inline] fn sub(self, o: Frame) -> Frame { Frame::new(self.l - o.l, self.r - o.r) } }
impl AddAssign for Frame { #[inline] fn add_assign(&mut self, o: Frame) { self.l += o.l; self.r += o.r; } }
impl SubAssign for Frame { #[inline] fn sub_assign(&mut self, o: Frame) { self.l -= o.l; self.r -= o.r; } }
impl Mul<f32> for Frame { type Output = Frame; #[inline] fn mul(self, k: f32) -> Frame { Frame::new(self.l * k, self.r * k) } }
impl Mul<Frame> for f32 { type Output = Frame; #[inline] fn mul(self, f: Frame) -> Frame { Frame::new(f.l * self, f.r * self) } }
impl Mul for Frame { type Output = Frame; #[inline] fn mul(self, o: Frame) -> Frame { Frame::new(self.l * o.l, self.r * o.r) } }
impl MulAssign<f32> for Frame { #[inline] fn mul_assign(&mut self, k: f32) { self.l *= k; self.r *= k; } }
impl Neg for Frame { type Output = Frame; #[inline] fn neg(self) -> Frame { Frame::new(-self.l, -self.r) } }
impl From<f32> for Frame { fn from(x: f32) -> Frame { Frame::mono(x) } }
impl From<(f32, f32)> for Frame { fn from((l, r): (f32, f32)) -> Frame { Frame::new(l, r) } }

/// MIDI note number (fractional allowed) → Hz. A4 = 69 = 440 Hz.
#[inline]
pub fn midi_to_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}
/// Hz → fractional MIDI note number.
#[inline]
pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz.max(1e-6) / 440.0).log2()
}

/// Fast tanh-like soft clipper (Padé approximation, exact-ish for |x| < 3, clamps beyond).
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Coefficient for a one-pole smoother reaching ~63% in `time` seconds.
#[inline]
pub fn time_coef(time: f32, sr: f32) -> f32 {
    if time <= 0.0 { 0.0 } else { (-1.0 / (time * sr)).exp() }
}

/// Flush denormals / tiny values to zero.
#[inline]
pub fn undenormal(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

/// Tiny, fast, deterministic PRNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(splitmix(seed) | 1)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform in [0, 1).
    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
    /// Uniform integer in [0, n).
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    /// Approximately gaussian (sum of 4 uniforms), mean 0, std ≈ 1.
    pub fn gauss(&mut self) -> f32 {
        (self.f32() + self.f32() + self.f32() + self.f32() - 2.0) * 1.732
    }
}

#[inline]
pub fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// Stateless hash noise in [0, 1) for (seed, index).
#[inline]
pub fn hash01(seed: u64, i: u64) -> f32 {
    (splitmix(seed ^ splitmix(i)) >> 40) as f32 / (1u64 << 24) as f32
}
/// Stateless hash noise in [-1, 1).
#[inline]
pub fn hash_bipolar(seed: u64, i: u64) -> f32 {
    hash01(seed, i) * 2.0 - 1.0
}
