//! Subtractive / virtual-analog synthesizer.

use super::{AtomicF32, vel_amp};
use crate::ctx::Ctx;
use crate::dsp::{Adsr, Frame, Osc, Rng, Svf, TAU, midi_to_hz, pan_gains, soft_clip};
use crate::instrument::{Instrument, NoteOn, Poly, PolyMode, Voice};
use crate::param::Param;
use crate::units::Val;
use std::f32::consts::PI;
use std::sync::Arc;

/// Maximum oscillators per voice.
pub const MAX_OSCS: usize = 3;
/// Maximum unison (supersaw) copies per oscillator.
pub const MAX_UNISON: usize = 8;
/// Filter coefficients are recomputed every `CTRL` samples.
const CTRL: usize = 8;

/// One oscillator of a [`Synth`]: waveform, tuning and level.
///
/// ```
/// use uwu_wave::prelude::*;
/// let s = Synth::new()
///     .osc(OscSpec::saw())
///     .osc(OscSpec::square().oct(-1).level(0.6))
///     .osc(OscSpec::pulse(0.3).fine(7.0).pwm(0.2, 0.5));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscSpec {
    pub wave: Wave,
    /// Octave offset.
    pub octave: i32,
    /// Semitone offset (fractional allowed).
    pub semi: f32,
    /// Fine detune in cents.
    pub fine: f32,
    /// Linear level.
    pub level: f32,
    /// Pulse-width modulation depth (0..0.45) — only for `Wave::Pulse`/`Wave::Square`.
    pub pwm: f32,
    /// PWM LFO rate in Hz.
    pub pwm_rate: f32,
}

pub use crate::dsp::Wave;

impl OscSpec {
    pub fn new(wave: Wave) -> OscSpec {
        OscSpec {
            wave,
            octave: 0,
            semi: 0.0,
            fine: 0.0,
            level: 1.0,
            pwm: 0.0,
            pwm_rate: 0.7,
        }
    }
    pub fn saw() -> OscSpec {
        OscSpec::new(Wave::Saw)
    }
    pub fn square() -> OscSpec {
        OscSpec::new(Wave::Square)
    }
    /// Pulse with duty cycle `width` (0..1).
    pub fn pulse(width: f32) -> OscSpec {
        OscSpec::new(Wave::Pulse(width))
    }
    pub fn sine() -> OscSpec {
        OscSpec::new(Wave::Sine)
    }
    pub fn tri() -> OscSpec {
        OscSpec::new(Wave::Triangle)
    }
    pub fn noise() -> OscSpec {
        OscSpec::new(Wave::Noise)
    }
    /// Octave offset (`-1` = one octave down).
    pub fn oct(mut self, o: i32) -> OscSpec {
        self.octave = o;
        self
    }
    /// Semitone offset (`7.0` = a fifth up).
    pub fn semi(mut self, st: f32) -> OscSpec {
        self.semi = st;
        self
    }
    /// Fine detune in cents.
    pub fn fine(mut self, cents: f32) -> OscSpec {
        self.fine = cents;
        self
    }
    /// Level (linear, or `-6.db()`).
    pub fn level(mut self, l: impl Val) -> OscSpec {
        self.level = l.val();
        self
    }
    /// Pulse-width modulation: `depth` (0..0.45) at `rate` Hz. Turns a square into a pulse.
    pub fn pwm(mut self, depth: f32, rate: f32) -> OscSpec {
        if self.wave == Wave::Square {
            self.wave = Wave::Pulse(0.5);
        }
        self.pwm = depth.clamp(0.0, 0.45);
        self.pwm_rate = rate;
        self
    }
    fn semis(&self) -> f32 {
        self.octave as f32 * 12.0 + self.semi + self.fine / 100.0
    }
}

impl From<Wave> for OscSpec {
    fn from(w: Wave) -> OscSpec {
        OscSpec::new(w)
    }
}

/// Filter type of a [`Synth`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vcf {
    /// Bypass.
    Off,
    /// 12 dB/oct state-variable lowpass.
    Lp,
    /// 24 dB/oct ladder lowpass with saturating input (squelchy, acid-ready).
    Lp24,
    /// 12 dB/oct highpass.
    Hp,
    /// Bandpass.
    Bp,
}

/// Static configuration shared by every voice (copied into voices).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SynthCfg {
    oscs: [OscSpec; MAX_OSCS],
    n_osc: usize,
    unison: usize,
    detune: f32,
    spread: f32,
    sub: f32,
    sub_wave: Wave,
    noise: f32,
    vcf: Vcf,
    key_track: f32,
    fenv: [f32; 4],
    fenv_amt: f32,
    vel_cutoff: f32,
    amp: [f32; 4],
    vel_sens: f32,
    pitch_amt: f32,
    pitch_decay: f32,
    vib_rate: f32,
    vib_depth: f32,
    vib_delay: f32,
    accent: f32,
    retrig: bool,
}

impl Default for SynthCfg {
    fn default() -> SynthCfg {
        let mut oscs = [OscSpec::saw(); MAX_OSCS];
        oscs[1].level = 0.0;
        oscs[2].level = 0.0;
        SynthCfg {
            oscs,
            n_osc: 1,
            unison: 1,
            detune: 0.0,
            spread: 0.0,
            sub: 0.0,
            sub_wave: Wave::Sine,
            noise: 0.0,
            vcf: Vcf::Lp,
            key_track: 0.0,
            fenv: [0.001, 0.3, 0.0, 0.3],
            fenv_amt: 0.0,
            vel_cutoff: 0.0,
            amp: [0.004, 0.2, 0.8, 0.15],
            vel_sens: 0.7,
            pitch_amt: 0.0,
            pitch_decay: 0.05,
            vib_rate: 5.5,
            vib_depth: 0.0,
            vib_delay: 0.0,
            accent: 0.0,
            retrig: false,
        }
    }
}

/// Block-rate values computed by the instrument and read by every voice.
#[derive(Debug)]
pub(crate) struct SynthShared {
    cutoff: AtomicF32,
    res: AtomicF32,
    drive: AtomicF32,
    gain: AtomicF32,
}

impl SynthShared {
    fn new() -> Arc<SynthShared> {
        Arc::new(SynthShared {
            cutoff: AtomicF32::new(8000.0),
            res: AtomicF32::new(0.1),
            drive: AtomicF32::new(0.0),
            gain: AtomicF32::new(0.3),
        })
    }
}

/// 4-pole zero-delay-feedback ladder lowpass with a saturating input stage.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ladder {
    s: [f32; 4],
    g: f32,
    b: f32,
    k: f32,
}

impl Ladder {
    /// `res` 0..1 (1 = self-oscillation).
    #[inline]
    pub(crate) fn set(&mut self, cut: f32, res: f32, sr: f32) {
        let g = (PI * cut.clamp(10.0, sr * 0.45) / sr).tan();
        self.g = g / (1.0 + g);
        self.b = 1.0 / (1.0 + g);
        self.k = res.clamp(0.0, 1.0) * 3.98;
    }
    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> f32 {
        let (g, b, k) = (self.g, self.b, self.k);
        let g2 = g * g;
        let fb = g2 * g * b * self.s[0] + g2 * b * self.s[1] + g * b * self.s[2] + b * self.s[3];
        let mut u = (x * (1.0 + 0.5 * k) - k * fb) / (1.0 + k * g2 * g2);
        u = soft_clip(u);
        for s in &mut self.s {
            let v = (u - *s) * g;
            let y = v + *s;
            *s = y + v;
            u = y;
        }
        u
    }
}

#[inline]
fn res_to_q(res: f32) -> f32 {
    0.707 * 30f32.powf(res.clamp(0.0, 1.0))
}

#[inline]
fn set_times(env: &mut Adsr, t: [f32; 4]) {
    env.attack = t[0];
    env.decay = t[1];
    env.sustain = t[2];
    env.release = t[3];
}

/// The voice behind [`Synth`]. Usually you don't touch it directly.
#[derive(Clone)]
pub struct SynthVoice {
    cfg: SynthCfg,
    sh: Arc<SynthShared>,
    sr: f32,
    oscs: [[Osc; MAX_UNISON]; MAX_OSCS],
    ratio: [[f32; MAX_UNISON]; MAX_OSCS],
    ugl: [f32; MAX_UNISON],
    ugr: [f32; MAX_UNISON],
    sub: Osc,
    rng: Rng,
    aenv: Adsr,
    fenv: Adsr,
    pitch: f32,
    target: f32,
    glide_k: f32,
    penv: f32,
    penv_k: f32,
    vib_ph: f32,
    vib_amt: f32,
    vib_inc: f32,
    pwm_ph: [f32; MAX_OSCS],
    vel: f32,
    vel_gain: f32,
    vel_gain_s: f32,
    accent_on: bool,
    svf: [Svf; 2],
    ladder: [Ladder; 2],
    cut_log: f32,
    gain_prev: f32,
    fresh: bool,
    ctrl_left: usize,
    fenv_val: f32,
}

impl SynthVoice {
    fn new(sh: Arc<SynthShared>) -> SynthVoice {
        SynthVoice {
            cfg: SynthCfg::default(),
            sh,
            sr: 48_000.0,
            oscs: std::array::from_fn(|_| std::array::from_fn(|_| Osc::new(Wave::Saw))),
            ratio: [[1.0; MAX_UNISON]; MAX_OSCS],
            ugl: [1.0; MAX_UNISON],
            ugr: [1.0; MAX_UNISON],
            sub: Osc::new(Wave::Sine),
            rng: Rng::new(7),
            aenv: Adsr::default(),
            fenv: Adsr::default(),
            pitch: 60.0,
            target: 60.0,
            glide_k: 0.0,
            penv: 0.0,
            penv_k: 0.0,
            vib_ph: 0.0,
            vib_amt: 0.0,
            vib_inc: 0.0,
            pwm_ph: [0.0; MAX_OSCS],
            vel: 1.0,
            vel_gain: 1.0,
            vel_gain_s: 1.0,
            accent_on: false,
            svf: [Svf::default(); 2],
            ladder: [Ladder::default(); 2],
            cut_log: 13.0,
            gain_prev: 0.0,
            fresh: true,
            ctrl_left: 0,
            fenv_val: 0.0,
        }
    }

    fn setup_unison(&mut self) {
        let n = self.cfg.unison.clamp(1, MAX_UNISON);
        let norm = 1.0 / (n as f32).sqrt();
        for u in 0..n {
            let x = if n == 1 {
                0.0
            } else {
                -1.0 + 2.0 * u as f32 / (n - 1) as f32
            };
            let (gl, gr) = pan_gains(x * self.cfg.spread);
            self.ugl[u] = gl * norm;
            self.ugr[u] = gr * norm;
            for o in 0..self.cfg.n_osc {
                let cents = x * self.cfg.detune;
                self.ratio[o][u] = 2f32.powf((self.cfg.oscs[o].semis() + cents / 100.0) / 12.0);
            }
        }
    }

    /// Advance glide / pitch envelope / vibrato by one sample and return the frequency.
    #[inline]
    fn next_hz(&mut self, cfg: &SynthCfg, vib_dt: f32) -> f32 {
        self.pitch = self.target + (self.pitch - self.target) * self.glide_k;
        let mut m = self.pitch;
        if cfg.pitch_amt != 0.0 {
            m += self.penv * cfg.pitch_amt;
            self.penv *= self.penv_k;
        }
        if cfg.vib_depth > 0.0 {
            self.vib_amt = (self.vib_amt + self.vib_inc).min(1.0);
            m += (TAU * self.vib_ph).sin() * cfg.vib_depth * self.vib_amt;
            self.vib_ph += vib_dt;
            if self.vib_ph >= 1.0 {
                self.vib_ph -= 1.0;
            }
        }
        midi_to_hz(m)
    }

    fn set_velocity(&mut self, vel: f32) {
        self.vel = vel.clamp(0.0, 1.0);
        self.accent_on = self.cfg.accent > 0.0 && vel >= 0.85;
        let acc = if self.accent_on {
            1.0 + 0.7 * self.cfg.accent
        } else {
            1.0
        };
        self.vel_gain = vel_amp(vel, self.cfg.vel_sens) * acc;
    }
}

impl Voice for SynthVoice {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }

    fn start(&mut self, note: &NoteOn, _ctx: &Ctx) {
        let sr = self.sr;
        let cfg = self.cfg;
        let was_idle = self.aenv.is_idle();
        self.rng = Rng::new(
            note.id
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add(note.pitch.to_bits() as u64),
        );
        self.setup_unison();
        let n = cfg.unison.clamp(1, MAX_UNISON);
        for o in 0..cfg.n_osc {
            for u in 0..n {
                let osc = &mut self.oscs[o][u];
                osc.wave = cfg.oscs[o].wave;
                if was_idle || cfg.retrig {
                    osc.phase = if cfg.retrig && n == 1 {
                        0.0
                    } else {
                        self.rng.f32()
                    };
                }
                *osc = osc.clone().seed(self.rng.next_u64());
            }
            if was_idle {
                self.pwm_ph[o] = self.rng.f32();
            }
        }
        self.sub.wave = cfg.sub_wave;
        if was_idle || cfg.retrig {
            self.sub.phase = 0.0;
            self.svf = [Svf::default(); 2];
            self.ladder = [Ladder::default(); 2];
            self.fresh = true;
            self.ctrl_left = 0;
            self.vib_ph = 0.0;
        }
        self.pitch = note.pitch;
        self.target = note.pitch;
        self.glide_k = 0.0;
        self.penv = 1.0;
        self.penv_k = if cfg.pitch_decay <= 0.0 {
            0.0
        } else {
            (-3.0 / (cfg.pitch_decay * sr)).exp()
        };
        self.vib_amt = if cfg.vib_delay > 0.0 { 0.0 } else { 1.0 };
        self.vib_inc = if cfg.vib_delay > 0.0 {
            1.0 / (cfg.vib_delay * sr)
        } else {
            1.0
        };
        self.set_velocity(note.vel);
        if was_idle {
            self.vel_gain_s = self.vel_gain;
        }
        set_times(&mut self.aenv, cfg.amp);
        let mut f = cfg.fenv;
        if self.accent_on {
            f[1] *= 0.6;
        }
        set_times(&mut self.fenv, f);
        self.aenv.gate_on(sr);
        self.fenv.gate_on(sr);
    }

    fn legato(&mut self, note: &NoteOn, glide: f32, _ctx: &Ctx) {
        self.target = note.pitch;
        // ≈ 95 % of the way after `glide` seconds
        self.glide_k = if glide <= 0.0 {
            0.0
        } else {
            (-3.0 / (glide * self.sr)).exp()
        };
        self.set_velocity(note.vel);
        if self.aenv.is_idle() {
            self.aenv.gate_on(self.sr);
        }
    }

    fn release(&mut self, _ctx: &Ctx) {
        self.aenv.gate_off(self.sr);
        self.fenv.gate_off(self.sr);
    }

    fn render(&mut self, out: &mut [Frame], _ctx: &Ctx) -> bool {
        let n = out.len();
        if n == 0 {
            return !self.aenv.is_idle();
        }
        let sr = self.sr;
        let cfg = self.cfg;
        let cut_target = self.sh.cutoff.get().clamp(10.0, 24_000.0).log2();
        let gain_target = self.sh.gain.get().max(0.0);
        let res = self.sh.res.get().clamp(0.0, 1.0);
        let drive = self.sh.drive.get().max(0.0);
        if self.fresh {
            self.cut_log = cut_target;
            self.gain_prev = gain_target;
            self.fresh = false;
        }
        let dcut = (cut_target - self.cut_log) / n as f32;
        let dgain = (gain_target - self.gain_prev) / n as f32;
        let q = res_to_q(res);
        let drv = 1.0 + 3.0 * drive;
        let drv_norm = 1.0 / (1.0 + drive).sqrt();
        let uni = cfg.unison.clamp(1, MAX_UNISON);
        let kt = cfg.key_track * (self.target - 60.0) / 12.0;
        let velc = cfg.vel_cutoff * (self.vel - 1.0);
        let env_amt = cfg.fenv_amt
            + if self.accent_on {
                2.0 * cfg.accent
            } else {
                0.0
            };
        let vib_dt = cfg.vib_rate / sr;
        let mut pwm_dt = [0.0f32; MAX_OSCS];
        for o in 0..cfg.n_osc {
            pwm_dt[o] = cfg.oscs[o].pwm_rate / sr;
        }

        // pitch is static for the whole block unless something modulates it
        let settled = (self.pitch - self.target).abs() < 1e-4;
        let static_pitch = settled && cfg.pitch_amt == 0.0 && cfg.vib_depth <= 0.0;
        if settled {
            self.pitch = self.target;
        }
        let f_static = midi_to_hz(self.pitch);

        for f in out.iter_mut() {
            // ---- pitch
            let f0 = if static_pitch {
                f_static
            } else {
                self.next_hz(&cfg, vib_dt)
            };

            // ---- oscillators
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for o in 0..cfg.n_osc {
                let spec = &cfg.oscs[o];
                if spec.level == 0.0 {
                    continue;
                }
                if spec.pwm > 0.0 {
                    let base = match spec.wave {
                        Wave::Pulse(w) => w,
                        _ => 0.5,
                    };
                    let duty = (base + spec.pwm * (TAU * self.pwm_ph[o]).sin()).clamp(0.03, 0.97);
                    self.pwm_ph[o] += pwm_dt[o];
                    if self.pwm_ph[o] >= 1.0 {
                        self.pwm_ph[o] -= 1.0;
                    }
                    for u in 0..uni {
                        self.oscs[o][u].wave = Wave::Pulse(duty);
                    }
                }
                // pulse waves carry a DC offset of (2·duty − 1): remove it
                let dc = match self.oscs[o][0].wave {
                    Wave::Pulse(w) => 2.0 * w.clamp(0.01, 0.99) - 1.0,
                    _ => 0.0,
                };
                let (mut ol, mut or) = (0.0f32, 0.0f32);
                for u in 0..uni {
                    let s = self.oscs[o][u].next(f0 * self.ratio[o][u], sr) - dc;
                    ol += s * self.ugl[u];
                    or += s * self.ugr[u];
                }
                l += ol * spec.level;
                r += or * spec.level;
            }
            let mut mono = 0.0;
            if cfg.sub > 0.0 {
                mono += self.sub.next(f0 * 0.5, sr) * cfg.sub;
            }
            if cfg.noise > 0.0 {
                mono += self.rng.bipolar() * cfg.noise;
            }
            l += mono;
            r += mono;

            // ---- filter
            if self.ctrl_left == 0 {
                self.ctrl_left = CTRL;
                let cut = (self.cut_log + env_amt * self.fenv_val + kt + velc)
                    .exp2()
                    .clamp(16.0, sr * 0.45);
                match cfg.vcf {
                    Vcf::Off => {}
                    Vcf::Lp24 => {
                        self.ladder[0].set(cut, res, sr);
                        self.ladder[1] = Ladder {
                            s: self.ladder[1].s,
                            ..self.ladder[0]
                        };
                    }
                    _ => {
                        self.svf[0].set(cut, q, sr);
                        self.svf[1].set(cut, q, sr);
                    }
                }
            }
            self.ctrl_left -= 1;
            self.cut_log += dcut;
            self.fenv_val = self.fenv.next();
            if drive > 0.0 && cfg.vcf != Vcf::Lp24 {
                l = soft_clip(l * drv) * drv_norm;
                r = soft_clip(r * drv) * drv_norm;
            }
            let (l, r) = match cfg.vcf {
                Vcf::Off => (l, r),
                Vcf::Lp => (self.svf[0].lp(l), self.svf[1].lp(r)),
                Vcf::Hp => (self.svf[0].hp(l), self.svf[1].hp(r)),
                Vcf::Bp => (self.svf[0].bp(l), self.svf[1].bp(r)),
                Vcf::Lp24 => (
                    self.ladder[0].tick(l * drv) * drv_norm,
                    self.ladder[1].tick(r * drv) * drv_norm,
                ),
            };

            // ---- amp
            let a = self.aenv.next();
            self.vel_gain_s += (self.vel_gain - self.vel_gain_s) * 0.002;
            self.gain_prev += dgain;
            let g = a * self.vel_gain_s * self.gain_prev;
            *f += Frame::new(l * g, r * g);
        }
        !self.aenv.is_idle()
    }
}

/// Subtractive / virtual-analog polysynth: up to 3 oscillators (with unison/supersaw), sub
/// oscillator, noise, a resonant filter with its own envelope, amp envelope, pitch envelope,
/// vibrato, glide and mono/legato modes.
///
/// Start from a preset and tweak:
///
/// ```
/// use uwu_wave::prelude::*;
/// let lead = Synth::lead().cutoff(lfo::sine(4.bars()).range(1200.0, 5000.0)).vibrato(5.0, 0.2);
/// let acid = Synth::acid().res(0.9).env(4.0);
/// let custom = Synth::new()
///     .osc(OscSpec::saw())
///     .osc(OscSpec::saw().fine(9.0))
///     .sub(0.5)
///     .lp24()
///     .cutoff(900.0)
///     .res(0.3)
///     .filter_env(0.001, 0.25, 0.2, 0.2, 3.0)
///     .adsr(0.002, 0.3, 0.7, 0.2)
///     .mono();
/// ```
///
/// Velocity scales level (see [`Synth::velocity`]) and, with [`Synth::vel_cutoff`], brightness.
/// With [`Synth::accent`] (on by default in [`Synth::acid`]) notes with velocity ≥ 0.85 are
/// accented: louder, with a stronger and snappier filter envelope.
pub struct Synth {
    poly: Poly<SynthVoice>,
    sh: Arc<SynthShared>,
    cutoff: Param,
    res: Param,
    drive: Param,
    gain: Param,
    user_oscs: bool,
}

impl Clone for Synth {
    fn clone(&self) -> Synth {
        let sh = SynthShared::new();
        let mut poly = self.poly.clone();
        poly.panic();
        poly.proto_mut().sh = sh.clone();
        Synth {
            poly,
            sh,
            cutoff: self.cutoff.clone(),
            res: self.res.clone(),
            drive: self.drive.clone(),
            gain: self.gain.clone(),
            user_oscs: self.user_oscs,
        }
    }
}

impl Default for Synth {
    fn default() -> Self {
        Synth::new()
    }
}

impl Synth {
    /// A plain saw through an open-ish lowpass. Calling [`Synth::osc`] replaces the oscillator.
    pub fn new() -> Synth {
        let sh = SynthShared::new();
        Synth {
            poly: Poly::new(SynthVoice::new(sh.clone())).named("synth"),
            sh,
            cutoff: Param::Const(8000.0),
            res: Param::Const(0.1),
            drive: Param::Const(0.0),
            gain: Param::Const(0.3),
            user_oscs: false,
        }
    }

    fn cfg(&mut self) -> &mut SynthCfg {
        &mut self.poly.proto_mut().cfg
    }
    fn with(mut self, f: impl FnOnce(&mut SynthCfg)) -> Synth {
        f(self.cfg());
        self
    }

    // ---- oscillators ---------------------------------------------------------------------

    /// Add an oscillator (max 3). The first call replaces the preset's oscillators.
    pub fn osc(mut self, spec: impl Into<OscSpec>) -> Synth {
        let spec = spec.into();
        if !self.user_oscs {
            self.user_oscs = true;
            self.cfg().n_osc = 0;
        }
        let c = self.cfg();
        if c.n_osc < MAX_OSCS {
            c.oscs[c.n_osc] = spec;
            c.n_osc += 1;
        } else {
            c.oscs[MAX_OSCS - 1] = spec;
        }
        self
    }
    /// Set the waveform of oscillator 1.
    pub fn wave(self, w: Wave) -> Synth {
        self.with(|c| c.oscs[0].wave = w)
    }
    /// Pulse-width modulation on oscillator 1 (depth 0..0.45, rate in Hz).
    pub fn pwm(self, depth: f32, rate: f32) -> Synth {
        self.with(|c| c.oscs[0] = c.oscs[0].pwm(depth, rate))
    }
    /// Unison / supersaw: `voices` copies of every oscillator (max 8) detuned across ±`detune` cents.
    pub fn unison(self, voices: usize, detune_cents: f32) -> Synth {
        self.with(|c| {
            c.unison = voices.clamp(1, MAX_UNISON);
            c.detune = detune_cents;
            if c.spread == 0.0 && voices > 1 {
                c.spread = 0.7;
            }
        })
    }
    /// Stereo spread of unison voices (0 = mono, 1 = hard left/right).
    pub fn spread(self, s: f32) -> Synth {
        self.with(|c| c.spread = s.clamp(0.0, 1.0))
    }
    /// Sub oscillator (sine, one octave down) level.
    pub fn sub(self, level: impl Val) -> Synth {
        self.with(|c| c.sub = level.val())
    }
    /// Sub oscillator waveform (default sine).
    pub fn sub_wave(self, w: Wave) -> Synth {
        self.with(|c| c.sub_wave = w)
    }
    /// White noise level.
    pub fn noise(self, level: impl Val) -> Synth {
        self.with(|c| c.noise = level.val())
    }
    /// Reset oscillator phases on every note (punchier basses). Default: free-running.
    pub fn retrigger(self, on: bool) -> Synth {
        self.with(|c| c.retrig = on)
    }

    // ---- amp envelope ----------------------------------------------------------------------

    pub fn attack(self, secs: f32) -> Synth {
        self.with(|c| c.amp[0] = secs)
    }
    pub fn decay(self, secs: f32) -> Synth {
        self.with(|c| c.amp[1] = secs)
    }
    pub fn sustain(self, level: f32) -> Synth {
        self.with(|c| c.amp[2] = level.clamp(0.0, 1.0))
    }
    pub fn release(self, secs: f32) -> Synth {
        self.with(|c| c.amp[3] = secs)
    }
    /// Amp envelope (seconds, seconds, 0..1, seconds).
    pub fn adsr(self, a: f32, d: f32, s: f32, r: f32) -> Synth {
        self.with(|c| c.amp = [a, d, s.clamp(0.0, 1.0), r])
    }
    /// Velocity sensitivity of the level (0 = none, 1 = full). Default 0.7.
    pub fn velocity(self, sens: f32) -> Synth {
        self.with(|c| c.vel_sens = sens)
    }

    // ---- filter ----------------------------------------------------------------------------

    /// Filter cutoff in Hz (automatable).
    pub fn cutoff(mut self, hz: impl Into<Param>) -> Synth {
        self.cutoff = hz.into();
        self
    }
    /// Resonance 0..1 (automatable). 1 ≈ self-oscillation.
    pub fn res(mut self, r: impl Into<Param>) -> Synth {
        self.res = r.into();
        self
    }
    /// Saturation into the filter, 0..~2 (automatable).
    pub fn drive(mut self, d: impl Into<Param>) -> Synth {
        self.drive = d.into();
        self
    }
    /// Filter envelope amount in octaves (negative = downward).
    pub fn env(self, octaves: f32) -> Synth {
        self.with(|c| c.fenv_amt = octaves)
    }
    /// Filter envelope (a, d, s, r) and amount in octaves.
    pub fn filter_env(self, a: f32, d: f32, s: f32, r: f32, octaves: f32) -> Synth {
        self.with(|c| {
            c.fenv = [a, d, s.clamp(0.0, 1.0), r];
            c.fenv_amt = octaves;
        })
    }
    /// Keyboard tracking of the cutoff (1 = follows pitch exactly, relative to C4).
    pub fn key_track(self, k: f32) -> Synth {
        self.with(|c| c.key_track = k)
    }
    /// Velocity → cutoff in octaves (soft notes are darker by this much at velocity 0).
    pub fn vel_cutoff(self, octaves: f32) -> Synth {
        self.with(|c| c.vel_cutoff = octaves)
    }
    /// Filter type (see [`Vcf`]).
    pub fn filter(self, v: Vcf) -> Synth {
        self.with(|c| c.vcf = v)
    }
    /// 12 dB lowpass.
    pub fn lp(self) -> Synth {
        self.filter(Vcf::Lp)
    }
    /// 24 dB ladder lowpass.
    pub fn lp24(self) -> Synth {
        self.filter(Vcf::Lp24)
    }
    pub fn hp(self) -> Synth {
        self.filter(Vcf::Hp)
    }
    pub fn bp(self) -> Synth {
        self.filter(Vcf::Bp)
    }
    /// Bypass the filter.
    pub fn no_filter(self) -> Synth {
        self.filter(Vcf::Off)
    }
    /// Accent amount for high-velocity (≥ 0.85) notes: louder + extra filter envelope.
    pub fn accent(self, amount: f32) -> Synth {
        self.with(|c| c.accent = amount)
    }

    // ---- pitch -------------------------------------------------------------------------------

    /// Pitch envelope: start `semitones` away and fall back with `decay` seconds (zaps, toms, laser).
    pub fn pitch_env(self, semitones: f32, decay: f32) -> Synth {
        self.with(|c| {
            c.pitch_amt = semitones;
            c.pitch_decay = decay;
        })
    }
    /// Vibrato: `rate` Hz, `depth` in semitones.
    pub fn vibrato(self, rate: f32, depth: f32) -> Synth {
        self.with(|c| {
            c.vib_rate = rate;
            c.vib_depth = depth;
        })
    }
    /// Fade vibrato in over `secs` after the note starts.
    pub fn vibrato_delay(self, secs: f32) -> Synth {
        self.with(|c| c.vib_delay = secs)
    }

    // ---- voicing -----------------------------------------------------------------------------

    /// Monophonic, every note retriggers.
    pub fn mono(mut self) -> Synth {
        self.poly.mode = PolyMode::Mono;
        self
    }
    /// Monophonic legato: overlapping notes glide (`glide` seconds) without retriggering.
    pub fn legato(mut self, glide: f32) -> Synth {
        self.poly.mode = PolyMode::Legato;
        self.poly.glide = glide;
        self
    }
    /// Back to polyphonic.
    pub fn poly(mut self) -> Synth {
        self.poly.mode = PolyMode::Poly;
        self
    }
    /// Maximum polyphony.
    pub fn voices(mut self, n: usize) -> Synth {
        self.poly.max = n.max(1);
        self
    }
    /// Output gain (linear or `-6.db()`; automatable).
    pub fn gain(mut self, g: impl Into<Param>) -> Synth {
        self.gain = g.into();
        self
    }
    /// Name shown in reports.
    pub fn named(mut self, name: impl Into<String>) -> Synth {
        self.poly = self.poly.named(name);
        self
    }

    // ---- presets -----------------------------------------------------------------------------

    /// Bright saw.
    pub fn saw() -> Synth {
        Synth::new()
            .named("saw")
            .cutoff(9000.0)
            .res(0.1)
            .adsr(0.004, 0.25, 0.8, 0.2)
    }
    /// Hollow square.
    pub fn square() -> Synth {
        Synth::new()
            .named("square")
            .wave(Wave::Square)
            .cutoff(7000.0)
            .adsr(0.004, 0.25, 0.8, 0.2)
    }
    /// Pure sine.
    pub fn sine() -> Synth {
        Synth::new()
            .named("sine")
            .wave(Wave::Sine)
            .no_filter()
            .adsr(0.005, 0.1, 0.9, 0.2)
            .gain(0.4)
    }
    /// Soft triangle.
    pub fn tri() -> Synth {
        Synth::new()
            .named("tri")
            .wave(Wave::Triangle)
            .no_filter()
            .adsr(0.005, 0.15, 0.85, 0.2)
            .gain(0.4)
    }
    /// 7-voice supersaw for trance/EDM chords and leads.
    pub fn supersaw() -> Synth {
        Synth::new()
            .named("supersaw")
            .unison(7, 28.0)
            .spread(0.9)
            .sub(0.25)
            .cutoff(11_000.0)
            .res(0.05)
            .filter_env(0.01, 0.6, 0.6, 0.4, 0.8)
            .adsr(0.01, 0.4, 0.85, 0.35)
            .voices(10)
            .gain(0.22)
    }
    /// TB-303-style acid bass: mono legato with slide, squelchy resonant 24 dB filter, accent on
    /// velocity ≥ 0.85. Sweep `.cutoff(...)` and `.res(...)` for the classic squelch.
    pub fn acid() -> Synth {
        Synth::new()
            .named("acid")
            .lp24()
            .cutoff(380.0)
            .res(0.82)
            .drive(0.35)
            .filter_env(0.002, 0.28, 0.0, 0.05, 3.3)
            .accent(0.8)
            .adsr(0.002, 1.5, 0.75, 0.025)
            .velocity(0.35)
            .retrigger(true)
            .legato(0.06)
            .gain(0.42)
    }
    /// Lush, slow, wide pad.
    pub fn pad() -> Synth {
        Synth::new()
            .named("pad")
            .osc(OscSpec::saw())
            .osc(OscSpec::saw().fine(9.0).level(0.8))
            .osc(OscSpec::pulse(0.4).oct(-1).pwm(0.15, 0.3).level(0.4))
            .unison(3, 14.0)
            .spread(1.0)
            .cutoff(2000.0)
            .res(0.12)
            .filter_env(0.9, 1.5, 0.6, 1.5, 1.0)
            .key_track(0.3)
            .adsr(0.7, 1.0, 0.85, 1.8)
            .vibrato(4.8, 0.06)
            .velocity(0.4)
            .voices(10)
            .gain(0.16)
    }
    /// Short, bright pluck with a snappy filter envelope.
    pub fn pluck() -> Synth {
        Synth::new()
            .named("pluck")
            .osc(OscSpec::saw())
            .osc(OscSpec::square().oct(1).fine(5.0).level(0.35))
            .unison(2, 8.0)
            .spread(0.5)
            .cutoff(320.0)
            .res(0.25)
            .filter_env(0.001, 0.2, 0.0, 0.15, 5.0)
            .key_track(0.5)
            .vel_cutoff(1.5)
            .adsr(0.001, 0.45, 0.0, 0.25)
            .gain(0.33)
    }
    /// Round, punchy mono bass.
    pub fn bass() -> Synth {
        Synth::new()
            .named("bass")
            .osc(OscSpec::saw())
            .osc(OscSpec::square().oct(-1).level(0.5))
            .sub(0.5)
            .lp24()
            .cutoff(200.0)
            .res(0.2)
            .drive(0.3)
            .filter_env(0.001, 0.18, 0.25, 0.1, 2.6)
            .vel_cutoff(1.0)
            .adsr(0.002, 0.3, 0.85, 0.08)
            .retrigger(true)
            .mono()
            .gain(0.5)
    }
    /// Detuned, growling drum & bass "reese".
    pub fn reese() -> Synth {
        Synth::new()
            .named("reese")
            .osc(OscSpec::saw().fine(-14.0))
            .osc(OscSpec::saw().fine(14.0))
            .unison(2, 10.0)
            .spread(0.8)
            .sub(0.6)
            .lp24()
            .cutoff(650.0)
            .res(0.25)
            .drive(0.6)
            .adsr(0.005, 0.3, 1.0, 0.15)
            .legato(0.04)
            .gain(0.45)
    }
    /// Expressive mono lead with delayed vibrato and glide.
    pub fn lead() -> Synth {
        Synth::new()
            .named("lead")
            .osc(OscSpec::saw())
            .osc(OscSpec::square().fine(7.0).level(0.6))
            .cutoff(2400.0)
            .res(0.25)
            .filter_env(0.005, 0.35, 0.4, 0.3, 1.6)
            .key_track(0.5)
            .vel_cutoff(1.0)
            .adsr(0.005, 0.25, 0.85, 0.25)
            .vibrato(5.5, 0.18)
            .vibrato_delay(0.35)
            .legato(0.045)
            .gain(0.3)
    }
    /// Analog brass: slow-opening filter, velocity → brightness.
    pub fn brass() -> Synth {
        Synth::new()
            .named("brass")
            .osc(OscSpec::saw())
            .osc(OscSpec::saw().fine(6.0).level(0.8))
            .cutoff(500.0)
            .res(0.15)
            .filter_env(0.07, 0.45, 0.55, 0.25, 2.6)
            .key_track(0.6)
            .vel_cutoff(1.5)
            .adsr(0.035, 0.3, 0.85, 0.22)
            .vibrato(5.0, 0.08)
            .vibrato_delay(0.5)
            .voices(8)
            .gain(0.24)
    }
    /// Electric-piano-ish keys: pulse + triangle with a decaying filter.
    pub fn keys() -> Synth {
        Synth::new()
            .named("keys")
            .osc(OscSpec::pulse(0.3))
            .osc(OscSpec::tri().oct(1).level(0.5))
            .cutoff(900.0)
            .res(0.1)
            .filter_env(0.001, 0.7, 0.2, 0.3, 2.5)
            .key_track(0.7)
            .vel_cutoff(2.0)
            .adsr(0.002, 1.6, 0.25, 0.35)
            .velocity(0.8)
            .gain(0.28)
    }
    /// 8-bit chiptune pulse (12.5 % duty, no filter, quick vibrato).
    pub fn chip() -> Synth {
        Synth::new()
            .named("chip")
            .wave(Wave::Pulse(0.125))
            .no_filter()
            .adsr(0.001, 0.08, 0.75, 0.05)
            .vibrato(6.5, 0.12)
            .vibrato_delay(0.2)
            .velocity(0.5)
            .voices(4)
            .gain(0.18)
    }
    /// Laser/zap: fast downward pitch envelope.
    pub fn zap() -> Synth {
        Synth::new()
            .named("zap")
            .wave(Wave::Square)
            .pitch_env(36.0, 0.12)
            .cutoff(5000.0)
            .adsr(0.001, 0.18, 0.0, 0.1)
            .gain(0.25)
    }
}

impl Instrument for Synth {
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

impl Synth {
    #[inline]
    fn update(&self, ctx: &Ctx) {
        self.sh.cutoff.set(ctx.p(&self.cutoff));
        self.sh.res.set(ctx.p(&self.res));
        self.sh.drive.set(ctx.p(&self.drive));
        self.sh.gain.set(ctx.p(&self.gain));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::test_util::*;

    fn all() -> Vec<Synth> {
        vec![
            Synth::new(),
            Synth::saw(),
            Synth::square(),
            Synth::sine(),
            Synth::tri(),
            Synth::supersaw(),
            Synth::acid(),
            Synth::pad(),
            Synth::pluck(),
            Synth::bass(),
            Synth::reese(),
            Synth::lead(),
            Synth::brass(),
            Synth::keys(),
            Synth::chip(),
            Synth::zap(),
        ]
    }

    #[test]
    fn presets_render_and_end() {
        for mut s in all() {
            let name = s.name();
            let out = play(&mut s, 48.0, 0.9, 0.5, 4.5);
            assert!(finite(&out), "{name}: non-finite");
            let p = peak(&out);
            assert!(p > 0.02 && p < 1.5, "{name}: peak {p}");
            assert!(!s.active(), "{name}: still active after release");
        }
    }

    #[test]
    fn velocity_scales_level() {
        let loud = peak(&play(&mut Synth::saw(), 60.0, 1.0, 0.3, 0.4));
        let soft = peak(&play(&mut Synth::saw(), 60.0, 0.3, 0.3, 0.4));
        assert!(soft < loud * 0.6, "{soft} vs {loud}");
    }

    #[test]
    fn legato_glides_without_retrigger() {
        let mut s = Synth::acid();
        let out = play_notes(
            &mut s,
            &[(0.0, 36.0, 0.7, 0.3), (0.25, 48.0, 0.7, 0.3)],
            0.8,
        );
        assert!(finite(&out));
        // no gap between the two overlapping notes
        let mid = &out[(0.24 * 48_000.0) as usize..(0.27 * 48_000.0) as usize];
        assert!(rms(mid) > 0.01);
    }

    #[test]
    fn automated_cutoff() {
        let mut s = Synth::saw()
            .cutoff(crate::param::lfo::sine(1.0).range(200.0, 8000.0))
            .res(0.5);
        let out = play(&mut s, 45.0, 1.0, 1.0, 1.5);
        assert!(finite(&out));
        assert!(peak(&out) > 0.05);
    }

    #[test]
    fn clone_is_independent() {
        let a = Synth::pad();
        let mut b = a.clone();
        let out = play(&mut b, 60.0, 1.0, 0.2, 0.3);
        assert!(peak(&out) > 0.0);
        assert!(!Arc::ptr_eq(&a.sh, &b.sh));
    }
}
