//! Analog-style drum voices (808/909 flavoured). Every sound implements [`Voice`], so it can be
//! played by a [`DrumMachine`](super::DrumMachine) or on its own through [`Poly`](crate::instrument::Poly).
//!
//! ```
//! use uwu_wave::prelude::*;
//! let kit = DrumMachine::tr808()
//!     .kick(Kick::tr808().decay(1.2).tune(45.0))
//!     .snare(Snare::tr909().snappy(0.9));
//! // an 808 "bass": the kick follows the note pitch
//! let boom = Poly::new(Kick::tr808().pitched().decay(1.6)).mono();
//! ```
//!
//! Drum sounds are one-shots: note-offs are ignored (a [`DrumMachine`](super::DrumMachine) uses
//! choke groups instead). Velocity scales level and, where it matters, brightness.

use super::{t60, vel_amp};
use crate::ctx::Ctx;
use crate::dsp::{Frame, OnePole, Rng, Svf, TAU, midi_to_hz, soft_clip};
use crate::instrument::{NoteOn, Voice};
use crate::units::Val;

const DEFAULT_SR: f32 = 48_000.0;

/// Exponential decay envelope.
#[derive(Clone, Copy, Debug, Default)]
struct Env {
    v: f32,
    k: f32,
}
impl Env {
    #[inline]
    fn trigger(&mut self, t: f32, sr: f32) {
        self.v = 1.0;
        self.k = t60(t.max(1e-4), sr);
    }
    #[inline]
    fn next(&mut self) -> f32 {
        let y = self.v;
        self.v *= self.k;
        y
    }
}

/// Six detuned square oscillators — the classic 808 cymbal/hat metal.
#[derive(Clone, Copy, Debug, Default)]
struct Metal {
    ph: [f32; 6],
}
const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
impl Metal {
    fn reset(&mut self, rng: &mut Rng) {
        for p in &mut self.ph {
            *p = rng.f32();
        }
    }
    #[inline]
    fn next(&mut self, tune: f32, sr: f32) -> f32 {
        let mut s = 0.0;
        for (p, f) in self.ph.iter_mut().zip(METAL) {
            s += if *p < 0.5 { 1.0 } else { -1.0 };
            *p += f * tune / sr;
            if *p >= 1.0 {
                *p -= 1.0;
            }
        }
        s * (1.0 / 6.0)
    }
}

/// 0.3 ms declick ramp.
#[inline]
fn declick(a: &mut f32, sr: f32) -> f32 {
    *a = (*a + 3300.0 / sr).min(1.0);
    *a
}

#[inline]
fn seed_of(n: &NoteOn) -> u64 {
    n.id.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD5_0F
}

// =============================================================================================
// Kick
// =============================================================================================

/// Kick drum: a sine with a pitch sweep, amp decay, click and drive. Track pitch bend retunes
/// it (great for 808 glides with [`Kick::pitched`]).
///
/// ```
/// use uwu_wave::prelude::*;
/// let k = Kick::tr909().tune(55.0).decay(0.4).click(0.6).drive(0.5);
/// let boom = Kick::tr808().decay(1.4).sweep(10.0);
/// ```
#[derive(Clone, Debug)]
pub struct Kick {
    /// Final (body) frequency in Hz.
    pub tune: f32,
    /// Pitch sweep in semitones above `tune` at the start.
    pub sweep: f32,
    /// Pitch envelope time (seconds to fall ~95 %).
    pub pitch_decay: f32,
    /// Hold at full level before the decay (seconds).
    pub hold: f32,
    /// Amp decay (seconds to -60 dB).
    pub decay: f32,
    /// Click/transient level.
    pub click: f32,
    /// Saturation amount (0..2).
    pub drive: f32,
    /// Output level.
    pub level: f32,
    /// Follow the note pitch instead of `tune` (for 808 basses).
    pub pitched: bool,
    sr: f32,
    phase: f32,
    f_end: f32,
    pe: f32,
    pk: f32,
    amp: Env,
    hold_left: u32,
    clk: Env,
    clk_lp: OnePole,
    rng: Rng,
    vel: f32,
    att: f32,
    on: bool,
    bend: f32,
}

impl Kick {
    pub fn new() -> Kick {
        Kick {
            tune: 50.0,
            sweep: 24.0,
            pitch_decay: 0.05,
            hold: 0.0,
            decay: 0.5,
            click: 0.3,
            drive: 0.2,
            level: 0.9,
            pitched: false,
            sr: DEFAULT_SR,
            phase: 0.0,
            f_end: 50.0,
            pe: 0.0,
            pk: 0.0,
            amp: Env::default(),
            hold_left: 0,
            clk: Env::default(),
            clk_lp: OnePole::default(),
            rng: Rng::new(1),
            vel: 1.0,
            att: 0.0,
            on: false,
            bend: 0.0,
        }
    }
    /// Boomy 808: long decay, gentle sweep, soft click.
    pub fn tr808() -> Kick {
        Kick {
            tune: 47.0,
            sweep: 13.0,
            pitch_decay: 0.06,
            decay: 1.0,
            click: 0.12,
            drive: 0.1,
            level: 0.85,
            ..Kick::new()
        }
    }
    /// Punchy 909: fast deep sweep, short decay, clicky attack.
    pub fn tr909() -> Kick {
        Kick {
            tune: 52.0,
            sweep: 30.0,
            pitch_decay: 0.035,
            hold: 0.025,
            decay: 0.38,
            click: 0.5,
            drive: 0.45,
            level: 0.72,
            ..Kick::new()
        }
    }
    /// Short, tight, round (lo-fi / boom-bap).
    pub fn tight() -> Kick {
        Kick {
            tune: 56.0,
            sweep: 18.0,
            pitch_decay: 0.03,
            decay: 0.28,
            click: 0.25,
            drive: 0.3,
            ..Kick::new()
        }
    }
    pub fn tune(mut self, hz: f32) -> Kick {
        self.tune = hz;
        self
    }
    pub fn sweep(mut self, semitones: f32) -> Kick {
        self.sweep = semitones;
        self
    }
    pub fn pitch_decay(mut self, secs: f32) -> Kick {
        self.pitch_decay = secs;
        self
    }
    pub fn hold(mut self, secs: f32) -> Kick {
        self.hold = secs;
        self
    }
    pub fn decay(mut self, secs: f32) -> Kick {
        self.decay = secs;
        self
    }
    pub fn click(mut self, amount: f32) -> Kick {
        self.click = amount;
        self
    }
    pub fn drive(mut self, amount: f32) -> Kick {
        self.drive = amount;
        self
    }
    pub fn level(mut self, l: impl Val) -> Kick {
        self.level = l.val();
        self
    }
    /// Follow note pitch (use with `Poly::new(Kick::tr808().pitched())` for 808 bass lines).
    pub fn pitched(mut self) -> Kick {
        self.pitched = true;
        self
    }
}

impl Default for Kick {
    fn default() -> Kick {
        Kick::new()
    }
}

impl Voice for Kick {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.clk_lp.set(3500.0, sr);
    }
    fn start(&mut self, n: &NoteOn, ctx: &Ctx) {
        let sr = self.sr;
        self.f_end = if self.pitched {
            midi_to_hz(n.pitch)
        } else {
            self.tune
        };
        self.vel = n.vel;
        self.phase = 0.0;
        self.pe = 1.0;
        self.pk = (-3.0 / (self.pitch_decay.max(1e-4) * sr)).exp();
        self.amp.trigger(self.decay, sr);
        self.hold_left = (self.hold * sr) as u32;
        self.clk.trigger(0.012, sr);
        self.clk_lp.set(2500.0 + 3000.0 * n.vel, sr);
        self.rng = Rng::new(seed_of(n));
        self.att = 0.0;
        self.on = true;
        self.bend = ctx.bend / 12.0;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85);
        let sweep = self.sweep * (0.7 + 0.3 * self.vel) / 12.0;
        let dg = 1.0 + 3.0 * self.drive;
        let dnorm = 1.0 / soft_clip(dg);
        // track pitch bend (octaves here), ramped across the block
        let db = (ctx.bend / 12.0 - self.bend) / out.len().max(1) as f32;
        for f in out.iter_mut() {
            self.bend += db;
            let freq = self.f_end * (sweep * self.pe + self.bend).exp2();
            self.pe *= self.pk;
            self.phase += freq / sr;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let body = (TAU * self.phase).sin();
            let a = if self.hold_left > 0 {
                self.hold_left -= 1;
                1.0
            } else {
                self.amp.next()
            };
            let mut x = soft_clip(body * dg) * dnorm * a;
            x += self.clk_lp.lp(self.rng.bipolar()) * self.clk.next() * self.click * 1.5;
            let y = x * g * declick(&mut self.att, sr);
            *f += Frame::mono(y);
        }
        self.on = self.amp.v > 1e-4 || self.hold_left > 0;
        self.on
    }
}

// =============================================================================================
// Snare
// =============================================================================================

/// Snare: two tuned body tones plus filtered noise ("snappy").
///
/// ```
/// use uwu_wave::prelude::*;
/// let s = Snare::tr909().tune(210.0).snappy(0.9).decay(0.25);
/// ```
#[derive(Clone, Debug)]
pub struct Snare {
    /// Body frequency (Hz).
    pub tune: f32,
    /// Ratio of the second body mode.
    pub ratio: f32,
    /// Body level.
    pub tone: f32,
    /// Body decay (s).
    pub tone_decay: f32,
    /// Noise level.
    pub snappy: f32,
    /// Noise decay (s).
    pub decay: f32,
    /// Noise highpass (Hz).
    pub color: f32,
    pub level: f32,
    sr: f32,
    ph: [f32; 2],
    pe: f32,
    pk: f32,
    tenv: Env,
    nenv: Env,
    hp: Svf,
    lp: OnePole,
    rng: Rng,
    vel: f32,
    att: f32,
    on: bool,
}

impl Snare {
    pub fn new() -> Snare {
        Snare {
            tune: 185.0,
            ratio: 1.8,
            tone: 0.6,
            tone_decay: 0.12,
            snappy: 0.7,
            decay: 0.2,
            color: 1800.0,
            level: 0.7,
            sr: DEFAULT_SR,
            ph: [0.0; 2],
            pe: 0.0,
            pk: 0.0,
            tenv: Env::default(),
            nenv: Env::default(),
            hp: Svf::default(),
            lp: OnePole::default(),
            rng: Rng::new(2),
            vel: 1.0,
            att: 0.0,
            on: false,
        }
    }
    pub fn tr808() -> Snare {
        Snare {
            tune: 180.0,
            ratio: 1.83,
            tone: 0.75,
            tone_decay: 0.13,
            snappy: 0.55,
            decay: 0.17,
            color: 1500.0,
            ..Snare::new()
        }
    }
    pub fn tr909() -> Snare {
        Snare {
            tune: 195.0,
            ratio: 1.6,
            tone: 0.55,
            tone_decay: 0.09,
            snappy: 0.85,
            decay: 0.24,
            color: 2200.0,
            ..Snare::new()
        }
    }
    /// Short, dusty snare for lo-fi kits.
    pub fn lofi() -> Snare {
        Snare {
            tune: 170.0,
            ratio: 1.7,
            tone: 0.7,
            tone_decay: 0.1,
            snappy: 0.5,
            decay: 0.14,
            color: 1200.0,
            ..Snare::new()
        }
    }
    pub fn tune(mut self, hz: f32) -> Snare {
        self.tune = hz;
        self
    }
    pub fn tone(mut self, level: f32) -> Snare {
        self.tone = level;
        self
    }
    pub fn snappy(mut self, level: f32) -> Snare {
        self.snappy = level;
        self
    }
    pub fn decay(mut self, secs: f32) -> Snare {
        self.decay = secs;
        self
    }
    pub fn tone_decay(mut self, secs: f32) -> Snare {
        self.tone_decay = secs;
        self
    }
    pub fn color(mut self, hp_hz: f32) -> Snare {
        self.color = hp_hz;
        self
    }
    pub fn level(mut self, l: impl Val) -> Snare {
        self.level = l.val();
        self
    }
}

impl Default for Snare {
    fn default() -> Snare {
        Snare::new()
    }
}

impl Voice for Snare {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.ph = [0.0, 0.25];
        self.pe = 1.0;
        self.pk = (-3.0 / (0.03 * sr)).exp();
        self.tenv.trigger(self.tone_decay, sr);
        self.nenv.trigger(self.decay * (0.8 + 0.2 * n.vel), sr);
        self.hp = Svf::new(self.color, 0.7, sr);
        self.lp.set(6000.0 + 6000.0 * n.vel, sr);
        self.rng = Rng::new(seed_of(n));
        self.att = 0.0;
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85);
        for f in out.iter_mut() {
            let bend = 1.0 + 0.25 * self.pe;
            self.pe *= self.pk;
            let f1 = self.tune * bend;
            self.ph[0] = (self.ph[0] + f1 / sr).fract();
            self.ph[1] = (self.ph[1] + f1 * self.ratio / sr).fract();
            let body = ((TAU * self.ph[0]).sin() * 0.65 + (TAU * self.ph[1]).sin() * 0.35)
                * self.tenv.next()
                * self.tone;
            let nz =
                self.hp.hp(self.lp.lp(self.rng.bipolar())) * self.nenv.next() * self.snappy * 1.4;
            *f += Frame::mono(soft_clip(body + nz) * g * declick(&mut self.att, sr));
        }
        self.on = self.tenv.v > 1e-4 || self.nenv.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Clap
// =============================================================================================

/// Hand clap: a few quick bandpassed noise bursts followed by a diffuse tail.
///
/// ```
/// use uwu_wave::prelude::*;
/// let c = Clap::tr909().decay(0.35).bursts(4);
/// ```
#[derive(Clone, Debug)]
pub struct Clap {
    /// Bandpass centre (Hz).
    pub tone: f32,
    /// Tail decay (s).
    pub decay: f32,
    /// Number of initial bursts.
    pub bursts: u32,
    /// Time between bursts (s).
    pub spacing: f32,
    pub level: f32,
    sr: f32,
    t: u32,
    space_n: u32,
    left: u32,
    benv: Env,
    tenv: Env,
    tail_on: bool,
    bp: Svf,
    hp: OnePole,
    rng: Rng,
    vel: f32,
    on: bool,
}

impl Clap {
    pub fn new() -> Clap {
        Clap {
            tone: 1100.0,
            decay: 0.25,
            bursts: 3,
            spacing: 0.011,
            level: 0.8,
            sr: DEFAULT_SR,
            t: 0,
            space_n: 1,
            left: 0,
            benv: Env::default(),
            tenv: Env::default(),
            tail_on: false,
            bp: Svf::default(),
            hp: OnePole::default(),
            rng: Rng::new(3),
            vel: 1.0,
            on: false,
        }
    }
    pub fn tr808() -> Clap {
        Clap {
            tone: 1000.0,
            decay: 0.3,
            bursts: 4,
            spacing: 0.01,
            ..Clap::new()
        }
    }
    pub fn tr909() -> Clap {
        Clap {
            tone: 1250.0,
            decay: 0.22,
            bursts: 3,
            spacing: 0.009,
            ..Clap::new()
        }
    }
    pub fn tone(mut self, hz: f32) -> Clap {
        self.tone = hz;
        self
    }
    pub fn decay(mut self, secs: f32) -> Clap {
        self.decay = secs;
        self
    }
    pub fn bursts(mut self, n: u32) -> Clap {
        self.bursts = n.max(1);
        self
    }
    pub fn spacing(mut self, secs: f32) -> Clap {
        self.spacing = secs;
        self
    }
    pub fn level(mut self, l: impl Val) -> Clap {
        self.level = l.val();
        self
    }
}

impl Default for Clap {
    fn default() -> Clap {
        Clap::new()
    }
}

impl Voice for Clap {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.t = 0;
        self.space_n = ((self.spacing * sr) as u32).max(1);
        self.left = self.bursts.max(1);
        self.benv = Env::default();
        self.tenv = Env::default();
        self.tail_on = false;
        self.bp = Svf::new(self.tone * (0.85 + 0.25 * n.vel), 1.6, sr);
        self.hp.set(500.0, sr);
        self.rng = Rng::new(seed_of(n));
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85) * 2.2;
        for f in out.iter_mut() {
            if self.left > 0 && self.t.is_multiple_of(self.space_n) {
                self.left -= 1;
                self.benv.trigger(0.025, sr);
                if self.left == 0 {
                    self.tenv.trigger(self.decay, sr);
                    self.tail_on = true;
                }
            }
            self.t += 1;
            let e = self.benv.next() + 0.55 * self.tenv.next();
            let x = self.bp.bp(self.hp.hp(self.rng.bipolar()));
            *f += Frame::mono(x * e * g);
        }
        self.on = self.left > 0 || self.benv.v > 1e-4 || self.tenv.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// HiHat
// =============================================================================================

/// Hi-hat from six square oscillators (808 metal) plus noise, band- and highpassed.
///
/// ```
/// use uwu_wave::prelude::*;
/// let ch = HiHat::closed().decay(0.04);
/// let oh = HiHat::open().noise(0.5).tone(9000.0);
/// ```
#[derive(Clone, Debug)]
pub struct HiHat {
    /// Metal pitch multiplier (1 = 808).
    pub tune: f32,
    /// Decay (s).
    pub decay: f32,
    /// Bandpass centre (Hz).
    pub tone: f32,
    /// Noise mix 0..1 (more noise = more 909).
    pub noise: f32,
    pub level: f32,
    sr: f32,
    metal: Metal,
    env: Env,
    bp: Svf,
    hp: Svf,
    rng: Rng,
    vel: f32,
    att: f32,
    on: bool,
}

impl HiHat {
    fn base(decay: f32) -> HiHat {
        HiHat {
            tune: 1.0,
            decay,
            tone: 10_000.0,
            noise: 0.2,
            level: 0.5,
            sr: DEFAULT_SR,
            metal: Metal::default(),
            env: Env::default(),
            bp: Svf::default(),
            hp: Svf::default(),
            rng: Rng::new(4),
            vel: 1.0,
            att: 0.0,
            on: false,
        }
    }
    /// Closed hat (~50 ms).
    pub fn closed() -> HiHat {
        HiHat::base(0.055)
    }
    /// Open hat (~0.45 s; choked by closed/pedal hats in a [`DrumMachine`](super::DrumMachine)).
    pub fn open() -> HiHat {
        HiHat {
            level: 0.42,
            ..HiHat::base(0.45)
        }
    }
    /// Pedal hat (foot chick).
    pub fn pedal() -> HiHat {
        HiHat {
            tone: 8000.0,
            level: 0.38,
            ..HiHat::base(0.09)
        }
    }
    pub fn tune(mut self, k: f32) -> HiHat {
        self.tune = k;
        self
    }
    pub fn decay(mut self, secs: f32) -> HiHat {
        self.decay = secs;
        self
    }
    pub fn tone(mut self, hz: f32) -> HiHat {
        self.tone = hz;
        self
    }
    pub fn noise(mut self, mix: f32) -> HiHat {
        self.noise = mix.clamp(0.0, 1.0);
        self
    }
    pub fn level(mut self, l: impl Val) -> HiHat {
        self.level = l.val();
        self
    }
}

impl Voice for HiHat {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.rng = Rng::new(seed_of(n));
        self.metal.reset(&mut self.rng);
        self.env.trigger(self.decay * (0.85 + 0.15 * n.vel), sr);
        self.bp = Svf::new(self.tone * (0.85 + 0.15 * n.vel), 0.9, sr);
        self.hp = Svf::new(self.tone * 0.68, 0.6, sr);
        self.att = 0.0;
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        // metal loses more energy in the bandpass than noise does: compensate
        let g = self.level * vel_amp(self.vel, 0.8) * (4.5 - 2.5 * self.noise);
        let nm = self.noise;
        for f in out.iter_mut() {
            let src = self.metal.next(self.tune, sr) * (1.0 - nm) + self.rng.bipolar() * nm;
            let x = self.hp.hp(self.bp.bp(src));
            *f += Frame::mono(x * self.env.next() * g * declick(&mut self.att, sr));
        }
        self.on = self.env.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Cymbal
// =============================================================================================

/// Crash / ride / china cymbals: metal + noise with a two-stage decay and optional bell.
///
/// ```
/// use uwu_wave::prelude::*;
/// let crash = Cymbal::crash().decay(2.5);
/// let ride = Cymbal::ride().bell(0.6);
/// ```
#[derive(Clone, Debug)]
pub struct Cymbal {
    pub tune: f32,
    /// Long decay (s).
    pub decay: f32,
    /// Bandpass centre (Hz).
    pub tone: f32,
    pub noise: f32,
    /// Bell/ping amount.
    pub bell: f32,
    pub level: f32,
    sr: f32,
    metal: Metal,
    metal2: Metal,
    fast: Env,
    slow: Env,
    bp: Svf,
    hp: Svf,
    bell_bp: Svf,
    rng: Rng,
    vel: f32,
    att: f32,
    on: bool,
}

impl Cymbal {
    fn base() -> Cymbal {
        Cymbal {
            tune: 1.0,
            decay: 2.0,
            tone: 7000.0,
            noise: 0.5,
            bell: 0.0,
            level: 0.45,
            sr: DEFAULT_SR,
            metal: Metal::default(),
            metal2: Metal::default(),
            fast: Env::default(),
            slow: Env::default(),
            bp: Svf::default(),
            hp: Svf::default(),
            bell_bp: Svf::default(),
            rng: Rng::new(5),
            vel: 1.0,
            att: 0.0,
            on: false,
        }
    }
    /// Washy crash (~2 s).
    pub fn crash() -> Cymbal {
        Cymbal::base()
    }
    /// Ride with a pingy bell.
    pub fn ride() -> Cymbal {
        Cymbal {
            tune: 1.25,
            decay: 1.6,
            tone: 8500.0,
            noise: 0.2,
            bell: 0.35,
            level: 0.3,
            ..Cymbal::base()
        }
    }
    /// Trashy china.
    pub fn china() -> Cymbal {
        Cymbal {
            tune: 0.8,
            decay: 1.4,
            tone: 4500.0,
            noise: 0.65,
            level: 0.45,
            ..Cymbal::base()
        }
    }
    /// Short splash.
    pub fn splash() -> Cymbal {
        Cymbal {
            tune: 1.4,
            decay: 0.7,
            tone: 9000.0,
            noise: 0.5,
            level: 0.4,
            ..Cymbal::base()
        }
    }
    pub fn tune(mut self, k: f32) -> Cymbal {
        self.tune = k;
        self
    }
    pub fn decay(mut self, secs: f32) -> Cymbal {
        self.decay = secs;
        self
    }
    pub fn tone(mut self, hz: f32) -> Cymbal {
        self.tone = hz;
        self
    }
    pub fn noise(mut self, mix: f32) -> Cymbal {
        self.noise = mix.clamp(0.0, 1.0);
        self
    }
    pub fn bell(mut self, amount: f32) -> Cymbal {
        self.bell = amount;
        self
    }
    pub fn level(mut self, l: impl Val) -> Cymbal {
        self.level = l.val();
        self
    }
}

impl Voice for Cymbal {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.rng = Rng::new(seed_of(n));
        self.metal.reset(&mut self.rng);
        self.metal2.reset(&mut self.rng);
        self.fast.trigger(0.18, sr);
        self.slow.trigger(self.decay * (0.7 + 0.3 * n.vel), sr);
        self.bp = Svf::new(self.tone * (0.8 + 0.2 * n.vel), 0.7, sr);
        self.hp = Svf::new(3000.0 * self.tune.sqrt(), 0.6, sr);
        self.bell_bp = Svf::new(2200.0 * self.tune, 6.0, sr);
        self.att = 0.0;
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.8) * 2.0;
        let nm = self.noise;
        for f in out.iter_mut() {
            let m =
                0.6 * self.metal.next(self.tune, sr) + 0.4 * self.metal2.next(self.tune * 1.47, sr);
            let src = m * (1.0 - nm) + self.rng.bipolar() * nm;
            let body = self.hp.hp(self.bp.bp(src));
            let fe = self.fast.next();
            let se = self.slow.next();
            let mut x = body * (0.45 * fe + 0.55 * se);
            if self.bell > 0.0 {
                x += self.bell_bp.bp(m) * self.bell * se * 2.0;
            }
            *f += Frame::mono(x * g * declick(&mut self.att, sr));
        }
        self.on = self.slow.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Tom
// =============================================================================================

/// Tom: a sine with a pitch drop, optional noise skin. Follows track pitch bend.
///
/// ```
/// use uwu_wave::prelude::*;
/// let floor = Tom::low().decay(0.7);
/// let custom = Tom::new(140.0).noise(0.3);
/// ```
#[derive(Clone, Debug)]
pub struct Tom {
    pub tune: f32,
    pub sweep: f32,
    pub pitch_decay: f32,
    pub decay: f32,
    pub noise: f32,
    pub level: f32,
    /// Follow note pitch.
    pub pitched: bool,
    sr: f32,
    phase: f32,
    f0: f32,
    pe: f32,
    pk: f32,
    amp: Env,
    nenv: Env,
    nlp: OnePole,
    rng: Rng,
    vel: f32,
    att: f32,
    on: bool,
    bend: f32,
}

impl Tom {
    /// Tom tuned to `hz`.
    pub fn new(hz: f32) -> Tom {
        Tom {
            tune: hz,
            sweep: 6.0,
            pitch_decay: 0.08,
            decay: 0.45,
            noise: 0.1,
            level: 0.5,
            pitched: false,
            sr: DEFAULT_SR,
            phase: 0.0,
            f0: hz,
            pe: 0.0,
            pk: 0.0,
            amp: Env::default(),
            nenv: Env::default(),
            nlp: OnePole::default(),
            rng: Rng::new(6),
            vel: 1.0,
            att: 0.0,
            on: false,
            bend: 0.0,
        }
    }
    pub fn low() -> Tom {
        Tom {
            decay: 0.6,
            ..Tom::new(85.0)
        }
    }
    pub fn mid() -> Tom {
        Tom {
            decay: 0.5,
            ..Tom::new(120.0)
        }
    }
    pub fn high() -> Tom {
        Tom {
            decay: 0.4,
            ..Tom::new(170.0)
        }
    }
    pub fn tune(mut self, hz: f32) -> Tom {
        self.tune = hz;
        self
    }
    pub fn sweep(mut self, semitones: f32) -> Tom {
        self.sweep = semitones;
        self
    }
    pub fn decay(mut self, secs: f32) -> Tom {
        self.decay = secs;
        self
    }
    pub fn noise(mut self, amount: f32) -> Tom {
        self.noise = amount;
        self
    }
    pub fn level(mut self, l: impl Val) -> Tom {
        self.level = l.val();
        self
    }
    pub fn pitched(mut self) -> Tom {
        self.pitched = true;
        self
    }
}

impl Voice for Tom {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, ctx: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.f0 = if self.pitched {
            midi_to_hz(n.pitch)
        } else {
            self.tune
        };
        self.phase = 0.0;
        self.pe = 1.0;
        self.pk = (-3.0 / (self.pitch_decay.max(1e-4) * sr)).exp();
        self.amp.trigger(self.decay, sr);
        self.nenv.trigger(0.06, sr);
        self.nlp.set(self.f0 * 20.0, sr);
        self.rng = Rng::new(seed_of(n));
        self.att = 0.0;
        self.on = true;
        self.bend = ctx.bend / 12.0;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85);
        let sweep = self.sweep * (0.6 + 0.4 * self.vel) / 12.0;
        let db = (ctx.bend / 12.0 - self.bend) / out.len().max(1) as f32;
        for f in out.iter_mut() {
            self.bend += db;
            let freq = self.f0 * (sweep * self.pe + self.bend).exp2();
            self.pe *= self.pk;
            self.phase = (self.phase + freq / sr).fract();
            let body = (TAU * self.phase).sin() * self.amp.next();
            let nz = self.nlp.lp(self.rng.bipolar()) * self.nenv.next() * self.noise;
            *f += Frame::mono(soft_clip((body + nz) * 1.2) * g * declick(&mut self.att, sr));
        }
        self.on = self.amp.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Cowbell
// =============================================================================================

/// 808 cowbell: two square waves (≈ 540 / 800 Hz) through a bandpass.
#[derive(Clone, Debug)]
pub struct Cowbell {
    pub tune: f32,
    pub decay: f32,
    pub level: f32,
    sr: f32,
    ph: [f32; 2],
    fast: Env,
    slow: Env,
    bp: Svf,
    vel: f32,
    att: f32,
    on: bool,
}

impl Cowbell {
    pub fn new() -> Cowbell {
        Cowbell {
            tune: 540.0,
            decay: 0.35,
            level: 0.4,
            sr: DEFAULT_SR,
            ph: [0.0; 2],
            fast: Env::default(),
            slow: Env::default(),
            bp: Svf::default(),
            vel: 1.0,
            att: 0.0,
            on: false,
        }
    }
    pub fn tr808() -> Cowbell {
        Cowbell::new()
    }
    pub fn tune(mut self, hz: f32) -> Cowbell {
        self.tune = hz;
        self
    }
    pub fn decay(mut self, secs: f32) -> Cowbell {
        self.decay = secs;
        self
    }
    pub fn level(mut self, l: impl Val) -> Cowbell {
        self.level = l.val();
        self
    }
}

impl Default for Cowbell {
    fn default() -> Cowbell {
        Cowbell::new()
    }
}

impl Voice for Cowbell {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.ph = [0.0, 0.3];
        self.fast.trigger(0.05, sr);
        self.slow.trigger(self.decay, sr);
        self.bp = Svf::new(self.tune * 3.1, 1.3, sr);
        self.att = 0.0;
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85);
        let (f1, f2) = (self.tune, self.tune * 1.4815);
        for f in out.iter_mut() {
            self.ph[0] = (self.ph[0] + f1 / sr).fract();
            self.ph[1] = (self.ph[1] + f2 / sr).fract();
            let sq = |p: f32| if p < 0.5 { 1.0 } else { -1.0 };
            let src = 0.5 * (sq(self.ph[0]) + sq(self.ph[1]));
            let o = self.bp.tick(src);
            let x = o.bp * 1.6 + o.lp * 0.25;
            let e = 0.6 * self.fast.next() + 0.4 * self.slow.next();
            *f += Frame::mono(x * e * g * declick(&mut self.att, sr));
        }
        self.on = self.slow.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Rimshot
// =============================================================================================

/// Rimshot / side-stick: two short inharmonic tones plus a click, highpassed and clipped.
#[derive(Clone, Debug)]
pub struct Rimshot {
    pub tune: f32,
    pub decay: f32,
    pub level: f32,
    sr: f32,
    ph: [f32; 2],
    env: Env,
    clk: Env,
    hp: OnePole,
    rng: Rng,
    vel: f32,
    on: bool,
}

impl Rimshot {
    pub fn new() -> Rimshot {
        Rimshot {
            tune: 455.0,
            decay: 0.035,
            level: 0.5,
            sr: DEFAULT_SR,
            ph: [0.0; 2],
            env: Env::default(),
            clk: Env::default(),
            hp: OnePole::default(),
            rng: Rng::new(8),
            vel: 1.0,
            on: false,
        }
    }
    pub fn tr808() -> Rimshot {
        Rimshot::new()
    }
    pub fn tr909() -> Rimshot {
        Rimshot {
            tune: 520.0,
            decay: 0.05,
            ..Rimshot::new()
        }
    }
    pub fn tune(mut self, hz: f32) -> Rimshot {
        self.tune = hz;
        self
    }
    pub fn decay(mut self, secs: f32) -> Rimshot {
        self.decay = secs;
        self
    }
    pub fn level(mut self, l: impl Val) -> Rimshot {
        self.level = l.val();
        self
    }
}

impl Default for Rimshot {
    fn default() -> Rimshot {
        Rimshot::new()
    }
}

impl Voice for Rimshot {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.ph = [0.0, 0.0];
        self.env.trigger(self.decay, sr);
        self.clk.trigger(0.004, sr);
        self.hp.set(350.0, sr);
        self.rng = Rng::new(seed_of(n));
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.85);
        for f in out.iter_mut() {
            self.ph[0] = (self.ph[0] + self.tune / sr).fract();
            self.ph[1] = (self.ph[1] + self.tune * 3.66 / sr).fract();
            let tone =
                ((TAU * self.ph[0]).sin() + 0.7 * (TAU * self.ph[1]).sin()) * self.env.next();
            let x = tone + self.rng.bipolar() * self.clk.next() * 0.8;
            *f += Frame::mono(soft_clip(self.hp.hp(x) * 2.0) * g);
        }
        self.on = self.env.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// Shaker
// =============================================================================================

/// Shaker / maracas / tambourine-ish noise percussion with a soft attack.
#[derive(Clone, Debug)]
pub struct Shaker {
    pub attack: f32,
    pub decay: f32,
    /// Highpass (Hz).
    pub tone: f32,
    /// Jingle (metal) amount — tambourine.
    pub jingle: f32,
    pub level: f32,
    sr: f32,
    metal: Metal,
    a: f32,
    ainc: f32,
    env: Env,
    hp: Svf,
    rng: Rng,
    vel: f32,
    on: bool,
}

impl Shaker {
    pub fn new() -> Shaker {
        Shaker {
            attack: 0.012,
            decay: 0.09,
            tone: 5500.0,
            jingle: 0.0,
            level: 0.35,
            sr: DEFAULT_SR,
            metal: Metal::default(),
            a: 0.0,
            ainc: 1.0,
            env: Env::default(),
            hp: Svf::default(),
            rng: Rng::new(9),
            vel: 1.0,
            on: false,
        }
    }
    /// Sharper, shorter maracas.
    pub fn maracas() -> Shaker {
        Shaker {
            attack: 0.003,
            decay: 0.05,
            tone: 7000.0,
            ..Shaker::new()
        }
    }
    /// Tambourine-ish jingle.
    pub fn tambourine() -> Shaker {
        Shaker {
            attack: 0.002,
            decay: 0.22,
            tone: 6000.0,
            jingle: 0.6,
            ..Shaker::new()
        }
    }
    pub fn attack(mut self, secs: f32) -> Shaker {
        self.attack = secs;
        self
    }
    pub fn decay(mut self, secs: f32) -> Shaker {
        self.decay = secs;
        self
    }
    pub fn tone(mut self, hz: f32) -> Shaker {
        self.tone = hz;
        self
    }
    pub fn level(mut self, l: impl Val) -> Shaker {
        self.level = l.val();
        self
    }
}

impl Default for Shaker {
    fn default() -> Shaker {
        Shaker::new()
    }
}

impl Voice for Shaker {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        let sr = self.sr;
        self.vel = n.vel;
        self.a = 0.0;
        self.ainc = 1.0 / (self.attack.max(1e-4) * sr);
        self.env.trigger(self.decay, sr);
        self.hp = Svf::new(self.tone * (0.8 + 0.2 * n.vel), 0.8, sr);
        self.rng = Rng::new(seed_of(n));
        self.metal.reset(&mut self.rng);
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let sr = self.sr;
        let g = self.level * vel_amp(self.vel, 0.8) * 1.6;
        for f in out.iter_mut() {
            let e = if self.a < 1.0 {
                self.a += self.ainc;
                self.a.min(1.0)
            } else {
                self.env.next()
            };
            let mut src = self.rng.bipolar();
            if self.jingle > 0.0 {
                src = src * (1.0 - self.jingle) + self.metal.next(2.7, sr) * self.jingle;
            }
            *f += Frame::mono(self.hp.hp(src) * e * g);
        }
        self.on = self.a < 1.0 || self.env.v > 1e-4;
        self.on
    }
}

// =============================================================================================
// DrumSound — one enum for every drum voice
// =============================================================================================

/// Any drum voice. Every drum type converts into it (`Kick::tr808().into()`), so a
/// [`DrumMachine`](super::DrumMachine) can hold a mix of them.
#[derive(Clone, Debug)]
pub enum DrumSound {
    Kick(Kick),
    Snare(Snare),
    Clap(Clap),
    HiHat(HiHat),
    Tom(Tom),
    Cymbal(Cymbal),
    Cowbell(Cowbell),
    Rimshot(Rimshot),
    Shaker(Shaker),
}

macro_rules! drum_dispatch {
    ($self:ident, $v:ident => $e:expr) => {
        match $self {
            DrumSound::Kick($v) => $e,
            DrumSound::Snare($v) => $e,
            DrumSound::Clap($v) => $e,
            DrumSound::HiHat($v) => $e,
            DrumSound::Tom($v) => $e,
            DrumSound::Cymbal($v) => $e,
            DrumSound::Cowbell($v) => $e,
            DrumSound::Rimshot($v) => $e,
            DrumSound::Shaker($v) => $e,
        }
    };
}

macro_rules! drum_from {
    ($($t:ident),*) => {$(
        impl From<$t> for DrumSound {
            fn from(v: $t) -> DrumSound {
                DrumSound::$t(v)
            }
        }
    )*};
}
drum_from!(
    Kick, Snare, Clap, HiHat, Tom, Cymbal, Cowbell, Rimshot, Shaker
);

impl Voice for DrumSound {
    fn prepare(&mut self, sr: f32) {
        drum_dispatch!(self, v => v.prepare(sr))
    }
    fn start(&mut self, note: &NoteOn, ctx: &Ctx) {
        drum_dispatch!(self, v => v.start(note, ctx))
    }
    fn release(&mut self, ctx: &Ctx) {
        drum_dispatch!(self, v => v.release(ctx))
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        drum_dispatch!(self, v => v.render(out, ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::{Instrument, Poly};
    use crate::synth::test_util::*;

    fn sounds() -> Vec<DrumSound> {
        vec![
            Kick::tr808().into(),
            Kick::tr909().into(),
            Kick::tight().into(),
            Snare::tr808().into(),
            Snare::tr909().into(),
            Clap::tr808().into(),
            Clap::tr909().into(),
            HiHat::closed().into(),
            HiHat::open().into(),
            HiHat::pedal().into(),
            Tom::low().into(),
            Tom::high().into(),
            Cymbal::crash().into(),
            Cymbal::ride().into(),
            Cymbal::china().into(),
            Cowbell::new().into(),
            Rimshot::tr808().into(),
            Rimshot::tr909().into(),
            Shaker::new().into(),
            Shaker::tambourine().into(),
        ]
    }

    #[test]
    fn every_drum_sounds_and_ends() {
        for (i, d) in sounds().into_iter().enumerate() {
            let mut p = Poly::new(d);
            let out = play(&mut p, 36.0, 1.0, 0.05, 4.0);
            assert!(finite(&out), "drum {i}");
            let pk = peak(&out);
            assert!(pk > 0.05 && pk < 1.6, "drum {i} peak {pk}");
            assert!(!p.active(), "drum {i} never ended");
        }
    }

    #[test]
    fn bend_retunes_808() {
        let measure = |bend: f32| {
            let mut p = Poly::new(Kick::tr808().pitched().sweep(0.0).decay(3.0));
            let sr = 48_000.0;
            p.prepare(sr);
            let mut ctx = Ctx::offline(sr);
            ctx.bend = bend;
            p.note_on(
                &NoteOn {
                    id: 1,
                    pitch: 45.0,
                    vel: 1.0,
                    dur: 1.0,
                },
                &ctx,
            );
            let mut out = vec![Frame::ZERO; 24_000];
            for c in out.chunks_mut(64) {
                p.render(c, &ctx);
                ctx = ctx.advanced(c.len());
            }
            out[4800..]
                .windows(2)
                .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
                .count() as f32
        };
        let r = measure(12.0) / measure(0.0);
        assert!((r - 2.0).abs() < 0.03, "{r}");
    }

    #[test]
    fn velocity_matters() {
        let loud = peak(&play(&mut Poly::new(Snare::tr909()), 38.0, 1.0, 0.1, 0.3));
        let soft = peak(&play(&mut Poly::new(Snare::tr909()), 38.0, 0.3, 0.1, 0.3));
        assert!(soft < loud * 0.6);
    }

    #[test]
    fn pitched_kick_follows_note() {
        let mut p = Poly::new(Kick::tr808().pitched().sweep(0.0));
        let out = play(&mut p, 45.0, 1.0, 0.1, 0.5); // A2 = 110 Hz
        // count zero crossings over 0.2 s from 0.1 s
        let s = &out[4800..14400];
        let zc = s
            .windows(2)
            .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
            .count();
        let hz = zc as f32 / 2.0 / 0.2;
        assert!((hz - 110.0).abs() < 8.0, "{hz}");
    }
}
