//! Modulation effects: [`Chorus`], [`Flanger`], [`Phaser`], [`Tremolo`], [`AutoPan`].
//!
//! Rates take anything convertible to a [`Rate`]: a musical length (`1.bars()`, `eighth()` —
//! locked to the song position) or Hz (`0.5`).

use super::Effect;
use super::util::{LfoPhase, Ring, Smooth, shape_at};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::{Param, Rate, Shape};
use std::f32::consts::PI;

#[inline]
fn mix_ramps(dry: &mut Ramp, wet: &mut Ramp, mix: f32, n: usize) {
    let m = mix.clamp(0.0, 1.0);
    dry.start(1.0 - m, n);
    wet.start(m, n);
}

// ---- Chorus ------------------------------------------------------------------------------------

/// Lush multi-voice stereo chorus.
///
/// ```ignore
/// Chorus::new()                                    // 0.6 Hz, 3 voices, 50% mix
/// Chorus::new().rate(0.3).depth(0.8).voices(4).mix(0.4)
/// Chorus::juno()                                   // classic 2-voice wide ensemble
/// ```
#[derive(Clone, Debug)]
pub struct Chorus {
    lfo: LfoPhase,
    depth: Param,
    delay: Param,
    feedback: Param,
    mix: Param,
    voices: usize,
    spread: f32,
    lines: [Ring; 2],
    fb: [f32; 2],
    sm_delay: Smooth,
    sm_depth: Smooth,
    dry_r: Ramp,
    wet_r: Ramp,
    sr: f32,
}

impl Chorus {
    pub fn new() -> Chorus {
        Chorus {
            lfo: LfoPhase::new(Rate::Hz(0.6)),
            depth: Param::Const(0.5),
            delay: Param::Const(0.012),
            feedback: Param::Const(0.0),
            mix: Param::Const(0.5),
            voices: 3,
            spread: 0.25,
            lines: Default::default(),
            fb: [0.0; 2],
            sm_delay: Smooth::default(),
            sm_depth: Smooth::default(),
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            sr: 48_000.0,
        }
    }
    /// Wide two-voice ensemble in the spirit of the classic 80s poly-synth chorus.
    pub fn juno() -> Chorus {
        Chorus::new()
            .rate(0.5)
            .depth(0.6)
            .voices(2)
            .delay(0.0065)
            .spread(0.5)
            .mix(0.5)
    }
    /// LFO rate (Hz or a musical length).
    pub fn rate(mut self, r: impl Into<Rate>) -> Self {
        self.lfo = LfoPhase::new(r.into());
        self
    }
    /// Modulation depth 0..1 (up to ±4 ms).
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    /// Base delay in seconds (default 12 ms).
    pub fn delay(mut self, secs: impl Into<Param>) -> Self {
        self.delay = secs.into();
        self
    }
    /// Feedback -0.9..0.9.
    pub fn feedback(mut self, fb: impl Into<Param>) -> Self {
        self.feedback = fb.into();
        self
    }
    /// Dry/wet (linear crossfade).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// Voices per channel (1–4).
    pub fn voices(mut self, n: usize) -> Self {
        self.voices = n.clamp(1, 4);
        self
    }
    /// Stereo LFO phase offset between channels (0..1 cycles, default 0.25).
    pub fn spread(mut self, s: f32) -> Self {
        self.spread = s;
        self
    }
}

impl Effect for Chorus {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        for l in &mut self.lines {
            *l = Ring::with_secs(0.06, sr);
        }
        self.sm_delay.time(0.05, sr);
        self.sm_depth.time(0.05, sr);
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if self.lines[0].is_empty() {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        let (ph0, inc) = self.lfo.block(ctx, n);
        self.sm_delay
            .set(ctx.p(&self.delay).clamp(0.001, 0.04) * sr);
        self.sm_depth
            .set(ctx.p(&self.depth).clamp(0.0, 1.0) * 0.004 * sr);
        let fb = ctx.p(&self.feedback).clamp(-0.9, 0.9);
        mix_ramps(&mut self.dry_r, &mut self.wet_r, ctx.p(&self.mix), n);
        let nv = self.voices;
        let vgain = 1.0 / (nv as f32).sqrt();
        for (k, f) in buf.iter_mut().enumerate() {
            let base = self.sm_delay.next();
            let dep = self.sm_depth.next().min(base - 2.0);
            let ph = ph0 + inc * k as f64;
            let x = [f.l, f.r];
            let mut wet = [0f32; 2];
            for ch in 0..2 {
                let mut acc = 0.0;
                for v in 0..nv {
                    let p = ph + v as f64 / nv as f64 + ch as f64 * self.spread as f64;
                    let s = shape_at(Shape::Sine, p, 0);
                    acc += self.lines[ch].read_cubic(base + dep * s);
                }
                wet[ch] = acc * vgain;
                self.lines[ch].push(crate::dsp::undenormal(x[ch] + fb * self.fb[ch]));
                self.fb[ch] = crate::dsp::undenormal(wet[ch]);
            }
            let (d, w) = (self.dry_r.next(), self.wet_r.next());
            *f = Frame::new(x[0] * d + wet[0] * w, x[1] * d + wet[1] * w);
        }
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.fb = [0.0; 2];
        self.lfo.reset();
    }
    fn tail(&self) -> f32 {
        0.1
    }
    fn name(&self) -> String {
        "chorus".into()
    }
}

// ---- Flanger -----------------------------------------------------------------------------------

/// Jet-plane flanger: a short swept delay with feedback.
///
/// ```ignore
/// Flanger::new().rate(0.2).depth(0.8).feedback(0.7)
/// Flanger::new().rate(2.bars()).feedback(-0.6)   // synced sweep, negative feedback = hollow
/// ```
#[derive(Clone, Debug)]
pub struct Flanger {
    lfo: LfoPhase,
    depth: Param,
    delay: Param,
    feedback: Param,
    mix: Param,
    spread: f32,
    lines: [Ring; 2],
    fb: [f32; 2],
    sm: [Smooth; 3],
    dry_r: Ramp,
    wet_r: Ramp,
    sr: f32,
}

impl Flanger {
    pub fn new() -> Flanger {
        Flanger {
            lfo: LfoPhase::new(Rate::Hz(0.2)),
            depth: Param::Const(0.7),
            delay: Param::Const(0.0006),
            feedback: Param::Const(0.6),
            mix: Param::Const(0.5),
            spread: 0.25,
            lines: Default::default(),
            fb: [0.0; 2],
            sm: Default::default(),
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            sr: 48_000.0,
        }
    }
    pub fn rate(mut self, r: impl Into<Rate>) -> Self {
        self.lfo = LfoPhase::new(r.into());
        self
    }
    /// Sweep depth 0..1 (up to +6 ms above the base delay).
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    /// Minimum delay in seconds (default 0.6 ms).
    pub fn delay(mut self, secs: impl Into<Param>) -> Self {
        self.delay = secs.into();
        self
    }
    /// Feedback -0.95..0.95 (negative = hollow tone).
    pub fn feedback(mut self, fb: impl Into<Param>) -> Self {
        self.feedback = fb.into();
        self
    }
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// Stereo LFO offset (cycles).
    pub fn spread(mut self, s: f32) -> Self {
        self.spread = s;
        self
    }
}

impl Effect for Flanger {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        for l in &mut self.lines {
            *l = Ring::with_secs(0.03, sr);
        }
        for s in &mut self.sm {
            s.time(0.03, sr);
        }
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if self.lines[0].is_empty() {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        let (ph0, inc) = self.lfo.block(ctx, n);
        self.sm[0].set(ctx.p(&self.delay).clamp(0.0001, 0.01) * sr);
        self.sm[1].set(ctx.p(&self.depth).clamp(0.0, 1.0) * 0.006 * sr);
        self.sm[2].set(ctx.p(&self.feedback).clamp(-0.95, 0.95));
        mix_ramps(&mut self.dry_r, &mut self.wet_r, ctx.p(&self.mix), n);
        for (k, f) in buf.iter_mut().enumerate() {
            let base = self.sm[0].next();
            let dep = self.sm[1].next();
            let fb = self.sm[2].next();
            let ph = ph0 + inc * k as f64;
            let x = [f.l, f.r];
            let mut wet = [0f32; 2];
            for ch in 0..2 {
                let u = 0.5 + 0.5 * shape_at(Shape::Sine, ph + ch as f64 * self.spread as f64, 0);
                let d = (base + dep * u).max(1.0);
                let y = self.lines[ch].read_cubic(d);
                self.lines[ch].push(crate::dsp::undenormal(x[ch] + fb * y.clamp(-4.0, 4.0)));
                wet[ch] = y;
            }
            self.fb = wet;
            let (d, w) = (self.dry_r.next(), self.wet_r.next());
            *f = Frame::new(x[0] * d + wet[0] * w, x[1] * d + wet[1] * w);
        }
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.lfo.reset();
    }
    fn tail(&self) -> f32 {
        0.2
    }
    fn name(&self) -> String {
        "flanger".into()
    }
}

// ---- Phaser ------------------------------------------------------------------------------------

const MAX_STAGES: usize = 12;

/// Classic swept all-pass phaser (2–12 stages) with feedback.
///
/// ```ignore
/// Phaser::new().rate(0.3).feedback(0.6)
/// Phaser::new().stages(12).range(100.0, 6000.0).rate(4.bars())
/// ```
#[derive(Clone, Debug)]
pub struct Phaser {
    lfo: LfoPhase,
    depth: Param,
    feedback: Param,
    mix: Param,
    lo: Param,
    hi: Param,
    stages: usize,
    spread: f32,
    x1: [[f32; MAX_STAGES]; 2],
    y1: [[f32; MAX_STAGES]; 2],
    last: [f32; 2],
    a: [f32; 2],
    dry_r: Ramp,
    wet_r: Ramp,
    sr: f32,
}

impl Phaser {
    pub fn new() -> Phaser {
        Phaser {
            lfo: LfoPhase::new(Rate::Hz(0.4)),
            depth: Param::Const(1.0),
            feedback: Param::Const(0.5),
            mix: Param::Const(0.5),
            lo: Param::Const(200.0),
            hi: Param::Const(3_500.0),
            stages: 6,
            spread: 0.25,
            x1: [[0.0; MAX_STAGES]; 2],
            y1: [[0.0; MAX_STAGES]; 2],
            last: [0.0; 2],
            a: [0.0; 2],
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            sr: 48_000.0,
        }
    }
    pub fn rate(mut self, r: impl Into<Rate>) -> Self {
        self.lfo = LfoPhase::new(r.into());
        self
    }
    /// Sweep depth 0..1 (fraction of the frequency range).
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    /// Feedback -0.95..0.95 (resonance of the notches).
    pub fn feedback(mut self, fb: impl Into<Param>) -> Self {
        self.feedback = fb.into();
        self
    }
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// Sweep range in Hz.
    pub fn range(mut self, lo: impl Into<Param>, hi: impl Into<Param>) -> Self {
        self.lo = lo.into();
        self.hi = hi.into();
        self
    }
    /// Number of all-pass stages (2–12, even numbers sound classic).
    pub fn stages(mut self, n: usize) -> Self {
        self.stages = n.clamp(1, MAX_STAGES);
        self
    }
    /// Stereo LFO offset (cycles).
    pub fn spread(mut self, s: f32) -> Self {
        self.spread = s;
        self
    }
}

impl Effect for Phaser {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        let n = buf.len();
        let (ph0, inc) = self.lfo.block(ctx, n);
        let lo = ctx.p(&self.lo).clamp(20.0, sr * 0.4);
        let hi = ctx.p(&self.hi).clamp(lo, sr * 0.45);
        let ratio = hi / lo;
        let depth = ctx.p(&self.depth).clamp(0.0, 1.0);
        let fb = ctx.p(&self.feedback).clamp(-0.95, 0.95);
        mix_ramps(&mut self.dry_r, &mut self.wet_r, ctx.p(&self.mix), n);
        let ns = self.stages;
        for (k, f) in buf.iter_mut().enumerate() {
            if k % 4 == 0 {
                let ph = ph0 + inc * k as f64;
                for ch in 0..2 {
                    let s = shape_at(Shape::Sine, ph + ch as f64 * self.spread as f64, 0);
                    let u = 0.5 + 0.5 * depth * s;
                    let fc = lo * ratio.powf(u);
                    let t = (PI * fc / sr).tan();
                    self.a[ch] = (t - 1.0) / (t + 1.0);
                }
            }
            let x = [f.l, f.r];
            let mut wet = [0f32; 2];
            for ch in 0..2 {
                let a = self.a[ch];
                let mut v = x[ch] + fb * self.last[ch];
                for s in 0..ns {
                    let y = a * v + self.x1[ch][s] - a * self.y1[ch][s];
                    self.x1[ch][s] = v;
                    self.y1[ch][s] = crate::dsp::undenormal(y);
                    v = y;
                }
                self.last[ch] = crate::dsp::undenormal(v.clamp(-4.0, 4.0));
                wet[ch] = v;
            }
            let (d, w) = (self.dry_r.next(), self.wet_r.next());
            *f = Frame::new(x[0] * d + wet[0] * w, x[1] * d + wet[1] * w);
        }
    }
    fn reset(&mut self) {
        self.x1 = [[0.0; MAX_STAGES]; 2];
        self.y1 = [[0.0; MAX_STAGES]; 2];
        self.last = [0.0; 2];
        self.lfo.reset();
    }
    fn tail(&self) -> f32 {
        0.1
    }
    fn name(&self) -> String {
        "phaser".into()
    }
}

// ---- Tremolo -----------------------------------------------------------------------------------

/// Volume modulation. Synced rates lock to the beat (great for gated-pad "chop" effects).
///
/// ```ignore
/// Tremolo::new(eighth()).depth(0.6)
/// Tremolo::new(sixteenth()).shape(Shape::Square).depth(1.0)   // trance gate
/// Tremolo::new(5.0).stereo(0.5)                               // 5 Hz, L/R alternating
/// ```
#[derive(Clone, Debug)]
pub struct Tremolo {
    lfo: LfoPhase,
    depth: Param,
    shape: Shape,
    stereo: f32,
    smooth: [f32; 2],
    sr: f32,
}

impl Tremolo {
    pub fn new(rate: impl Into<Rate>) -> Tremolo {
        Tremolo {
            lfo: LfoPhase::new(rate.into()),
            depth: Param::Const(0.5),
            shape: Shape::Sine,
            stereo: 0.0,
            smooth: [1.0; 2],
            sr: 48_000.0,
        }
    }
    /// Modulation depth 0..1 (1 = full chop to silence).
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    /// LFO waveform (sharp edges are de-clicked).
    pub fn shape(mut self, s: Shape) -> Self {
        self.shape = s;
        self
    }
    /// Phase offset of the right channel in cycles (0.5 = alternating L/R).
    pub fn stereo(mut self, offset: f32) -> Self {
        self.stereo = offset;
        self
    }
}

impl Effect for Tremolo {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let (ph0, inc) = self.lfo.block(ctx, buf.len());
        let depth = ctx.p(&self.depth).clamp(0.0, 1.0);
        let c = super::util::coef(0.002, self.sr);
        for (k, f) in buf.iter_mut().enumerate() {
            let ph = ph0 + inc * k as f64;
            let mut g = [0f32; 2];
            for (ch, gc) in g.iter_mut().enumerate() {
                // start each cycle at full level ("chop" feel)
                let s = shape_at(
                    self.shape,
                    ph + ch as f64 * self.stereo as f64
                        + 0.25 * matches!(self.shape, Shape::Sine) as u8 as f64,
                    0,
                );
                let target = 1.0 - depth * (0.5 - 0.5 * s);
                self.smooth[ch] = crate::dsp::undenormal(target + (self.smooth[ch] - target) * c);
                *gc = self.smooth[ch];
            }
            f.l *= g[0];
            f.r *= g[1];
        }
    }
    fn name(&self) -> String {
        "tremolo".into()
    }
}

// ---- AutoPan -----------------------------------------------------------------------------------

/// Sweeps the signal across the stereo field.
///
/// ```ignore
/// AutoPan::new(1.bars()).depth(0.7)
/// AutoPan::new(0.25).shape(Shape::Triangle)
/// ```
#[derive(Clone, Debug)]
pub struct AutoPan {
    lfo: LfoPhase,
    depth: Param,
    shape: Shape,
}

impl AutoPan {
    pub fn new(rate: impl Into<Rate>) -> AutoPan {
        AutoPan {
            lfo: LfoPhase::new(rate.into()),
            depth: Param::Const(0.8),
            shape: Shape::Sine,
        }
    }
    /// How far to swing 0..1.
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    pub fn shape(mut self, s: Shape) -> Self {
        self.shape = s;
        self
    }
}

impl Effect for AutoPan {
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let (ph0, inc) = self.lfo.block(ctx, buf.len());
        let depth = ctx.p(&self.depth).clamp(0.0, 1.0);
        for (k, f) in buf.iter_mut().enumerate() {
            let p = depth * shape_at(self.shape, ph0 + inc * k as f64, 0);
            *f = super::utility::stereo_pan(*f, p);
        }
    }
    fn name(&self) -> String {
        "autopan".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::eighth;
    use std::f32::consts::TAU;

    fn run(fx: &mut dyn Effect, n: usize) -> Vec<Frame> {
        let sr = 48_000.0;
        fx.prepare(sr);
        let mut ctx = Ctx::offline(sr);
        let mut out = vec![];
        let mut t = 0;
        for _ in 0..n / 64 {
            let mut buf = [Frame::ZERO; 64];
            for f in buf.iter_mut() {
                *f = Frame::mono(
                    0.5 * (t as f32 * TAU * 330.0 / sr).sin()
                        + 0.2 * (t as f32 * TAU * 2100.0 / sr).sin(),
                );
                t += 1;
            }
            fx.process(&mut buf, &ctx);
            out.extend_from_slice(&buf);
            ctx = ctx.advanced(64);
        }
        out
    }

    #[test]
    fn all_modulators_are_finite_and_audible() {
        let fxs: Vec<Box<dyn Effect>> = vec![
            Box::new(Chorus::new().feedback(0.5)),
            Box::new(Chorus::juno()),
            Box::new(Flanger::new().feedback(0.95)),
            Box::new(Flanger::new().feedback(-0.95).rate(eighth())),
            Box::new(Phaser::new().stages(12).feedback(0.95)),
            Box::new(Tremolo::new(eighth()).shape(Shape::Square).depth(1.0)),
            Box::new(AutoPan::new(0.5)),
        ];
        for mut fx in fxs {
            let out = run(fx.as_mut(), 48_000);
            let pk = out.iter().fold(0f32, |m, f| m.max(f.peak()));
            assert!(pk.is_finite() && pk > 0.1 && pk < 4.0, "{} {pk}", fx.name());
        }
    }

    #[test]
    fn tremolo_chops_on_the_beat() {
        let mut t = Tremolo::new(eighth()).shape(Shape::Square).depth(1.0);
        let out = run(&mut t, 48_000);
        // eighth at 120 BPM = 12000 samples; first half open, second half closed
        let open = out[1000..5000].iter().fold(0f32, |m, f| m.max(f.peak()));
        let shut = out[7000..11000].iter().fold(0f32, |m, f| m.max(f.peak()));
        assert!(open > 0.5 && shut < 1e-3, "{open} {shut}");
    }
}
