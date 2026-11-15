//! Saturation and destruction: [`Distortion`] (soft, hard, tube, fold, fuzz — 2× oversampled),
//! [`TapeSat`] and [`Bitcrush`].

use super::Effect;
use super::filter::{SvfMode, SvfStage};
use super::util::{Os2x, ShortDelay};
use crate::ctx::Ctx;
use crate::dsp::{DcBlock, Frame, Ramp};
use crate::param::Param;
use std::f32::consts::FRAC_PI_2;

/// Waveshaper curve of a [`Distortion`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DistMode {
    /// Smooth tanh saturation.
    Soft,
    /// Hard clipping.
    Hard,
    /// Asymmetric, even-harmonic "tube" warmth.
    Tube,
    /// Sine wavefolder — metallic, west-coast timbres.
    Fold,
    /// Biased, gated, buzzy fuzz.
    Fuzz,
    /// Gentle asymmetric tape curve (used by [`TapeSat`]).
    Tape,
}

#[inline]
fn shape(mode: DistMode, x: f32) -> f32 {
    match mode {
        DistMode::Soft => x.tanh(),
        DistMode::Hard => x.clamp(-1.0, 1.0),
        DistMode::Tube => {
            let x = x + 0.2;
            if x >= 0.0 {
                x.tanh()
            } else {
                (0.6 * x).tanh() / 0.6
            }
        }
        DistMode::Fold => (x * FRAC_PI_2).sin(),
        DistMode::Fuzz => {
            let x = 2.5 * x + 0.35;
            if x > 0.0 {
                1.0 - (-x).exp()
            } else {
                -0.85 * (1.0 - (1.6 * x).exp())
            }
        }
        DistMode::Tape => {
            let x = x.clamp(-4.0, 4.0);
            (x + 0.08 * x * x).tanh()
        }
    }
}

/// Distortion with drive, tone, dry/wet and output level. Runs 2× oversampled by default to keep
/// aliasing down (the dry path is delay-compensated).
///
/// ```ignore
/// Distortion::soft(4.0)                              // drive as linear gain…
/// Distortion::tube(12.db()).tone(6000.0).mix(0.5)    // …or in dB
/// Distortion::fold(lfo::sine(1.bars()).range(1.0, 6.0))
/// Distortion::fuzz(20.0).output(-6.db())
/// ```
#[derive(Clone, Debug)]
pub struct Distortion {
    mode: DistMode,
    drive: Param,
    tone: Param,
    mix: Param,
    output: Param,
    oversample: bool,
    os: [Os2x; 2],
    dry_dl: ShortDelay,
    dc: [DcBlock; 2],
    lp: SvfStage,
    drive_r: Ramp,
    out_r: Ramp,
    mix_r: Ramp,
    sr: f32,
}

impl Distortion {
    pub fn new(mode: DistMode) -> Distortion {
        Distortion {
            mode,
            drive: Param::Const(4.0),
            tone: Param::Const(20_000.0),
            mix: Param::Const(1.0),
            output: Param::Const(1.0),
            oversample: true,
            os: Default::default(),
            dry_dl: ShortDelay::default(),
            dc: Default::default(),
            lp: SvfStage::default(),
            drive_r: Ramp::default(),
            out_r: Ramp::default(),
            mix_r: Ramp::default(),
            sr: 48_000.0,
        }
    }
    pub fn soft(drive: impl Into<Param>) -> Distortion {
        Distortion::new(DistMode::Soft).drive(drive)
    }
    pub fn hard(drive: impl Into<Param>) -> Distortion {
        Distortion::new(DistMode::Hard).drive(drive)
    }
    pub fn tube(drive: impl Into<Param>) -> Distortion {
        Distortion::new(DistMode::Tube).drive(drive)
    }
    pub fn fold(drive: impl Into<Param>) -> Distortion {
        Distortion::new(DistMode::Fold).drive(drive)
    }
    pub fn fuzz(drive: impl Into<Param>) -> Distortion {
        Distortion::new(DistMode::Fuzz).drive(drive).tone(7_000.0)
    }
    /// Input gain into the shaper (linear; `12.db()` works).
    pub fn drive(mut self, d: impl Into<Param>) -> Self {
        self.drive = d.into();
        self
    }
    /// Post-shaper low-pass in Hz (darker = lower).
    pub fn tone(mut self, hz: impl Into<Param>) -> Self {
        self.tone = hz.into();
        self
    }
    /// Dry/wet (linear crossfade).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// Output gain (linear; `-6.db()`).
    pub fn output(mut self, g: impl Into<Param>) -> Self {
        self.output = g.into();
        self
    }
    /// Change the curve.
    pub fn mode(mut self, m: DistMode) -> Self {
        self.mode = m;
        self
    }
    /// Turn 2× oversampling on/off (on by default).
    pub fn oversample(mut self, on: bool) -> Self {
        self.oversample = on;
        self
    }
}

impl Effect for Distortion {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let n = buf.len();
        let sr = self.sr;
        let drive = ctx.p(&self.drive).clamp(0.01, 1000.0);
        self.drive_r.start(drive, n);
        // loose loudness compensation so turning up drive doesn't just mean "louder"
        self.out_r
            .start(ctx.p(&self.output) / drive.max(1.0).sqrt(), n);
        self.mix_r.start(ctx.p(&self.mix).clamp(0.0, 1.0), n);
        let tone = ctx.p(&self.tone).clamp(200.0, sr * 0.45);
        let use_tone = tone < sr * 0.44 && tone < 19_999.0;
        if use_tone {
            self.lp.set(SvfMode::LowPass, tone, 0.6, sr);
        }
        let mode = self.mode;
        for f in buf.iter_mut() {
            let d = self.drive_r.next();
            let x = [f.l * d, f.r * d];
            let mut y = [0f32; 2];
            for ch in 0..2 {
                let s = if self.oversample {
                    let (a, b) = self.os[ch].up(x[ch]);
                    self.os[ch].down(shape(mode, a), shape(mode, b))
                } else {
                    shape(mode, x[ch])
                };
                let s = self.dc[ch].tick(s);
                y[ch] = if use_tone { self.lp.tick(ch, s) } else { s };
            }
            let dry = if self.oversample {
                self.dry_dl.tick(*f, Os2x::LATENCY)
            } else {
                *f
            };
            let g = self.out_r.next();
            let m = self.mix_r.next();
            *f = dry * (1.0 - m) + Frame::new(y[0], y[1]) * (g * m);
        }
    }
    fn reset(&mut self) {
        self.os.iter_mut().for_each(|o| o.reset());
        self.dry_dl.reset();
        self.dc = Default::default();
        self.lp.reset();
    }
    fn tail(&self) -> f32 {
        0.01
    }
    fn name(&self) -> String {
        format!("distortion({:?})", self.mode).to_lowercase()
    }
}

/// Tape saturation: soft asymmetric compression of peaks, a little low-end "head bump" and a
/// gentle high roll-off. Glue for drums, bass and whole mixes.
///
/// ```ignore
/// TapeSat::new()                       // subtle
/// TapeSat::new().drive(6.db()).tone(9000.0)
/// ```
#[derive(Clone, Debug)]
pub struct TapeSat {
    dist: Distortion,
    bump: SvfStage,
    bump_db: f32,
}

impl TapeSat {
    pub fn new() -> TapeSat {
        TapeSat {
            dist: Distortion::new(DistMode::Tape).drive(2.0).tone(14_000.0),
            bump: SvfStage::default(),
            bump_db: 1.5,
        }
    }
    /// Drive into the tape (linear gain, default 2 = +6 dB).
    pub fn drive(mut self, d: impl Into<Param>) -> Self {
        self.dist = self.dist.drive(d);
        self
    }
    /// High roll-off in Hz.
    pub fn tone(mut self, hz: impl Into<Param>) -> Self {
        self.dist = self.dist.tone(hz);
        self
    }
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.dist = self.dist.mix(m);
        self
    }
    pub fn output(mut self, g: impl Into<Param>) -> Self {
        self.dist = self.dist.output(g);
        self
    }
    /// Low-end head bump in dB (default 1.5).
    pub fn bump(mut self, db: f32) -> Self {
        self.bump_db = db;
        self
    }
}

impl Effect for TapeSat {
    fn prepare(&mut self, sr: f32) {
        self.dist.prepare(sr);
        self.bump.set(SvfMode::Peak(self.bump_db), 85.0, 0.8, sr);
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if self.bump_db != 0.0 {
            for f in buf.iter_mut() {
                *f = self.bump.frame(*f);
            }
        }
        self.dist.process(buf, ctx);
    }
    fn reset(&mut self) {
        self.dist.reset();
        self.bump.reset();
    }
    fn tail(&self) -> f32 {
        0.02
    }
    fn name(&self) -> String {
        "tape".into()
    }
}

/// Bit-depth and sample-rate reduction.
///
/// ```ignore
/// Bitcrush::new().bits(8.0).rate(11_025.0)
/// Bitcrush::new().bits(auto![0 => 16.0, bar(8) => 3.0]).mix(0.5)
/// ```
#[derive(Clone, Debug)]
pub struct Bitcrush {
    bits: Param,
    rate: Param,
    mix: Param,
    phase: f32,
    hold: Frame,
    mix_r: Ramp,
    sr: f32,
}

impl Bitcrush {
    /// 8 bits, no rate reduction, fully wet.
    pub fn new() -> Bitcrush {
        Bitcrush {
            bits: Param::Const(8.0),
            rate: Param::Const(0.0),
            mix: Param::Const(1.0),
            phase: 1.0,
            hold: Frame::ZERO,
            mix_r: Ramp::default(),
            sr: 48_000.0,
        }
    }
    /// Bit depth 1–24 (fractional values morph smoothly).
    pub fn bits(mut self, b: impl Into<Param>) -> Self {
        self.bits = b.into();
        self
    }
    /// Target sample rate in Hz (0 = off).
    pub fn rate(mut self, hz: impl Into<Param>) -> Self {
        self.rate = hz.into();
        self
    }
    /// Alias of [`Bitcrush::rate`].
    pub fn downsample(self, hz: impl Into<Param>) -> Self {
        self.rate(hz)
    }
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
}

impl Effect for Bitcrush {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let bits = ctx.p(&self.bits).clamp(1.0, 24.0);
        let steps = 2f32.powf(bits - 1.0);
        let inv = 1.0 / steps;
        let rate = ctx.p(&self.rate);
        let step = if rate <= 0.0 || rate >= self.sr {
            1.0
        } else {
            rate / self.sr
        };
        self.mix_r
            .start(ctx.p(&self.mix).clamp(0.0, 1.0), buf.len());
        for f in buf.iter_mut() {
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.hold = f.map(|x| (x.clamp(-1.0, 1.0) * steps).round() * inv);
            }
            let m = self.mix_r.next();
            *f = *f * (1.0 - m) + self.hold * m;
        }
    }
    fn reset(&mut self) {
        self.phase = 1.0;
        self.hold = Frame::ZERO;
    }
    fn name(&self) -> String {
        "bitcrush".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn run(fx: &mut dyn Effect, hz: f32, amp: f32) -> Vec<Frame> {
        let sr = 48_000.0;
        fx.prepare(sr);
        let ctx = Ctx::offline(sr);
        let mut out = vec![];
        let mut t = 0;
        for _ in 0..200 {
            let mut buf = [Frame::ZERO; 64];
            for f in buf.iter_mut() {
                *f = Frame::mono(amp * (t as f32 * TAU * hz / sr).sin());
                t += 1;
            }
            fx.process(&mut buf, &ctx);
            out.extend_from_slice(&buf);
        }
        out
    }

    #[test]
    fn every_mode_is_bounded() {
        for m in [
            DistMode::Soft,
            DistMode::Hard,
            DistMode::Tube,
            DistMode::Fold,
            DistMode::Fuzz,
            DistMode::Tape,
        ] {
            let out = run(&mut Distortion::new(m).drive(30.0), 220.0, 0.8);
            let pk = out.iter().fold(0f32, |a, f| a.max(f.peak()));
            assert!(pk.is_finite() && pk > 0.05 && pk < 1.5, "{m:?} {pk}");
        }
        let out = run(&mut TapeSat::new(), 100.0, 0.5);
        assert!(out.iter().all(|f| f.l.is_finite()));
    }

    #[test]
    fn dry_path_is_aligned() {
        // mix 0 must reproduce the (delayed) input exactly
        let out = run(&mut Distortion::soft(10.0).mix(0.0), 440.0, 0.5);
        let t = 5000;
        let x = 0.5 * (((t - Os2x::LATENCY) as f32) * TAU * 440.0 / 48_000.0).sin();
        assert!((out[t].l - x).abs() < 1e-5);
    }

    #[test]
    fn bitcrush_quantizes() {
        let out = run(&mut Bitcrush::new().bits(2.0), 100.0, 0.9);
        for f in &out[100..] {
            let v = f.l * 2.0;
            assert!((v - v.round()).abs() < 1e-5, "{}", f.l);
        }
        let out = run(&mut Bitcrush::new().bits(16.0).rate(4_800.0), 100.0, 0.9);
        assert_eq!(out[1000].l, out[1001].l);
    }
}
