//! Filters: [`LowPass`], [`HighPass`], [`BandPass`], [`Notch`] (state-variable, cascadable to
//! 12/24/48 dB/oct) and the [`Ladder`] (Moog-style 4-pole nonlinear lowpass).
//!
//! All cutoffs/resonances are [`Param`]s, smoothed per sample in the log-frequency domain, so
//! sweeping them with automation or LFOs is zipper-free:
//!
//! ```ignore
//! LowPass::new(800.0).q(1.2).slope(24)
//! HighPass::new(auto![0 => 20.0, bar(8) => 600.0 => exp])
//! Ladder::new(lfo::sine(2.bars()).range(300.0, 4000.0)).resonance(0.6).drive(2.0)
//! ```

use super::Effect;
use super::util::Smooth;
use crate::ctx::Ctx;
use crate::dsp::Frame;
use crate::param::Param;
use std::f32::consts::PI;

/// Response of an [`SvfStage`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SvfMode {
    LowPass,
    HighPass,
    /// Constant 0 dB peak gain band pass.
    BandPass,
    Notch,
    AllPass,
    /// Bell with gain in dB.
    Peak(f32),
    /// Low shelf with gain in dB.
    LowShelf(f32),
    /// High shelf with gain in dB.
    HighShelf(f32),
}

/// Stereo TPT state-variable filter section (Cytomic/Simper "SvfLinearTrapOptimised2").
/// Coefficients are shared by both channels; stable under fast modulation.
#[derive(Clone, Copy, Debug)]
pub struct SvfStage {
    a1: f32,
    a2: f32,
    a3: f32,
    m0: f32,
    m1: f32,
    m2: f32,
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl Default for SvfStage {
    fn default() -> Self {
        SvfStage {
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            m0: 1.0,
            m1: 0.0,
            m2: 0.0,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
        }
    }
}

impl SvfStage {
    /// Recompute coefficients (keeps state).
    pub fn set(&mut self, mode: SvfMode, freq: f32, q: f32, sr: f32) {
        let f = freq.clamp(10.0, sr * 0.49);
        let mut g = (PI * f / sr).tan();
        let mut k = 1.0 / q.max(0.025);
        let (m0, m1, m2) = match mode {
            SvfMode::LowPass => (0.0, 0.0, 1.0),
            SvfMode::HighPass => (1.0, -k, -1.0),
            SvfMode::BandPass => (0.0, k, 0.0),
            SvfMode::Notch => (1.0, -k, 0.0),
            SvfMode::AllPass => (1.0, -2.0 * k, 0.0),
            SvfMode::Peak(db) => {
                let a = 10f32.powf(db / 40.0);
                k = 1.0 / (q.max(0.025) * a);
                (1.0, k * (a * a - 1.0), 0.0)
            }
            SvfMode::LowShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                g /= a.sqrt();
                (1.0, k * (a - 1.0), a * a - 1.0)
            }
            SvfMode::HighShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                g *= a.sqrt();
                (a * a, k * (1.0 - a) * a, 1.0 - a * a)
            }
        };
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.m0 = m0;
        self.m1 = m1;
        self.m2 = m2;
    }
    /// Process one sample of channel `ch` (0 = left, 1 = right).
    #[inline]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let v3 = x - self.ic2[ch];
        let v1 = self.a1 * self.ic1[ch] + self.a2 * v3;
        let v2 = self.ic2[ch] + self.a2 * self.ic1[ch] + self.a3 * v3;
        self.ic1[ch] = crate::dsp::undenormal(2.0 * v1 - self.ic1[ch]);
        self.ic2[ch] = crate::dsp::undenormal(2.0 * v2 - self.ic2[ch]);
        self.m0 * x + self.m1 * v1 + self.m2 * v2
    }
    #[inline]
    pub fn frame(&mut self, f: Frame) -> Frame {
        Frame::new(self.tick(0, f.l), self.tick(1, f.r))
    }
    pub fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }
}

/// Coefficients are refreshed every SUB samples while parameters move.
pub(crate) const SUB: usize = 8;

/// Butterworth section Qs for a cascade, with the user's resonance applied to the last section.
fn stage_q(stages: usize, i: usize, q: f32) -> f32 {
    let res = q / std::f32::consts::FRAC_1_SQRT_2;
    match stages {
        2 => [0.541_196_1, 1.306_563][i] * if i == 1 { res } else { 1.0 },
        4 => {
            [0.509_795_6, 0.601_344_9, 0.899_976_3, 2.562_915_4][i] * if i == 3 { res } else { 1.0 }
        }
        _ => q,
    }
}

#[derive(Clone, Debug)]
struct FilterCore {
    mode: SvfMode,
    cutoff: Param,
    q: Param,
    stages: usize,
    st: [SvfStage; 4],
    sm_f: Smooth,
    sm_q: Smooth,
    last: (f32, f32),
    smooth_secs: f32,
    sr: f32,
}

impl FilterCore {
    fn new(mode: SvfMode, cutoff: Param) -> FilterCore {
        FilterCore {
            mode,
            cutoff,
            q: Param::Const(std::f32::consts::FRAC_1_SQRT_2),
            stages: 1,
            st: [SvfStage::default(); 4],
            sm_f: Smooth::default(),
            sm_q: Smooth::default(),
            last: (f32::NAN, f32::NAN),
            smooth_secs: 0.01,
            sr: 48_000.0,
        }
    }
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.sm_f.time(self.smooth_secs, sr);
        self.sm_q.time(self.smooth_secs, sr);
        self.sm_f.reset();
        self.sm_q.reset();
        self.last = (f32::NAN, f32::NAN);
    }
    fn reset(&mut self) {
        self.st.iter_mut().for_each(|s| s.reset());
        self.sm_f.reset();
        self.sm_q.reset();
    }
    fn update(&mut self, lf: f32, q: f32) {
        if (lf, q) == self.last {
            return;
        }
        self.last = (lf, q);
        let f = lf.exp2();
        for i in 0..self.stages {
            let sq = match self.mode {
                SvfMode::LowPass | SvfMode::HighPass => stage_q(self.stages, i, q),
                _ => q,
            };
            self.st[i].set(self.mode, f, sq, self.sr);
        }
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        self.sm_f
            .set(ctx.p(&self.cutoff).clamp(10.0, sr * 0.49).log2());
        self.sm_q.set(ctx.p(&self.q).clamp(0.1, 40.0));
        let n = self.stages;
        for chunk in buf.chunks_mut(SUB) {
            let lf = self.sm_f.advance(chunk.len());
            let q = self.sm_q.advance(chunk.len());
            self.update(lf, q);
            for f in chunk.iter_mut() {
                let (mut l, mut r) = (f.l, f.r);
                for s in &mut self.st[..n] {
                    l = s.tick(0, l);
                    r = s.tick(1, r);
                }
                *f = Frame::new(l, r);
            }
        }
    }
}

macro_rules! svf_effect {
    ($name:ident, $mode:expr, $label:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug)]
        pub struct $name(FilterCore);

        impl $name {
            /// New filter at `cutoff` Hz (constant, automation, LFO, lane…). Q defaults to 0.707.
            pub fn new(cutoff: impl Into<Param>) -> Self {
                $name(FilterCore::new($mode, cutoff.into()))
            }
            /// Resonance / Q (0.707 = flat Butterworth; 2-10 = resonant). Automatable.
            pub fn q(mut self, q: impl Into<Param>) -> Self {
                self.0.q = q.into();
                self
            }
            /// Alias of [`Self::q`].
            pub fn res(self, q: impl Into<Param>) -> Self {
                self.q(q)
            }
            /// Change the cutoff.
            pub fn cutoff(mut self, hz: impl Into<Param>) -> Self {
                self.0.cutoff = hz.into();
                self
            }
            /// Steepness in dB/octave: 12, 24 or 48 (cascaded sections).
            pub fn slope(mut self, db: u32) -> Self {
                self.0.stages = match db {
                    0..=12 => 1,
                    13..=24 => 2,
                    _ => 4,
                };
                self
            }
            /// Parameter smoothing time in seconds (default 10 ms).
            pub fn smoothing(mut self, secs: f32) -> Self {
                self.0.smooth_secs = secs.max(0.0);
                self
            }
        }

        impl Effect for $name {
            fn prepare(&mut self, sr: f32) {
                self.0.prepare(sr)
            }
            fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
                self.0.process(buf, ctx)
            }
            fn reset(&mut self) {
                self.0.reset()
            }
            fn tail(&self) -> f32 {
                0.05
            }
            fn name(&self) -> String {
                $label.into()
            }
        }
    };
}

svf_effect!(
    LowPass,
    SvfMode::LowPass,
    "lowpass",
    "Low-pass filter (12/24/48 dB/oct).\n\n```ignore\nLowPass::new(1200.0).q(0.9).slope(24)\nLowPass::new(lfo::sine(4.bars()).range(400.0, 6000.0))\n```"
);
svf_effect!(
    HighPass,
    SvfMode::HighPass,
    "highpass",
    "High-pass filter (12/24/48 dB/oct).\n\n```ignore\nHighPass::new(120.0).slope(24)\nHighPass::new(auto![0 => 20.0, bar(8) => 800.0 => exp]) // riser sweep\n```"
);
svf_effect!(
    BandPass,
    SvfMode::BandPass,
    "bandpass",
    "Band-pass filter with 0 dB peak gain; higher Q = narrower. Slopes cascade identical sections.\n\n```ignore\nBandPass::new(1000.0).q(4.0)\n```"
);
svf_effect!(
    Notch,
    SvfMode::Notch,
    "notch",
    "Notch (band-reject) filter; higher Q = narrower.\n\n```ignore\nNotch::new(lfo::sine(1.bars()).range(300.0, 3000.0)).q(2.0)\n```"
);

/// Moog-style 4-pole (24 dB/oct) ladder lowpass with a nonlinear (tanh) feedback path.
///
/// Zero-delay-feedback model: stable at any cutoff, self-oscillates as `resonance` approaches 1,
/// and `drive` pushes the input into warm saturation.
///
/// ```ignore
/// Ladder::new(800.0).resonance(0.7).drive(2.0)
/// Ladder::new(lfo::saw(1.bars()).range(200.0, 5000.0)).resonance(0.5)
/// ```
#[derive(Clone, Debug)]
pub struct Ladder {
    cutoff: Param,
    resonance: Param,
    drive: Param,
    s: [[f32; 4]; 2],
    sm_f: Smooth,
    sm_k: Smooth,
    sm_d: Smooth,
    sr: f32,
}

impl Ladder {
    pub fn new(cutoff: impl Into<Param>) -> Ladder {
        Ladder {
            cutoff: cutoff.into(),
            resonance: Param::Const(0.3),
            drive: Param::Const(1.0),
            s: [[0.0; 4]; 2],
            sm_f: Smooth::default(),
            sm_k: Smooth::default(),
            sm_d: Smooth::default(),
            sr: 48_000.0,
        }
    }
    /// Resonance 0..1 (≈1 self-oscillates). Automatable.
    pub fn resonance(mut self, r: impl Into<Param>) -> Self {
        self.resonance = r.into();
        self
    }
    /// Alias of [`Ladder::resonance`].
    pub fn res(self, r: impl Into<Param>) -> Self {
        self.resonance(r)
    }
    /// Input drive (linear gain; `2.0` or `6.db()`). Automatable.
    pub fn drive(mut self, d: impl Into<Param>) -> Self {
        self.drive = d.into();
        self
    }
    pub fn cutoff(mut self, hz: impl Into<Param>) -> Self {
        self.cutoff = hz.into();
        self
    }
}

impl Effect for Ladder {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.sm_f.time(0.008, sr);
        self.sm_k.time(0.01, sr);
        self.sm_d.time(0.01, sr);
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        self.sm_f
            .set(ctx.p(&self.cutoff).clamp(10.0, sr * 0.45).log2());
        self.sm_k.set(ctx.p(&self.resonance).clamp(0.0, 1.05) * 4.0);
        self.sm_d.set(ctx.p(&self.drive).clamp(0.01, 100.0));
        for chunk in buf.chunks_mut(SUB) {
            let n = chunk.len();
            let fc = self.sm_f.advance(n).exp2();
            let k = self.sm_k.advance(n);
            let drive = self.sm_d.advance(n);
            let g = (PI * fc / sr).tan();
            let gg = g / (1.0 + g);
            let beta = 1.0 / (1.0 + g);
            let g4 = gg * gg * gg * gg;
            let comp = 1.0 + 0.5 * k; // partial passband-loss compensation
            let out_gain = 1.0 / drive.max(1.0).sqrt();
            for f in chunk.iter_mut() {
                let mut out = [0f32; 2];
                for (ch, x) in [f.l, f.r].into_iter().enumerate() {
                    let s = &mut self.s[ch];
                    let sum = beta * (gg * gg * gg * s[0] + gg * gg * s[1] + gg * s[2] + s[3]);
                    let xin = x * drive * comp;
                    let u = ((xin - k * sum) / (1.0 + k * g4)).tanh();
                    let mut y = u;
                    for st in s.iter_mut() {
                        let v = (y - *st) * gg;
                        let o = v + *st;
                        *st = crate::dsp::undenormal(o + v);
                        y = o;
                    }
                    out[ch] = y * out_gain;
                }
                *f = Frame::new(out[0], out[1]);
            }
        }
    }
    fn reset(&mut self) {
        self.s = [[0.0; 4]; 2];
    }
    fn tail(&self) -> f32 {
        0.1
    }
    fn name(&self) -> String {
        "ladder".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_rms(fx: &mut dyn Effect, hz: f32) -> f32 {
        let sr = 48_000.0;
        fx.prepare(sr);
        let ctx = Ctx::offline(sr);
        let mut acc = 0.0;
        let mut cnt = 0;
        let mut t = 0usize;
        for blk in 0..200 {
            let mut buf = [Frame::ZERO; 64];
            for f in buf.iter_mut() {
                *f = Frame::mono((t as f32 * std::f32::consts::TAU * hz / sr).sin() * 0.5);
                t += 1;
            }
            fx.process(&mut buf, &ctx);
            if blk > 100 {
                for f in &buf {
                    assert!(f.l.is_finite() && f.r.is_finite());
                    acc += f.l * f.l;
                    cnt += 1;
                }
            }
        }
        (acc / cnt as f32).sqrt() / (0.5 / std::f32::consts::SQRT_2)
    }

    #[test]
    fn lowpass_attenuates_highs() {
        assert!(sine_rms(&mut LowPass::new(1000.0), 100.0) > 0.95);
        assert!(sine_rms(&mut LowPass::new(1000.0), 8000.0) < 0.03);
        assert!(sine_rms(&mut LowPass::new(1000.0).slope(24), 8000.0) < 0.002);
        assert!(sine_rms(&mut LowPass::new(1000.0).slope(48), 4000.0) < 0.0005);
    }

    #[test]
    fn highpass_band_notch() {
        assert!(sine_rms(&mut HighPass::new(1000.0).slope(24), 100.0) < 0.001);
        assert!(sine_rms(&mut HighPass::new(1000.0), 10000.0) > 0.95);
        let bp = sine_rms(&mut BandPass::new(1000.0).q(2.0), 1000.0);
        assert!((bp - 1.0).abs() < 0.05, "{bp}");
        assert!(sine_rms(&mut Notch::new(1000.0).q(2.0), 1000.0) < 0.02);
    }

    #[test]
    fn ladder_is_stable_and_filters() {
        assert!(sine_rms(&mut Ladder::new(500.0).resonance(0.0), 8000.0) < 0.01);
        let lo = sine_rms(&mut Ladder::new(2000.0).resonance(0.0), 100.0);
        assert!(lo > 0.7, "{lo}");
        // self-oscillation stays bounded
        let r = sine_rms(&mut Ladder::new(1000.0).resonance(1.05).drive(4.0), 200.0);
        assert!(r.is_finite() && r < 3.0, "{r}");
    }
}
