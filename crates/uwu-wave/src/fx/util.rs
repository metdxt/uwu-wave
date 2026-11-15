//! Small helpers shared by the effects: parameter smoothing, LFO phase, delay-time values,
//! level-knob conversion and a 2× oversampler.

use crate::ctx::Ctx;
use crate::dsp::Frame;
use crate::param::{Automation, Lane, Lfo, Param, Rate, Shape};
use crate::time::Time;
use crate::units::{db_to_gain, gain_to_db};

/// Interpret a *level* knob (threshold, ceiling, EQ gain…).
///
/// Levels are linear gains, so `-18.db()` (which converts to `0.126`) is the idiomatic way to
/// write them. As a convenience, values `<= 0` are read as decibels: `-18.0` means -18 dB and
/// `0` means 0 dB (a linear gain of zero would be meaningless for these knobs).
#[inline]
pub fn level(v: f32) -> f32 {
    if v <= 0.0 { db_to_gain(v) } else { v }
}

/// Same as [`level`] but returns decibels.
#[inline]
pub fn level_db(v: f32) -> f32 {
    if v <= 0.0 { v } else { gain_to_db(v) }
}

/// Amplitude → dB for per-sample detectors (floored at -200 dB).
#[inline]
pub(crate) fn amp_db(x: f32) -> f32 {
    20.0 * x.max(1e-10).log10()
}

/// dB → amplitude without the -144 dB silence floor of [`db_to_gain`].
#[inline]
pub(crate) fn db_amp(db: f32) -> f32 {
    10f32.powf(db * 0.05)
}

/// Dry/wet gains for "send-style" effects (reverb, delay): the dry signal stays at full level up
/// to `mix = 0.5`, the wet reaches full level at `mix = 0.5`.
#[inline]
pub(crate) fn send_mix(mix: f32) -> (f32, f32) {
    let m = mix.clamp(0.0, 1.0);
    ((2.0 * (1.0 - m)).min(1.0), (2.0 * m).min(1.0))
}

/// Coefficient of a one-pole smoother/envelope follower with time constant `secs`.
#[inline]
pub(crate) fn coef(secs: f32, sr: f32) -> f32 {
    if secs <= 0.0 {
        0.0
    } else {
        (-1.0 / (secs * sr)).exp()
    }
}

/// One-pole parameter smoother that can be advanced by whole sub-blocks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Smooth {
    pub value: f32,
    pub target: f32,
    coef: f32,
    init: bool,
}

impl Smooth {
    /// Set the smoothing time constant (seconds).
    pub fn time(&mut self, secs: f32, sr: f32) {
        self.coef = coef(secs, sr);
    }
    /// New target. The very first call jumps straight to it.
    #[inline]
    pub fn set(&mut self, t: f32) {
        self.target = t;
        if !self.init {
            self.value = t;
            self.init = true;
        }
    }
    /// Advance one sample.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.value = self.target + (self.value - self.target) * self.coef;
        self.value
    }
    /// Advance `n` samples at once.
    #[inline]
    pub fn advance(&mut self, n: usize) -> f32 {
        let d = self.value - self.target;
        if d.abs() <= 1e-6 * (1.0 + self.target.abs()) {
            self.value = self.target;
        } else {
            self.value = self.target + d * self.coef.powi(n as i32);
        }
        self.value
    }
    /// Jump to the target.
    pub fn snap(&mut self) {
        self.value = self.target;
    }
    /// Forget the state (the next `set` jumps).
    pub fn reset(&mut self) {
        self.init = false;
    }
}

/// Evaluate an LFO shape at a phase (cycles). Bipolar output `-1..1`.
#[inline]
pub(crate) fn shape_at(shape: Shape, ph: f64, seed: u64) -> f32 {
    let p = ph.rem_euclid(1.0) as f32;
    match shape {
        Shape::Sine => (p * std::f32::consts::TAU).sin(),
        Shape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
        Shape::Saw => 2.0 * p - 1.0,
        Shape::Ramp => 1.0 - 2.0 * p,
        Shape::Square => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Shape::SampleHold => crate::dsp::hash_bipolar(seed, ph.floor() as i64 as u64),
        Shape::Drift => {
            let n = ph.floor() as i64;
            let a = crate::dsp::hash_bipolar(seed, n as u64);
            let b = crate::dsp::hash_bipolar(seed, (n + 1) as u64);
            a + (b - a) * (0.5 - 0.5 * (std::f32::consts::PI * p).cos())
        }
    }
}

/// Phase source for effect LFOs: tempo-synced rates lock to the song position, Hz rates run free.
#[derive(Clone, Copy, Debug)]
pub struct LfoPhase {
    pub rate: Rate,
    acc: f64,
}

impl LfoPhase {
    pub fn new(rate: Rate) -> LfoPhase {
        LfoPhase { rate, acc: 0.0 }
    }
    /// `(phase at the first frame, phase increment per frame)` for a block of `n` frames.
    #[inline]
    pub fn block(&mut self, ctx: &Ctx, n: usize) -> (f64, f64) {
        match self.rate {
            Rate::Period(p) => (ctx.clock.beat / p, ctx.beats_per_sample() / p),
            Rate::Hz(hz) => {
                let inc = hz / ctx.sr as f64;
                let ph = self.acc;
                self.acc = (self.acc + inc * n as f64).rem_euclid(1.0);
                (ph, inc)
            }
        }
    }
    pub fn reset(&mut self) {
        self.acc = 0.0;
    }
}

/// A delay time: tempo-synced (`dotted(eighth())`, `3.beats()`) or in seconds (`0.3`,
/// `250.ms()`, or any automatable [`Param`] in seconds).
#[derive(Clone, Debug)]
pub enum DelayTime {
    /// Musical length in beats (follows tempo changes).
    Beats(Param),
    /// Seconds.
    Secs(Param),
}

impl DelayTime {
    /// A synced time given as an automatable number of beats.
    pub fn beats(p: impl Into<Param>) -> DelayTime {
        DelayTime::Beats(p.into())
    }
    /// Seconds.
    pub fn secs(p: impl Into<Param>) -> DelayTime {
        DelayTime::Secs(p.into())
    }
    /// Current value in seconds.
    #[inline]
    pub fn at(&self, ctx: &Ctx) -> f32 {
        match self {
            DelayTime::Beats(p) => (ctx.p(p) as f64 * 60.0 / ctx.bpm) as f32,
            DelayTime::Secs(p) => ctx.p(p),
        }
    }
    /// Constant length in seconds, assuming tempos down to 60 BPM for synced times.
    pub(crate) fn const_hint(&self) -> Option<f32> {
        match self {
            DelayTime::Beats(p) => p.as_const().map(|b| b.abs()),
            DelayTime::Secs(p) => p.as_const().map(|s| s.abs()),
        }
    }
}

impl From<Time> for DelayTime {
    fn from(t: Time) -> DelayTime {
        DelayTime::Beats(Param::Const(t.0 as f32))
    }
}
macro_rules! dt_secs { ($($t:ty),*) => { $(impl From<$t> for DelayTime { fn from(v: $t) -> DelayTime { DelayTime::Secs(v.into()) } })* } }
dt_secs!(f32, f64, i32, Param, Lfo, Automation, Lane, &Lane);

// ---- 2× oversampling ---------------------------------------------------------------------------

const HB_TAPS: usize = 47; // half-band FIR length (centre tap = 23)
const HB_EVEN: usize = 24; // non-zero even taps
const HB_MASK: usize = 31;

/// Mono 2× oversampler built from a 47-tap Blackman half-band FIR (≈ 75 dB stop band).
///
/// `up(x)` yields two samples at twice the rate; `down(a, b)` turns two oversampled samples back
/// into one. Round-trip latency is [`Os2x::LATENCY`] samples at the base rate.
#[derive(Clone, Debug)]
pub struct Os2x {
    h: [f32; HB_EVEN],
    up_hist: [f32; 32],
    up_pos: usize,
    even: [f32; 32],
    odd: [f32; 32],
    dn_pos: usize,
}

impl Default for Os2x {
    fn default() -> Self {
        Os2x::new()
    }
}

impl Os2x {
    /// Round-trip latency at the base rate.
    pub const LATENCY: usize = 23;

    pub fn new() -> Os2x {
        use std::f64::consts::PI;
        let m = (HB_TAPS - 1) as f64 / 2.0;
        let mut sum = 0.0f64;
        let mut raw = [0f64; HB_EVEN];
        for (k, r) in raw.iter_mut().enumerate() {
            let n = (2 * k) as f64;
            let t = n - m; // odd offsets from the centre
            let sinc = (PI * 0.5 * t).sin() / (PI * 0.5 * t);
            let w = 0.42 - 0.5 * (2.0 * PI * n / (HB_TAPS - 1) as f64).cos()
                + 0.08 * (4.0 * PI * n / (HB_TAPS - 1) as f64).cos();
            *r = 0.5 * sinc * w;
            sum += *r;
        }
        let mut h = [0f32; HB_EVEN];
        for k in 0..HB_EVEN {
            h[k] = (raw[k] * 0.5 / sum) as f32; // even taps sum to 0.5, centre is 0.5 → DC gain 1
        }
        Os2x {
            h,
            up_hist: [0.0; 32],
            up_pos: 0,
            even: [0.0; 32],
            odd: [0.0; 32],
            dn_pos: 0,
        }
    }

    /// Upsample one base-rate sample into two.
    #[inline]
    pub fn up(&mut self, x: f32) -> (f32, f32) {
        self.up_pos = (self.up_pos + 1) & HB_MASK;
        self.up_hist[self.up_pos] = x;
        let mut a = 0.0;
        for k in 0..HB_EVEN {
            a += self.h[k] * self.up_hist[self.up_pos.wrapping_sub(k) & HB_MASK];
        }
        let b = self.up_hist[self.up_pos.wrapping_sub(11) & HB_MASK];
        (2.0 * a, b)
    }

    /// Downsample two oversampled samples into one.
    #[inline]
    pub fn down(&mut self, a: f32, b: f32) -> f32 {
        self.dn_pos = (self.dn_pos + 1) & HB_MASK;
        self.even[self.dn_pos] = a;
        self.odd[self.dn_pos] = b;
        let mut y = 0.0;
        for k in 0..HB_EVEN {
            y += self.h[k] * self.even[self.dn_pos.wrapping_sub(k) & HB_MASK];
        }
        y + 0.5 * self.odd[self.dn_pos.wrapping_sub(12) & HB_MASK]
    }

    pub fn reset(&mut self) {
        self.up_hist = [0.0; 32];
        self.even = [0.0; 32];
        self.odd = [0.0; 32];
    }
}

/// Short fixed delay (< 32 samples) used to align dry signals with oversampled paths.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ShortDelay {
    buf: [Frame; 32],
    pos: usize,
}

impl ShortDelay {
    #[inline]
    pub fn tick(&mut self, x: Frame, d: usize) -> Frame {
        self.pos = (self.pos + 1) & 31;
        self.buf[self.pos] = x;
        self.buf[self.pos.wrapping_sub(d) & 31]
    }
    pub fn reset(&mut self) {
        *self = ShortDelay::default();
    }
}

// ---- fast power-of-two delay line -------------------------------------------------------------

/// Mono circular delay line with power-of-two capacity (mask indexing, no divisions).
///
/// Same reading conventions as [`crate::dsp::DelayLine`]: `tap(0)` is the last pushed sample.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl Ring {
    /// Room for at least `len` samples of delay.
    pub fn new(len: usize) -> Ring {
        let n = (len + 4).next_power_of_two();
        Ring {
            buf: vec![0.0; n],
            mask: n - 1,
            pos: 0,
        }
    }
    /// Room for `secs` seconds at `sr`.
    pub fn with_secs(secs: f32, sr: f32) -> Ring {
        Ring::new((secs * sr).ceil() as usize + 4)
    }
    /// Largest usable delay (samples) for cubic reads.
    pub fn max_delay(&self) -> f32 {
        self.buf.len().saturating_sub(4) as f32
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) & self.mask;
        self.buf[self.pos] = x;
    }
    #[inline]
    pub fn tap(&self, d: usize) -> f32 {
        self.buf[self.pos.wrapping_sub(d) & self.mask]
    }
    /// Linear-interpolated read `d` samples back.
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let d = d.clamp(0.0, self.max_delay());
        let i = d as usize;
        let f = d - i as f32;
        let a = self.tap(i);
        a + (self.tap(i + 1) - a) * f
    }
    /// Cubic (Hermite) read — for modulated delays.
    #[inline]
    pub fn read_cubic(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, self.max_delay());
        let i = d as usize;
        let f = d - i as f32;
        let (xm1, x0, x1, x2) = (
            self.tap(i - 1),
            self.tap(i),
            self.tap(i + 1),
            self.tap(i + 2),
        );
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * f + c2) * f + c1) * f + x0
    }
    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert!((level(-6.0) - 0.501).abs() < 0.01);
        assert_eq!(level(0.0), 1.0);
        assert_eq!(level(0.5), 0.5);
        assert!((level_db(0.5) + 6.02).abs() < 0.01);
    }

    #[test]
    fn oversampler_round_trip() {
        let mut os = Os2x::new();
        let sr = 48000.0;
        let w = std::f32::consts::TAU * 1000.0 / sr;
        let mut max_err = 0f32;
        for i in 0..2000usize {
            let (a, b) = os.up((i as f32 * w).sin());
            let y = os.down(a, b);
            if i > 100 {
                let xr = ((i - Os2x::LATENCY) as f32 * w).sin();
                max_err = max_err.max((y - xr).abs());
            }
        }
        assert!(max_err < 0.01, "{max_err}");
    }
}
