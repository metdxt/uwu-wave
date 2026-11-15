//! 2–4 operator FM synthesizer.

use super::{AtomicF32, vel_amp};
use crate::ctx::Ctx;
use crate::dsp::{Adsr, Frame, TAU, midi_to_hz};
use crate::instrument::{Instrument, NoteOn, Poly, PolyMode, Voice};
use crate::param::Param;
use crate::units::Val;
use std::sync::Arc;

/// Operator routing. Operators are numbered 1..=4; operator 1 is always a carrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmAlgo {
    /// 4 → 3 → 2 → 1. Feedback on 4.
    Stack,
    /// (2 → 1) + (4 → 3): two carriers, each with its own modulator (DX e-piano). Feedback on 4.
    Pairs,
    /// 2, 3, 4 → 1. Feedback on 4.
    Branch,
    /// 3 → 2 → 1, plus 4 as a separate carrier. Feedback on 3.
    StackPlus,
    /// 4 → 1, 2, 3: one modulator, three carriers (organ / brass). Feedback on 4.
    Fork,
    /// All four are carriers (additive). Feedback on 4.
    Parallel,
}

impl FmAlgo {
    /// (modulation matrix [target][source], carriers, feedback operator)
    fn routing(self) -> ([[bool; 4]; 4], [bool; 4], usize) {
        let mut m = [[false; 4]; 4];
        let mut c = [false; 4];
        let fb;
        match self {
            FmAlgo::Stack => {
                m[0][1] = true;
                m[1][2] = true;
                m[2][3] = true;
                c[0] = true;
                fb = 3;
            }
            FmAlgo::Pairs => {
                m[0][1] = true;
                m[2][3] = true;
                c[0] = true;
                c[2] = true;
                fb = 3;
            }
            FmAlgo::Branch => {
                m[0][1] = true;
                m[0][2] = true;
                m[0][3] = true;
                c[0] = true;
                fb = 3;
            }
            FmAlgo::StackPlus => {
                m[0][1] = true;
                m[1][2] = true;
                c[0] = true;
                c[3] = true;
                fb = 2;
            }
            FmAlgo::Fork => {
                m[0][3] = true;
                m[1][3] = true;
                m[2][3] = true;
                c[0] = true;
                c[1] = true;
                c[2] = true;
                fb = 3;
            }
            FmAlgo::Parallel => {
                c = [true; 4];
                fb = 3;
            }
        }
        (m, c, fb)
    }
}

/// One FM operator: a sine at `ratio` × note frequency (+ `detune` Hz) with its own envelope.
/// For modulators `level` is the modulation index (radians); for carriers it's the output level.
///
/// ```
/// use uwu_wave::prelude::*;
/// let tine = FmOp::new(14.0).level(0.8).adsr(0.0, 0.15, 0.0, 0.1).vel(1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FmOp {
    pub ratio: f32,
    /// Detune in Hz (adds beating).
    pub detune: f32,
    pub level: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Velocity sensitivity 0..1 (on modulators: velocity → brightness).
    pub vel: f32,
    /// Level change per octave above C4 (negative = darker high notes).
    pub key_scale: f32,
}

impl FmOp {
    pub fn new(ratio: f32) -> FmOp {
        FmOp {
            ratio,
            detune: 0.0,
            level: 1.0,
            attack: 0.001,
            decay: 1.0,
            sustain: 1.0,
            release: 0.3,
            vel: 0.6,
            key_scale: 0.0,
        }
    }
    pub fn ratio(mut self, r: f32) -> FmOp {
        self.ratio = r;
        self
    }
    pub fn detune(mut self, hz: f32) -> FmOp {
        self.detune = hz;
        self
    }
    pub fn level(mut self, l: impl Val) -> FmOp {
        self.level = l.val();
        self
    }
    pub fn adsr(mut self, a: f32, d: f32, s: f32, r: f32) -> FmOp {
        self.attack = a;
        self.decay = d;
        self.sustain = s.clamp(0.0, 1.0);
        self.release = r;
        self
    }
    pub fn vel(mut self, sens: f32) -> FmOp {
        self.vel = sens;
        self
    }
    pub fn key_scale(mut self, per_octave: f32) -> FmOp {
        self.key_scale = per_octave;
        self
    }
}

#[derive(Debug)]
struct FmShared {
    index: AtomicF32,
    gain: AtomicF32,
}

/// The voice behind [`Fm`].
#[derive(Clone)]
pub struct FmVoice {
    ops: [FmOp; 4],
    n_ops: usize,
    algo: FmAlgo,
    feedback: f32,
    vel_sens: f32,
    sh: Arc<FmShared>,
    sr: f32,
    env: [Adsr; 4],
    ph: [f32; 4],
    out: [f32; 4],
    lvl: [f32; 4],
    fb_hist: [f32; 2],
    pitch: f32,
    target: f32,
    glide_k: f32,
    amp: f32,
    gain_prev: f32,
    index_prev: f32,
    fresh: bool,
    /// Mod wheel → extra modulation index (fraction of the patch index at full wheel).
    mod_index: f32,
    /// Current (ramped) track pitch bend in semitones.
    bend: f32,
}

impl FmVoice {
    fn op_levels(&mut self, vel: f32) {
        let (_, car, _) = self.algo.routing();
        for i in 0..4 {
            let op = &self.ops[i];
            let vs = 1.0 - op.vel.clamp(0.0, 1.0) * (1.0 - vel);
            let ks = (op.key_scale * (self.target - 60.0) / 12.0).exp2();
            self.lvl[i] = op.level * vs * ks;
            if car[i] {
                self.lvl[i] = op.level * ks;
            }
        }
        self.amp = vel_amp(vel, self.vel_sens);
    }
}

impl Voice for FmVoice {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, ctx: &Ctx) {
        let was_idle = self.env.iter().all(|e| e.is_idle());
        if was_idle {
            self.bend = ctx.bend;
        }
        self.pitch = n.pitch;
        self.target = n.pitch;
        self.glide_k = 0.0;
        self.op_levels(n.vel);
        for i in 0..4 {
            let o = self.ops[i];
            let e = &mut self.env[i];
            e.attack = o.attack;
            e.decay = o.decay;
            e.sustain = o.sustain;
            e.release = o.release;
            e.gate_on(self.sr);
            if was_idle {
                self.ph[i] = 0.0;
                self.out[i] = 0.0;
            }
        }
        if was_idle {
            self.fb_hist = [0.0; 2];
            self.fresh = true;
        }
    }
    fn legato(&mut self, n: &NoteOn, glide: f32, _: &Ctx) {
        self.target = n.pitch;
        self.glide_k = if glide <= 0.0 {
            0.0
        } else {
            (-3.0 / (glide * self.sr)).exp()
        };
        self.op_levels(n.vel);
    }
    fn release(&mut self, _: &Ctx) {
        for e in &mut self.env {
            e.gate_off(self.sr);
        }
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        let n = out.len();
        let (m, car, fbop) = self.algo.routing();
        let nops = self.n_ops.clamp(1, 4);
        // mod wheel → brightness (more modulation index); ramped like the index itself
        let index =
            self.sh.index.get().max(0.0) * (1.0 + self.mod_index * ctx.modwheel.clamp(0.0, 1.0));
        let gain = self.sh.gain.get().max(0.0);
        if self.fresh {
            self.gain_prev = gain;
            self.index_prev = index;
            self.fresh = false;
        }
        let dg = (gain - self.gain_prev) / n.max(1) as f32;
        let di = (index - self.index_prev) / n.max(1) as f32;
        let sr = self.sr;
        let fb = self.feedback;
        let dbend = (ctx.bend - self.bend) / n.max(1) as f32;
        let mut hz = midi_to_hz(self.pitch + self.bend);
        let mut inc = [0.0f32; 4];
        for i in 0..nops {
            inc[i] = (hz * self.ops[i].ratio + self.ops[i].detune) / sr;
        }
        let gliding = self.glide_k > 0.0 && (self.pitch - self.target).abs() > 1e-3;
        let bending = dbend != 0.0;
        for f in out.iter_mut() {
            if gliding || bending {
                if gliding {
                    self.pitch = self.target + (self.pitch - self.target) * self.glide_k;
                }
                self.bend += dbend;
                hz = midi_to_hz(self.pitch + self.bend);
                for i in 0..nops {
                    inc[i] = (hz * self.ops[i].ratio + self.ops[i].detune) / sr;
                }
            }
            self.index_prev += di;
            self.gain_prev += dg;
            let mut sum = 0.0;
            for i in (0..nops).rev() {
                let mut pm = 0.0;
                for j in (i + 1)..nops {
                    if m[i][j] {
                        pm += self.out[j];
                    }
                }
                if i == fbop && fb != 0.0 {
                    pm += fb * 0.5 * (self.fb_hist[0] + self.fb_hist[1]);
                }
                let e = self.env[i].next();
                let lvl = if car[i] {
                    self.lvl[i]
                } else {
                    self.lvl[i] * self.index_prev
                };
                let y = (TAU * self.ph[i] + pm).sin() * e * lvl;
                if i == fbop {
                    self.fb_hist[1] = self.fb_hist[0];
                    self.fb_hist[0] = y / lvl.max(1e-6);
                }
                self.ph[i] += inc[i];
                self.ph[i] -= self.ph[i].floor();
                self.out[i] = y;
                if car[i] {
                    sum += y;
                }
            }
            *f += Frame::mono(sum * self.amp * self.gain_prev);
        }
        (0..nops).any(|i| car[i] && !self.env[i].is_idle())
    }
}

/// 2–4 operator FM synth (DX-style). Pick an [`FmAlgo`], set the operators, done.
///
/// Track pitch bend retunes every operator (ramped per sample); the track mod wheel raises
/// the modulation index (brighter / more bite, see [`Fm::mod_index`]).
///
/// ```
/// use uwu_wave::prelude::*;
/// let ep = Fm::epiano().index(0.8);                 // mellower
/// let wob = Fm::bass().index(lfo::sine(2.beats()).range(0.3, 1.6)); // automatable brightness
/// let custom = Fm::new(FmAlgo::Stack)
///     .op(1, FmOp::new(1.0).adsr(0.001, 1.5, 0.0, 0.3))
///     .op(2, FmOp::new(3.5).level(2.5).adsr(0.001, 0.8, 0.0, 0.3))
///     .ops(2)
///     .feedback(0.2);
/// ```
pub struct Fm {
    poly: Poly<FmVoice>,
    sh: Arc<FmShared>,
    index: Param,
    gain: Param,
}

impl Clone for Fm {
    fn clone(&self) -> Fm {
        let sh = Arc::new(FmShared {
            index: AtomicF32::new(1.0),
            gain: AtomicF32::new(0.3),
        });
        let mut poly = self.poly.clone();
        poly.panic();
        poly.proto_mut().sh = sh.clone();
        Fm {
            poly,
            sh,
            index: self.index.clone(),
            gain: self.gain.clone(),
        }
    }
}

impl Fm {
    /// Empty patch: 4 operators with ratio 1, operator 1 at full level, the rest silent.
    pub fn new(algo: FmAlgo) -> Fm {
        let sh = Arc::new(FmShared {
            index: AtomicF32::new(1.0),
            gain: AtomicF32::new(0.3),
        });
        let mut ops = [FmOp::new(1.0).level(0.0); 4];
        ops[0].level = 1.0;
        let v = FmVoice {
            ops,
            n_ops: 4,
            algo,
            feedback: 0.0,
            vel_sens: 0.7,
            sh: sh.clone(),
            sr: 48_000.0,
            env: [Adsr::default(); 4],
            ph: [0.0; 4],
            out: [0.0; 4],
            lvl: [0.0; 4],
            fb_hist: [0.0; 2],
            pitch: 60.0,
            target: 60.0,
            glide_k: 0.0,
            amp: 1.0,
            gain_prev: 0.3,
            index_prev: 1.0,
            fresh: true,
            mod_index: 1.0,
            bend: 0.0,
        };
        Fm {
            poly: Poly::new(v).named("fm"),
            sh,
            index: Param::Const(1.0),
            gain: Param::Const(0.3),
        }
    }
    fn v(&mut self) -> &mut FmVoice {
        self.poly.proto_mut()
    }
    /// Set operator `n` (1..=4).
    pub fn op(mut self, n: usize, op: FmOp) -> Fm {
        let i = n.clamp(1, 4) - 1;
        self.v().ops[i] = op;
        self
    }
    /// Change one operator in place: `.edit(2, |o| o.level(3.0))`.
    pub fn edit(mut self, n: usize, f: impl FnOnce(FmOp) -> FmOp) -> Fm {
        let i = n.clamp(1, 4) - 1;
        let o = self.v().ops[i];
        self.v().ops[i] = f(o);
        self
    }
    /// Number of active operators (2..=4); higher operators are ignored.
    pub fn ops(mut self, n: usize) -> Fm {
        self.v().n_ops = n.clamp(1, 4);
        self
    }
    pub fn algo(mut self, a: FmAlgo) -> Fm {
        self.v().algo = a;
        self
    }
    /// Self-feedback of the top operator (0..~1.5; higher = buzzier).
    pub fn feedback(mut self, fb: f32) -> Fm {
        self.v().feedback = fb;
        self
    }
    /// Global modulation-index multiplier (brightness, automatable). Default 1.
    pub fn index(mut self, k: impl Into<Param>) -> Fm {
        self.index = k.into();
        self
    }
    /// Mod wheel → brightness: at full wheel the modulation index is multiplied by
    /// `1 + amount` (default 1.0, i.e. doubled). 0 ignores the wheel.
    pub fn mod_index(mut self, amount: f32) -> Fm {
        self.v().mod_index = amount;
        self
    }
    /// Output gain (automatable).
    pub fn gain(mut self, g: impl Into<Param>) -> Fm {
        self.gain = g.into();
        self
    }
    /// Velocity → level sensitivity (0..1).
    pub fn velocity(mut self, sens: f32) -> Fm {
        self.v().vel_sens = sens;
        self
    }
    pub fn voices(mut self, n: usize) -> Fm {
        self.poly.max = n.max(1);
        self
    }
    pub fn mono(mut self) -> Fm {
        self.poly.mode = PolyMode::Mono;
        self
    }
    pub fn legato(mut self, glide: f32) -> Fm {
        self.poly.mode = PolyMode::Legato;
        self.poly.glide = glide;
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> Fm {
        self.poly = self.poly.named(name);
        self
    }

    // ---- presets -----------------------------------------------------------------------------

    /// DX-style electric piano: warm body + bell-like tine.
    pub fn epiano() -> Fm {
        Fm::new(FmAlgo::Pairs)
            .named("epiano")
            .op(
                1,
                FmOp::new(1.0)
                    .level(0.75)
                    .adsr(0.001, 3.2, 0.0, 0.45)
                    .key_scale(-0.1),
            )
            .op(
                2,
                FmOp::new(1.0)
                    .level(1.1)
                    .adsr(0.001, 1.6, 0.0, 0.4)
                    .vel(0.8)
                    .key_scale(-0.25),
            )
            .op(
                3,
                FmOp::new(1.0)
                    .detune(0.7)
                    .level(0.35)
                    .adsr(0.001, 1.8, 0.0, 0.4),
            )
            .op(
                4,
                FmOp::new(14.0)
                    .level(0.9)
                    .adsr(0.0, 0.14, 0.0, 0.1)
                    .vel(1.0)
                    .key_scale(-0.4),
            )
            .velocity(0.75)
            .gain(0.38)
    }
    /// Glassy, long bell.
    pub fn bell() -> Fm {
        Fm::new(FmAlgo::Pairs)
            .named("bell")
            .op(1, FmOp::new(1.0).level(0.7).adsr(0.001, 5.0, 0.0, 2.5))
            .op(
                2,
                FmOp::new(3.5)
                    .level(2.4)
                    .adsr(0.001, 3.5, 0.0, 2.0)
                    .vel(0.7),
            )
            .op(
                3,
                FmOp::new(2.0)
                    .detune(1.3)
                    .level(0.3)
                    .adsr(0.001, 2.5, 0.0, 1.5),
            )
            .op(4, FmOp::new(7.13).level(1.4).adsr(0.001, 1.2, 0.0, 1.0))
            .velocity(0.6)
            .gain(0.3)
    }
    /// Punchy FM bass (mono).
    pub fn bass() -> Fm {
        Fm::new(FmAlgo::Stack)
            .named("fm bass")
            .op(1, FmOp::new(1.0).level(1.0).adsr(0.001, 0.8, 0.6, 0.06))
            .op(
                2,
                FmOp::new(1.0)
                    .level(2.6)
                    .adsr(0.001, 0.22, 0.25, 0.08)
                    .vel(0.7),
            )
            .op(3, FmOp::new(2.0).level(0.6).adsr(0.001, 0.08, 0.0, 0.05))
            .ops(3)
            .feedback(0.0)
            .velocity(0.5)
            .mono()
            .gain(0.42)
    }
    /// Inharmonic metallic hit.
    pub fn metallic() -> Fm {
        Fm::new(FmAlgo::Branch)
            .named("metallic")
            .op(1, FmOp::new(1.0).level(0.8).adsr(0.001, 1.8, 0.0, 0.8))
            .op(2, FmOp::new(1.414).level(2.0).adsr(0.001, 1.2, 0.0, 0.6))
            .op(3, FmOp::new(2.76).level(1.4).adsr(0.001, 0.7, 0.0, 0.5))
            .op(4, FmOp::new(5.19).level(1.0).adsr(0.001, 0.4, 0.0, 0.3))
            .feedback(0.3)
            .gain(0.28)
    }
    /// Woody marimba.
    pub fn marimba() -> Fm {
        Fm::new(FmAlgo::Pairs)
            .named("marimba")
            .op(
                1,
                FmOp::new(1.0)
                    .level(0.85)
                    .adsr(0.001, 0.7, 0.0, 0.25)
                    .key_scale(-0.3),
            )
            .op(
                2,
                FmOp::new(4.0)
                    .level(1.7)
                    .adsr(0.0, 0.045, 0.0, 0.04)
                    .vel(0.9),
            )
            .op(3, FmOp::new(4.0).level(0.12).adsr(0.0, 0.12, 0.0, 0.08))
            .op(4, FmOp::new(10.0).level(0.7).adsr(0.0, 0.02, 0.0, 0.02))
            .velocity(0.8)
            .gain(0.4)
    }
}

impl Instrument for Fm {
    fn prepare(&mut self, sr: f32) {
        self.poly.prepare(sr);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.update(ctx);
        self.poly.note_on(note, ctx);
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        self.poly.note_off(id, ctx);
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        self.update(ctx);
        self.poly.render(out, ctx);
    }
    fn active(&self) -> bool {
        self.poly.active()
    }
    fn panic(&mut self) {
        self.poly.panic();
    }
    fn name(&self) -> String {
        self.poly.name()
    }
}

impl Fm {
    fn update(&self, ctx: &Ctx) {
        self.sh.index.set(ctx.p(&self.index));
        self.sh.gain.set(ctx.p(&self.gain));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::test_util::*;

    #[test]
    fn presets_render_and_end() {
        for mut s in [
            Fm::epiano(),
            Fm::bell(),
            Fm::bass(),
            Fm::metallic(),
            Fm::marimba(),
        ] {
            let name = s.name();
            let out = play(&mut s, 60.0, 0.9, 0.4, 9.0);
            assert!(finite(&out), "{name}");
            let p = peak(&out);
            assert!(p > 0.05 && p < 1.5, "{name}: {p}");
            assert!(!s.active(), "{name} never ended");
        }
    }

    #[test]
    fn index_changes_brightness() {
        // more modulation → more high-frequency energy (measure by first-difference rms)
        let hf = |k: f32| {
            let out = play(&mut Fm::bell().index(k), 60.0, 1.0, 0.3, 0.3);
            let d: Vec<_> = out.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&d)
        };
        assert!(hf(1.5) > hf(0.1) * 1.5);
    }

    fn render_ctx(f: &mut Fm, bend: f32, mw: f32) -> Vec<Frame> {
        let sr = 48_000.0;
        f.prepare(sr);
        let mut ctx = Ctx::offline(sr);
        ctx.bend = bend;
        ctx.modwheel = mw;
        f.note_on(
            &NoteOn {
                id: 1,
                pitch: 57.0,
                vel: 1.0,
                dur: 1.0,
            },
            &ctx,
        );
        let mut out = vec![Frame::ZERO; 24_000];
        for c in out.chunks_mut(64) {
            f.render(c, &ctx);
            ctx = ctx.advanced(c.len());
        }
        out
    }

    #[test]
    fn bend_and_modwheel() {
        // a pure carrier: bend +12 doubles the frequency
        let sine = || {
            Fm::new(FmAlgo::Stack)
                .ops(1)
                .op(1, FmOp::new(1.0).adsr(0.001, 1.0, 1.0, 0.1))
        };
        let zc = |o: &[Frame]| {
            o[4800..]
                .windows(2)
                .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
                .count() as f32
        };
        let ratio =
            zc(&render_ctx(&mut sine(), 12.0, 0.0)) / zc(&render_ctx(&mut sine(), 0.0, 0.0));
        assert!((ratio - 2.0).abs() < 0.02, "{ratio}");
        // mod wheel makes it brighter
        let hf = |o: Vec<Frame>| {
            let d: Vec<_> = o.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&d) / rms(&o)
        };
        assert!(
            hf(render_ctx(&mut Fm::epiano(), 0.0, 1.0))
                > hf(render_ctx(&mut Fm::epiano(), 0.0, 0.0)) * 1.1
        );
    }

    #[test]
    fn all_algorithms() {
        for a in [
            FmAlgo::Stack,
            FmAlgo::Pairs,
            FmAlgo::Branch,
            FmAlgo::StackPlus,
            FmAlgo::Fork,
            FmAlgo::Parallel,
        ] {
            let mut f = Fm::new(a)
                .op(2, FmOp::new(2.0).level(1.0))
                .op(3, FmOp::new(3.0).level(0.5))
                .op(4, FmOp::new(1.0).level(0.5))
                .feedback(0.5);
            let out = play(&mut f, 57.0, 1.0, 0.2, 1.0);
            assert!(finite(&out) && peak(&out) > 0.01, "{a:?}");
        }
    }
}
