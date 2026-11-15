//! Algorithmic stereo reverb: 4-stage multichannel diffuser feeding an 8×8 modulated feedback
//! delay network (Householder matrix) with in-loop damping and optional shimmer.

use super::Effect;
use super::util::{Ring, Smooth, send_mix};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp, Rng, undenormal};

/// Stereo one-pole low-pass with denormal flushing (input tone filters).
#[derive(Clone, Copy, Debug, Default)]
struct OnePole {
    a: f32,
    y: f32,
}
impl OnePole {
    fn set(&mut self, freq: f32, sr: f32) {
        self.a = 1.0 - (-TAU * freq / sr).exp();
    }
    #[inline]
    fn lp(&mut self, x: f32) -> f32 {
        self.y = undenormal(self.y + self.a * (x - self.y));
        self.y
    }
    #[inline]
    fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
    fn reset(&mut self) {
        self.y = 0.0;
    }
}
use crate::param::Param;
use std::f32::consts::{PI, TAU};

const N: usize = 8;
const STAGES: usize = 4;
const MAX_SIZE: f32 = 2.5;
const MAX_PREDELAY: f32 = 1.0;
const SUB: usize = 16;
/// Diffuser stage spans (ms at size 1).
const DIFF_MS: [f32; STAGES] = [4.5, 9.0, 16.0, 28.0];
/// Feedback delay lengths (ms at size 1), roughly exponentially spread.
const FDN_MS: [f32; N] = [43.7, 51.1, 58.9, 67.3, 76.9, 86.3, 97.1, 109.4];
/// Free-running modulation rates per line (Hz).
const MOD_HZ: [f32; N] = [0.13, 0.21, 0.29, 0.37, 0.47, 0.59, 0.71, 0.83];
const OUT_L: [f32; N] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_R: [f32; N] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const INV_SQRT_N: f32 = 0.353_553_4;

#[inline]
fn hadamard8(v: &mut [f32; N]) {
    let mut h = 1;
    while h < N {
        let mut i = 0;
        while i < N {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += 2 * h;
        }
        h *= 2;
    }
    for x in v.iter_mut() {
        *x *= INV_SQRT_N;
    }
}

/// Granular (two crossfaded taps) pitch shifter used in the shimmer loop.
#[derive(Clone, Debug, Default)]
struct PitchShift {
    buf: Ring,
    phase: f32,
    win: f32,
}
impl PitchShift {
    fn prepare(&mut self, sr: f32) {
        self.win = 0.05 * sr;
        self.buf = Ring::new(self.win as usize + 8);
    }
    #[inline]
    fn tick(&mut self, x: f32, ratio: f32) -> f32 {
        self.buf.push(x);
        self.phase -= (ratio - 1.0) / self.win;
        self.phase = self.phase.rem_euclid(1.0);
        let p2 = (self.phase + 0.5).fract();
        let w1 = (PI * self.phase).sin();
        let w1 = w1 * w1;
        w1 * self.buf.read(self.phase * self.win + 1.0)
            + (1.0 - w1) * self.buf.read(p2 * self.win + 1.0)
    }
}

/// High-quality stereo reverb.
///
/// Knobs (all automatable [`Param`]s): `size` (room scale, 0.1–2.5, 1 = medium hall),
/// `decay` (RT60 in seconds), `damping` (0 bright … 1 dark), `predelay` (seconds),
/// `width` (0 mono … 1 normal … 2 extra wide), `mix`, `modulation` (0–1 chorusing of the
/// tail), `low_cut` / `high_cut` (input filters, Hz), `shimmer` (0–1 octave-up feedback),
/// `early` (early-reflection level).
///
/// `mix` follows a send-style law: dry stays at full level up to 0.5, wet reaches full level at
/// 0.5. On an effect-return bus use [`Reverb::wet`] (100% wet).
///
/// ```ignore
/// let verb = song.bus("verb").fx(Reverb::hall().wet());
/// let pad = song.track("pad", pad).fx(Reverb::plate().mix(0.25).decay(3.5));
/// Reverb::shimmer().decay(8.0).wet()
/// Reverb::room().size(lfo::sine(8.bars()).range(0.3, 0.6))
/// ```
#[derive(Clone, Debug)]
pub struct Reverb {
    size: Param,
    decay: Param,
    damping: Param,
    predelay: Param,
    width: Param,
    mix: Param,
    modulation: Param,
    low_cut: Param,
    high_cut: Param,
    shimmer: Param,
    early: Param,
    seed: u64,
    // runtime
    sr: f32,
    prepared: bool,
    pre: [Ring; 2],
    diff: [[Ring; N]; STAGES],
    diff_ms: [[f32; N]; STAGES],
    diff_flip: [[f32; N]; STAGES],
    diff_perm: [[usize; N]; STAGES],
    fdn: [Ring; N],
    damp: [f32; N],
    rot: [(f32, f32); N],
    shift: [PitchShift; 2],
    in_hp: [OnePole; 2],
    in_lp: [OnePole; 2],
    sm_size: Smooth,
    sm_pre: Smooth,
    dry_r: Ramp,
    wet_r: Ramp,
    width_r: Ramp,
    last_decay: f32,
    last_pre: f32,
    last_shimmer: f32,
}

impl Reverb {
    /// A medium, general-purpose hall (size 1, 2.2 s, 25% mix).
    pub fn new() -> Reverb {
        Reverb {
            size: Param::Const(1.0),
            decay: Param::Const(2.2),
            damping: Param::Const(0.4),
            predelay: Param::Const(0.02),
            width: Param::Const(1.0),
            mix: Param::Const(0.25),
            modulation: Param::Const(0.3),
            low_cut: Param::Const(100.0),
            high_cut: Param::Const(12_000.0),
            shimmer: Param::Const(0.0),
            early: Param::Const(0.3),
            seed: 0x7e7e,
            sr: 48_000.0,
            prepared: false,
            pre: Default::default(),
            diff: Default::default(),
            diff_ms: [[1.0; N]; STAGES],
            diff_flip: [[1.0; N]; STAGES],
            diff_perm: [[0; N]; STAGES],
            fdn: Default::default(),
            damp: [0.0; N],
            rot: [(1.0, 0.0); N],
            shift: Default::default(),
            in_hp: Default::default(),
            in_lp: Default::default(),
            sm_size: Smooth::default(),
            sm_pre: Smooth::default(),
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            width_r: Ramp::default(),
            last_decay: 2.2,
            last_pre: 0.02,
            last_shimmer: 0.0,
        }
    }
    /// Small, lively room.
    pub fn room() -> Reverb {
        Reverb::new()
            .size(0.35)
            .decay(0.7)
            .damping(0.55)
            .predelay(0.004)
            .modulation(0.15)
            .early(0.6)
    }
    /// Wood-panelled chamber: dense, medium decay.
    pub fn chamber() -> Reverb {
        Reverb::new()
            .size(0.6)
            .decay(1.3)
            .damping(0.45)
            .predelay(0.01)
            .modulation(0.2)
            .early(0.45)
    }
    /// Concert hall.
    pub fn hall() -> Reverb {
        Reverb::new()
            .size(1.0)
            .decay(2.8)
            .damping(0.35)
            .predelay(0.025)
            .modulation(0.35)
            .early(0.25)
    }
    /// Bright, dense plate (no early reflections) — vocals and snares.
    pub fn plate() -> Reverb {
        Reverb::new()
            .size(0.55)
            .decay(2.0)
            .damping(0.15)
            .predelay(0.0)
            .modulation(0.5)
            .early(0.0)
            .high_cut(16_000.0)
            .low_cut(150.0)
    }
    /// Huge stone space.
    pub fn cathedral() -> Reverb {
        Reverb::new()
            .size(2.0)
            .decay(6.5)
            .damping(0.45)
            .predelay(0.045)
            .modulation(0.3)
            .early(0.2)
    }
    /// Ethereal octave-up shimmer tail.
    pub fn shimmer() -> Reverb {
        Reverb::new()
            .size(1.6)
            .decay(6.0)
            .damping(0.3)
            .predelay(0.03)
            .modulation(0.5)
            .early(0.1)
            .shimmer_amount(0.35)
            .mix(0.4)
    }

    /// Room scale (0.1–2.5; 1 = medium hall). Changing it glides smoothly.
    pub fn size(mut self, s: impl Into<Param>) -> Self {
        self.size = s.into();
        self
    }
    /// Decay time RT60 (seconds).
    pub fn decay(mut self, secs: impl Into<Param>) -> Self {
        self.decay = secs.into();
        self
    }
    /// High-frequency damping 0 (bright) … 1 (dark).
    pub fn damping(mut self, d: impl Into<Param>) -> Self {
        self.damping = d.into();
        self
    }
    /// Pre-delay before the reverb starts (seconds, ≤ 1).
    pub fn predelay(mut self, secs: impl Into<Param>) -> Self {
        self.predelay = secs.into();
        self
    }
    /// Stereo width of the reverb (0 mono, 1 normal, 2 extra wide).
    pub fn width(mut self, w: impl Into<Param>) -> Self {
        self.width = w.into();
        self
    }
    /// Dry/wet (send-style law, see type docs).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// 100% wet, no dry — for effect-return buses.
    pub fn wet(self) -> Self {
        self.mix(1.0)
    }
    /// Tail modulation depth 0..1 (lush chorusing, less metallic ringing).
    pub fn modulation(mut self, m: impl Into<Param>) -> Self {
        self.modulation = m.into();
        self
    }
    /// High-pass on the reverb input (Hz) — keeps the low end clean.
    pub fn low_cut(mut self, hz: impl Into<Param>) -> Self {
        self.low_cut = hz.into();
        self
    }
    /// Low-pass on the reverb input (Hz).
    pub fn high_cut(mut self, hz: impl Into<Param>) -> Self {
        self.high_cut = hz.into();
        self
    }
    /// Octave-up shimmer amount 0..1.
    pub fn shimmer_amount(mut self, s: impl Into<Param>) -> Self {
        self.shimmer = s.into();
        self
    }
    /// Early-reflection level (0..1).
    pub fn early(mut self, e: impl Into<Param>) -> Self {
        self.early = e.into();
        self
    }
    /// Change the random seed of the diffuser (subtly different character).
    pub fn seed(mut self, s: u64) -> Self {
        self.seed = s;
        self
    }
}

impl Effect for Reverb {
    #[allow(clippy::needless_range_loop)]
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        let ms = sr / 1000.0;
        for p in &mut self.pre {
            *p = Ring::with_secs(MAX_PREDELAY + 0.01, sr);
        }
        let mut rng = Rng::new(self.seed);
        for s in 0..STAGES {
            for i in 0..N {
                let frac = (i as f32 + 0.15 + 0.7 * rng.f32()) / N as f32;
                self.diff_ms[s][i] = 0.3 + DIFF_MS[s] * frac;
                self.diff_flip[s][i] = if rng.chance(0.5) { -1.0 } else { 1.0 };
                self.diff[s][i] = Ring::new((self.diff_ms[s][i] * MAX_SIZE * ms) as usize + 8);
            }
            let mut perm: [usize; N] = std::array::from_fn(|i| i);
            for i in (1..N).rev() {
                perm.swap(i, rng.below(i + 1));
            }
            self.diff_perm[s] = perm;
        }
        for i in 0..N {
            self.fdn[i] = Ring::new((FDN_MS[i] * MAX_SIZE * ms + 0.003 * sr) as usize + 16);
        }
        for s in &mut self.shift {
            s.prepare(sr);
        }
        self.sm_size.time(0.3, sr);
        self.sm_pre.time(0.05, sr);
        self.prepared = true;
        self.reset();
    }

    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if !self.prepared {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        // ---- block-rate parameters
        let decay = ctx.p(&self.decay).clamp(0.05, 120.0);
        let damping = ctx.p(&self.damping).clamp(0.0, 1.0);
        let pre = ctx.p(&self.predelay).clamp(0.0, MAX_PREDELAY);
        let shimmer = ctx.p(&self.shimmer).clamp(0.0, 1.0);
        let early_g = ctx.p(&self.early).clamp(0.0, 2.0);
        let depth = ctx.p(&self.modulation).clamp(0.0, 1.0) * 0.0012 * sr;
        self.last_decay = decay;
        self.last_pre = pre;
        self.last_shimmer = shimmer;
        self.sm_size.set(ctx.p(&self.size).clamp(0.1, MAX_SIZE));
        self.sm_pre.set(pre * sr);
        let (dg, wg) = send_mix(ctx.p(&self.mix));
        self.dry_r.start(dg, n);
        self.wet_r.start(wg, n);
        self.width_r.start(ctx.p(&self.width).clamp(0.0, 2.0), n);
        let lc = ctx.p(&self.low_cut).clamp(10.0, sr * 0.45);
        let hc = ctx.p(&self.high_cut).clamp(200.0, sr * 0.45);
        for ch in 0..2 {
            self.in_hp[ch].set(lc, sr);
            self.in_lp[ch].set(hc, sr);
        }
        let fc = (20_000.0 * 0.05f32.powf(damping)).min(sr * 0.45);
        let damp_a = 1.0 - (-TAU * fc / sr).exp();
        let rot_step: [(f32, f32); N] = std::array::from_fn(|i| {
            let w = TAU * MOD_HZ[i] / sr;
            (w.cos(), w.sin())
        });
        let ms = sr / 1000.0;

        for chunk in buf.chunks_mut(SUB) {
            let size = self.sm_size.advance(chunk.len());
            let flen: [f32; N] = std::array::from_fn(|i| FDN_MS[i] * size * ms);
            let gain: [f32; N] = std::array::from_fn(|i| 10f32.powf(-3.0 * flen[i] / (sr * decay)));
            let gmean = gain.iter().sum::<f32>() / N as f32;
            let late_norm = (1.0 - gmean * gmean).max(1e-4).powf(0.25) * INV_SQRT_N * 1.6;
            let dlen: [[f32; N]; STAGES] = std::array::from_fn(|s| {
                std::array::from_fn(|i| (self.diff_ms[s][i] * size * ms).max(1.0))
            });

            for f in chunk.iter_mut() {
                let dry = *f;
                // pre-delay + input filters
                let pd = self.sm_pre.next();
                let mut xin = [0f32; 2];
                for (ch, x) in [dry.l, dry.r].into_iter().enumerate() {
                    self.pre[ch].push(x);
                    let y = self.pre[ch].read(pd);
                    let y = self.in_hp[ch].hp(y);
                    xin[ch] = self.in_lp[ch].lp(y);
                }
                // diffusion
                let mut v: [f32; N] = std::array::from_fn(|i| xin[i & 1]);
                for s in 0..STAGES {
                    for i in 0..N {
                        let d = &mut self.diff[s][i];
                        d.push(v[i]);
                        v[i] = d.read(dlen[s][i]);
                    }
                    hadamard8(&mut v);
                    let mut w = [0f32; N];
                    for i in 0..N {
                        w[self.diff_perm[s][i]] = v[i] * self.diff_flip[s][i];
                    }
                    v = w;
                }
                // feedback delay network
                let mut y = [0f32; N];
                for i in 0..N {
                    let (c, sn) = self.rot[i];
                    let (cw, sw) = rot_step[i];
                    self.rot[i] = (c * cw - sn * sw, sn * cw + c * sw);
                    let d = flen[i] + depth * (1.0 + sn) + 2.0;
                    let r = self.fdn[i].read_cubic(d);
                    self.damp[i] = undenormal(self.damp[i] + damp_a * (r - self.damp[i]));
                    y[i] = self.damp[i] * gain[i];
                }
                if shimmer > 0.0 {
                    let a = self.shift[0].tick(y[0], 2.0);
                    let b = self.shift[1].tick(y[5], 2.0);
                    y[0] += shimmer * (a - y[0]);
                    y[5] += shimmer * (b - y[5]);
                }
                let (mut ll, mut lr, mut el, mut er) = (0.0, 0.0, 0.0, 0.0);
                let mut sum = 0.0;
                for i in 0..N {
                    ll += y[i] * OUT_L[i];
                    lr += y[i] * OUT_R[i];
                    el += v[i] * OUT_L[i];
                    er += v[i] * OUT_R[i];
                    sum += y[i];
                }
                let hh = sum * (2.0 / N as f32);
                for i in 0..N {
                    self.fdn[i].push(undenormal(y[i] - hh + v[i]));
                }
                let wl = ll * late_norm + el * early_g * INV_SQRT_N * 1.6;
                let wr = lr * late_norm + er * early_g * INV_SQRT_N * 1.6;
                let width = self.width_r.next();
                let m = 0.5 * (wl + wr);
                let sd = 0.5 * (wl - wr) * width;
                let wet = Frame::new(m + sd, m - sd);
                *f = dry * self.dry_r.next() + wet * self.wet_r.next();
            }
        }
        // keep the modulation phasors on the unit circle
        for r in &mut self.rot {
            let k = 1.0 / (r.0 * r.0 + r.1 * r.1).sqrt();
            r.0 *= k;
            r.1 *= k;
        }
    }

    fn reset(&mut self) {
        for p in &mut self.pre {
            p.clear();
        }
        for st in &mut self.diff {
            for d in st.iter_mut() {
                d.clear();
            }
        }
        for d in &mut self.fdn {
            d.clear();
        }
        self.damp = [0.0; N];
        for (i, r) in self.rot.iter_mut().enumerate() {
            let ph = i as f32 * 0.77;
            *r = (ph.cos(), ph.sin());
        }
        for c in 0..2 {
            self.in_hp[c].reset();
            self.in_lp[c].reset();
        }
        self.sm_size.reset();
        self.sm_pre.reset();
    }

    fn tail(&self) -> f32 {
        let decay = self.decay.as_const().unwrap_or(self.last_decay);
        let pre = self.predelay.as_const().unwrap_or(self.last_pre);
        let sh = self
            .shimmer
            .as_const()
            .unwrap_or(self.last_shimmer)
            .clamp(0.0, 1.0);
        pre + decay * 1.4 * (1.0 + 2.0 * sh)
    }

    fn name(&self) -> String {
        "reverb".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_response(mut r: Reverb, secs: f32) -> Vec<Frame> {
        let sr = 48_000.0;
        r.prepare(sr);
        let ctx = Ctx::offline(sr);
        let mut out = vec![];
        let blocks = (secs * sr / 64.0) as usize;
        for b in 0..blocks {
            let mut buf = [Frame::ZERO; 64];
            if b == 0 {
                buf[0] = Frame::new(1.0, 1.0);
            }
            r.process(&mut buf, &ctx);
            out.extend_from_slice(&buf);
        }
        out
    }

    fn energy(v: &[Frame]) -> f32 {
        v.iter().map(|f| f.l * f.l + f.r * f.r).sum()
    }

    #[test]
    fn tail_decays_at_the_requested_rate() {
        let ir = impulse_response(Reverb::hall().decay(2.0).wet().predelay(0.0), 3.0);
        assert!(ir.iter().all(|f| f.l.is_finite() && f.r.is_finite()));
        // energy 0.5..0.6 s vs 1.5..1.6 s: 1 s later should be ~30 dB down for RT60 = 2 s
        let e1 = energy(&ir[24_000..28_800]);
        let e2 = energy(&ir[72_000..76_800]);
        let db = 10.0 * (e2 / e1).log10();
        assert!(db < -20.0 && db > -45.0, "{db}");
        // stereo decorrelation
        let corr: f32 = ir[4800..48_000].iter().map(|f| f.l * f.r).sum::<f32>()
            / energy(&ir[4800..48_000])
            * 2.0;
        assert!(corr.abs() < 0.5, "{corr}");
    }

    #[test]
    fn presets_are_stable() {
        for r in [
            Reverb::room(),
            Reverb::chamber(),
            Reverb::plate(),
            Reverb::cathedral(),
            Reverb::shimmer().shimmer_amount(1.0),
        ] {
            let t = r.tail();
            assert!(t > 0.5 && t < 60.0, "{t}");
            let ir = impulse_response(r.wet(), 4.0);
            let late = ir[180_000..].iter().fold(0f32, |m, f| m.max(f.peak()));
            assert!(late.is_finite() && late < 0.2, "{late}");
        }
    }

    #[test]
    fn dry_passes_at_zero_mix() {
        let ir = impulse_response(Reverb::hall().mix(0.0), 0.1);
        assert!((ir[0].l - 1.0).abs() < 1e-6);
        assert!(ir[100..].iter().all(|f| f.l.abs() < 1e-6));
    }
}
