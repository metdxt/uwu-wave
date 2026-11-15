//! Drawbar (tonewheel-style) organ with percussion, key click, overdrive and a rotary speaker.

use super::{CHUNK, clear, t60, vel_amp};
use crate::ctx::Ctx;
use crate::dsp::{Adsr, DelayLine, Frame, OnePole, Ramp, Rng, TAU, midi_to_hz, soft_clip};
use crate::instrument::{Instrument, NoteOn, Poly, Voice};
use crate::param::Param;

/// Harmonic number of each drawbar relative to the 16' sub-fundamental:
/// 16', 5⅓', 8', 4', 2⅔', 2', 1⅗', 1⅓', 1'.
const HARM: [usize; 9] = [1, 3, 2, 4, 6, 8, 10, 12, 16];

#[derive(Clone, Copy, Debug)]
struct OrganCfg {
    bars: [f32; 9],
    perc: f32,
    perc_harm: usize,
    perc_decay: f32,
    click: f32,
    vel_sens: f32,
    /// Mod wheel → vibrato (used when there is no rotary speaker).
    mw_vibrato: bool,
}

/// Rotary horn speed (Hz) the mod wheel crossfades towards.
const ROTARY_FAST: f32 = 6.7;
/// Mod-wheel vibrato (no rotary): rate (Hz) and depth at full wheel (semitones).
const MW_VIB_RATE: f32 = 6.9;
const MW_VIB_DEPTH: f32 = 0.3;

/// The voice behind [`Organ`]: nine sine drawbars computed with a Chebyshev recurrence.
#[derive(Clone)]
pub struct OrganVoice {
    cfg: OrganCfg,
    sr: f32,
    phase: f32,
    inc: f32,
    lv: [f32; 17],
    kmax: usize,
    env: Adsr,
    perc: f32,
    perc_k: f32,
    perc_lv: f32,
    click: f32,
    click_k: f32,
    click_lp: OnePole,
    rng: Rng,
    amp: f32,
    pitch: f32,
    bend: f32,
    mw: f32,
    vib_ph: f32,
}

impl Voice for OrganVoice {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.click_lp.set(5000.0, sr);
    }
    fn start(&mut self, n: &NoteOn, ctx: &Ctx) {
        let sr = self.sr;
        self.pitch = n.pitch;
        if self.env.is_idle() {
            self.bend = ctx.bend;
            self.mw = ctx.modwheel.clamp(0.0, 1.0);
        }
        let f16 = midi_to_hz(n.pitch + self.bend) * 0.5;
        self.inc = f16 / sr;
        if self.env.is_idle() {
            self.phase = 0.0;
        }
        let total: f32 = self.cfg.bars.iter().sum();
        let norm = 1.0 / total.sqrt().max(1.0);
        self.lv = [0.0; 17];
        for (b, &h) in self.cfg.bars.iter().zip(HARM.iter()) {
            if (h as f32) * f16 < sr * 0.45 {
                self.lv[h] += b * norm;
            }
        }
        self.perc_lv = if (self.cfg.perc_harm as f32) * f16 < sr * 0.45 {
            self.cfg.perc
        } else {
            0.0
        };
        // highest harmonic we actually need to compute
        let top = (1..=16).rev().find(|&k| self.lv[k] != 0.0).unwrap_or(1);
        self.kmax = if self.perc_lv > 0.0 {
            top.max(self.cfg.perc_harm)
        } else {
            top
        }
        .min(16);
        self.perc = 1.0;
        self.perc_k = t60(self.cfg.perc_decay, sr);
        self.click = self.cfg.click;
        self.click_k = t60(0.006, sr);
        self.rng = crate::dsp::Rng::new(n.id ^ 0x0F6A);
        self.amp = vel_amp(n.vel, self.cfg.vel_sens);
        self.env = Adsr::new(0.004, 0.05, 1.0, 0.045);
        self.env.gate_on(sr);
    }
    fn release(&mut self, _: &Ctx) {
        self.env.gate_off(self.sr);
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        let ph_h = self.cfg.perc_harm.min(16);
        let n = out.len().max(1) as f32;
        let dbend = (ctx.bend - self.bend) / n;
        let mw_t = if self.cfg.mw_vibrato {
            ctx.modwheel.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dmw = (mw_t - self.mw) / n;
        let vib_dt = MW_VIB_RATE / self.sr;
        // semitones → linear frequency deviation (small-angle approximation of 2^(x/12))
        let vib_k = MW_VIB_DEPTH * std::f32::consts::LN_2 / 12.0;
        let half_over_sr = 0.5 / self.sr;
        for f in out.iter_mut() {
            if dbend != 0.0 {
                self.bend += dbend;
                self.inc = midi_to_hz(self.pitch + self.bend) * half_over_sr;
            }
            self.mw += dmw;
            let mut inc = self.inc;
            if self.mw > 0.0 {
                inc *= 1.0 + vib_k * self.mw * (TAU * self.vib_ph).sin();
                self.vib_ph += vib_dt;
                if self.vib_ph >= 1.0 {
                    self.vib_ph -= 1.0;
                }
            }
            let th = TAU * self.phase;
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let (s1, c) = th.sin_cos();
            let c2 = 2.0 * c;
            let (mut a, mut b) = (0.0f32, s1); // sin((k-1)θ), sin(kθ)
            let mut sum = self.lv[1] * s1;
            let mut perc = 0.0;
            for k in 2..=self.kmax {
                let s = c2 * b - a;
                a = b;
                b = s;
                sum += self.lv[k] * s;
                if k == ph_h {
                    perc = s;
                }
            }
            let mut x = sum + perc * self.perc * self.perc_lv;
            self.perc *= self.perc_k;
            if self.click > 1e-4 {
                x += self.click_lp.lp(self.rng.bipolar()) * self.click;
                self.click *= self.click_k;
            }
            let y = x * self.env.next() * self.amp;
            *f += Frame::mono(y);
        }
        !self.env.is_idle()
    }
}

/// Rotary speaker state (horn + drum).
#[derive(Clone)]
struct Rotary {
    horn_rate: f32,
    drum_rate: f32,
    horn_ph: f32,
    drum_ph: f32,
    dl: DelayLine,
    xover: OnePole,
}

/// Drawbar organ. Track pitch bend retunes all voices (ramped per sample); the track mod wheel
/// speeds the rotary speaker up towards fast (≈6.7 Hz, with rotor inertia), or — with
/// [`Organ::no_rotary`] — adds a 6.9 Hz scanner-style vibrato.
///
/// Registrations are written like on a Hammond: `Organ::new("888000000")`.
///
/// ```
/// use uwu_wave::prelude::*;
/// let jazz = Organ::jazz();
/// let rock = Organ::new("888800000").drive(0.8).rotary_fast();
/// // automate the Leslie: slow in the verse, fast in the chorus
/// let speed = Lane::new(0.8);
/// let b3 = Organ::gospel().rotary(&speed);
/// ```
pub struct Organ {
    poly: Poly<OrganVoice>,
    gain: Param,
    rotary: Option<Param>,
    drive: f32,
    ramp: Ramp,
    rot: Option<Rotary>,
    scratch: Vec<Frame>,
    sr: f32,
}

impl Clone for Organ {
    fn clone(&self) -> Organ {
        let mut poly = self.poly.clone();
        poly.panic();
        Organ {
            poly,
            gain: self.gain.clone(),
            rotary: self.rotary.clone(),
            drive: self.drive,
            ramp: Ramp::default(),
            rot: None,
            scratch: vec![Frame::ZERO; CHUNK],
            sr: self.sr,
        }
    }
}

fn parse_bars(s: &str) -> [f32; 9] {
    let mut b = [0.0; 9];
    for (i, d) in s.chars().filter_map(|c| c.to_digit(10)).take(9).enumerate() {
        b[i] = d.min(8) as f32 / 8.0;
    }
    b
}

impl Organ {
    /// Organ with a drawbar registration (nine digits 0–8, spaces ignored), slow rotary.
    pub fn new(drawbars: &str) -> Organ {
        let v = OrganVoice {
            cfg: OrganCfg {
                bars: parse_bars(drawbars),
                perc: 0.0,
                perc_harm: 6,
                perc_decay: 0.3,
                click: 0.08,
                vel_sens: 0.25,
                mw_vibrato: false,
            },
            sr: 48_000.0,
            phase: 0.0,
            inc: 0.0,
            lv: [0.0; 17],
            kmax: 16,
            env: Adsr::new(0.004, 0.05, 1.0, 0.045),
            perc: 0.0,
            perc_k: 0.0,
            perc_lv: 0.0,
            click: 0.0,
            click_k: 0.0,
            click_lp: OnePole::default(),
            rng: Rng::new(3),
            amp: 1.0,
            pitch: 60.0,
            bend: 0.0,
            mw: 0.0,
            vib_ph: 0.0,
        };
        Organ {
            poly: Poly::new(v).named("organ"),
            gain: Param::Const(0.2),
            rotary: Some(Param::Const(0.8)),
            drive: 0.0,
            ramp: Ramp::default(),
            rot: None,
            scratch: vec![Frame::ZERO; CHUNK],
            sr: 48_000.0,
        }
    }
    fn cfg(mut self, f: impl FnOnce(&mut OrganCfg)) -> Organ {
        f(&mut self.poly.proto_mut().cfg);
        self
    }
    /// Classic jazz: 888000000 with fast 3rd-harmonic percussion.
    pub fn jazz() -> Organ {
        Organ::new("888000000")
            .named("jazz organ")
            .percussion(3, 0.6, 0.25)
    }
    /// Full gospel registration with a touch of drive.
    pub fn gospel() -> Organ {
        Organ::new("888800008")
            .named("gospel organ")
            .percussion(2, 0.4, 0.5)
            .drive(0.3)
    }
    /// Gritty rock organ: overdriven, fast rotary.
    pub fn rock() -> Organ {
        Organ::new("888600000")
            .named("rock organ")
            .drive(0.9)
            .rotary_fast()
            .gain(0.18)
    }
    /// All drawbars out — huge and bright.
    pub fn full() -> Organ {
        Organ::new("888888888").named("full organ").gain(0.15)
    }
    /// Soft, flute-like ballad registration.
    pub fn soft() -> Organ {
        Organ::new("008400000").named("soft organ").click(0.02)
    }
    /// Change the drawbar registration.
    pub fn drawbars(self, bars: &str) -> Organ {
        let b = parse_bars(bars);
        self.cfg(|c| c.bars = b)
    }
    /// Percussion on the 2nd (`2`) or 3rd (`3`) harmonic with level and decay (s).
    pub fn percussion(self, harmonic: u32, level: f32, decay: f32) -> Organ {
        self.cfg(|c| {
            c.perc_harm = if harmonic == 2 { 4 } else { 6 };
            c.perc = level;
            c.perc_decay = decay;
        })
    }
    /// Key-click level.
    pub fn click(self, level: f32) -> Organ {
        self.cfg(|c| c.click = level)
    }
    /// Velocity sensitivity (real tonewheels have none; default 0.25).
    pub fn velocity(self, sens: f32) -> Organ {
        self.cfg(|c| c.vel_sens = sens)
    }
    /// Preamp overdrive (0..2).
    pub fn drive(mut self, d: f32) -> Organ {
        self.drive = d.max(0.0);
        self
    }
    /// Rotary speaker horn speed in Hz (automatable; ≈0.8 chorale, ≈6.7 tremolo). The rotor
    /// accelerates/brakes with realistic inertia.
    pub fn rotary(mut self, hz: impl Into<Param>) -> Organ {
        self.rotary = Some(hz.into());
        self.poly.proto_mut().cfg.mw_vibrato = false;
        self
    }
    pub fn rotary_slow(self) -> Organ {
        self.rotary(0.8)
    }
    pub fn rotary_fast(self) -> Organ {
        self.rotary(6.7)
    }
    /// Bypass the rotary speaker.
    pub fn no_rotary(mut self) -> Organ {
        self.rotary = None;
        self.poly.proto_mut().cfg.mw_vibrato = true;
        self
    }
    /// Output gain (automatable).
    pub fn gain(mut self, g: impl Into<Param>) -> Organ {
        self.gain = g.into();
        self
    }
    pub fn voices(mut self, n: usize) -> Organ {
        self.poly.max = n.max(1);
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> Organ {
        self.poly = self.poly.named(name);
        self
    }
}

impl Instrument for Organ {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.poly.prepare(sr);
        self.scratch.resize(CHUNK, Frame::ZERO);
        self.rot = self.rotary.as_ref().map(|_| Rotary {
            horn_rate: 0.8,
            drum_rate: 0.7,
            horn_ph: 0.0,
            drum_ph: 0.25,
            dl: DelayLine::with_secs(0.01, sr),
            xover: OnePole::new(800.0, sr),
        });
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.poly.note_on(note, ctx);
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        self.poly.note_off(id, ctx);
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        let mut pos = 0;
        while pos < out.len() {
            let n = CHUNK.min(out.len() - pos);
            let cx = ctx.advanced(pos);
            let buf = &mut self.scratch[..n];
            clear(buf);
            self.poly.render(buf, &cx);
            self.ramp.start(cx.p(&self.gain), n);
            let dg = 1.0 + 2.5 * self.drive;
            let dnorm = 1.0 / soft_clip(dg);
            // mod wheel crossfades the rotor from its set speed to fast (the classic
            // half-moon switch, but continuous); inertia below makes it spin up/down naturally
            let mw = cx.modwheel.clamp(0.0, 1.0);
            let target = self
                .rotary
                .as_ref()
                .map(|p| {
                    let slow = cx.p(p).clamp(0.0, 12.0);
                    slow + (ROTARY_FAST - slow) * mw
                })
                .unwrap_or(0.0);
            let kh = (-1.0 / (0.6 * sr)).exp();
            let kd = (-1.0 / (1.8 * sr)).exp();
            for (o, x) in out[pos..pos + n].iter_mut().zip(buf.iter()) {
                let mut m = x.mid();
                if self.drive > 0.0 {
                    m = soft_clip(m * dg) * dnorm;
                }
                let y = match self.rot.as_mut() {
                    Some(r) => {
                        r.horn_rate = target + (r.horn_rate - target) * kh;
                        r.drum_rate = target * 0.88 + (r.drum_rate - target * 0.88) * kd;
                        r.horn_ph = (r.horn_ph + r.horn_rate / sr).fract();
                        r.drum_ph = (r.drum_ph + r.drum_rate / sr).fract();
                        let low = r.xover.lp(m);
                        let high = m - low;
                        r.dl.push(high);
                        let (hs, hc) = (TAU * r.horn_ph).sin_cos();
                        let base = 0.0018 * sr;
                        let depth = 0.00035 * sr;
                        let hl = r.dl.read_cubic(base + depth * hs) * (1.0 + 0.3 * hc);
                        let hr = r.dl.read_cubic(base - depth * hs) * (1.0 - 0.3 * hc);
                        let ds = (TAU * r.drum_ph).sin();
                        Frame::new(hl + low * (1.0 + 0.18 * ds), hr + low * (1.0 - 0.18 * ds))
                    }
                    None => Frame::mono(m),
                };
                *o += y * self.ramp.next();
            }
            pos += n;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::test_util::*;

    #[test]
    fn presets_render_and_end() {
        for mut o in [
            Organ::jazz(),
            Organ::gospel(),
            Organ::rock(),
            Organ::full(),
            Organ::soft(),
            Organ::new("800000000").no_rotary(),
        ] {
            let name = o.name();
            let out = play(&mut o, 60.0, 0.8, 0.5, 1.5);
            assert!(finite(&out), "{name}");
            let pk = peak(&out);
            assert!(pk > 0.03 && pk < 1.5, "{name}: {pk}");
            assert!(!o.active(), "{name}");
        }
    }

    #[test]
    fn bend_retunes_organ() {
        let sr = 48_000.0;
        let mut o = Organ::new("008000000").no_rotary().click(0.0);
        o.prepare(sr);
        let mut ctx = Ctx::offline(sr);
        ctx.bend = 12.0;
        o.note_on(
            &NoteOn {
                id: 1,
                pitch: 57.0,
                vel: 1.0,
                artic: Default::default(),
                dur: 1.0,
            },
            &ctx,
        );
        let mut out = vec![Frame::ZERO; 24_000];
        for c in out.chunks_mut(64) {
            o.render(c, &ctx);
            ctx = ctx.advanced(c.len());
        }
        let s = &out[4800..];
        let zc = s
            .windows(2)
            .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
            .count();
        let hz = zc as f32 / 2.0 / (s.len() as f32 / sr);
        assert!((hz - 440.0).abs() < 5.0, "{hz}");
    }

    #[test]
    fn modwheel_speeds_up_rotary() {
        // amplitude modulation depth of the horn grows with rotor speed within a short window
        let am = |mw: f32| {
            let sr = 48_000.0;
            let mut o = Organ::new("000000008").rotary_slow().click(0.0);
            o.prepare(sr);
            let mut ctx = Ctx::offline(sr);
            ctx.modwheel = mw;
            o.note_on(
                &NoteOn {
                    id: 1,
                    pitch: 72.0,
                    vel: 1.0,
                    artic: Default::default(),
                    dur: 4.0,
                },
                &ctx,
            );
            let mut out = vec![Frame::ZERO; 96_000];
            for c in out.chunks_mut(64) {
                o.render(c, &ctx);
                ctx = ctx.advanced(c.len());
            }
            // count envelope wiggles of the left channel over the last second
            let env: Vec<f32> = out[48_000..]
                .chunks(480)
                .map(|c| c.iter().map(|f| f.l.abs()).fold(0.0, f32::max))
                .collect();
            let mean = env.iter().sum::<f32>() / env.len() as f32;
            env.windows(2)
                .filter(|w| (w[0] < mean) != (w[1] < mean))
                .count()
        };
        assert!(am(1.0) > am(0.0) + 4, "{} vs {}", am(1.0), am(0.0));
    }

    #[test]
    fn drawbar_parse() {
        assert_eq!(
            parse_bars("88 8000 000"),
            [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(parse_bars("4")[0], 0.5);
    }

    #[test]
    fn single_drawbar_is_a_sine_at_the_right_pitch() {
        let mut o = Organ::new("008000000").no_rotary().click(0.0);
        let out = play(&mut o, 69.0, 1.0, 1.0, 0.6);
        let s = &out[4800..28800];
        let zc = s
            .windows(2)
            .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
            .count();
        let hz = zc as f32 / 2.0 / 0.5;
        assert!((hz - 440.0).abs() < 4.0, "{hz}");
    }
}
