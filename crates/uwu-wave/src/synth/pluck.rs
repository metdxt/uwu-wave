//! Karplus–Strong plucked strings.

use super::{CHUNK, clear, vel_amp};
use crate::ctx::Ctx;
use crate::dsp::{Biquad, BiquadKind, Frame, Ramp, Rng, midi_to_hz, pan_gains};
use crate::instrument::{Instrument, NoteOn, Poly, Voice};
use crate::param::Param;

#[derive(Clone, Copy, Debug)]
struct PluckCfg {
    decay: f32,
    bright: f32,
    pick: f32,
    release: f32,
    ring: bool,
    spread: f32,
    vel_sens: f32,
}

/// The voice behind [`Pluck`]: a Karplus–Strong string with a filtered-noise pluck, pick-position
/// comb, linear-phase damping filter and fractional (Hermite) delay tuning.
#[derive(Clone)]
pub struct PluckVoice {
    cfg: PluckCfg,
    sr: f32,
    buf: Vec<f32>,
    mask: usize,
    w: usize,
    delay: f32,
    g: f32,
    s: f32,
    prev: f32,
    amp: f32,
    gl: f32,
    gr: f32,
    rng: Rng,
    age: usize,
    period_n: usize,
    on: bool,
}

impl PluckVoice {
    #[inline]
    fn at(&self, i: isize) -> f32 {
        self.buf[(i as usize) & self.mask]
    }
    #[inline]
    fn read(&self, pos: f32) -> f32 {
        let i = pos.floor();
        let t = pos - i;
        let i = i as isize;
        let (xm1, x0, x1, x2) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }
    fn loop_gain(&self, t60: f32, hz: f32) -> f32 {
        0.001f32.powf(1.0 / (hz * t60.max(0.01)))
    }
}

impl Voice for PluckVoice {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        let need = (sr / 18.0) as usize + 8;
        let cap = need.next_power_of_two();
        if self.buf.len() != cap {
            self.buf = vec![0.0; cap];
            self.mask = cap - 1;
        }
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        if self.buf.is_empty() {
            self.prepare(self.sr);
        }
        let sr = self.sr;
        let cfg = self.cfg;
        let hz = midi_to_hz(n.pitch).clamp(20.0, sr * 0.25);
        let period = sr / hz;
        // loop filter: y = (1-S)·x + S·x[n-1] → exactly S samples of delay
        self.s = 0.5 * (1.0 - cfg.bright.clamp(0.0, 1.0)).powf(0.7) * 0.98 + 0.01;
        self.delay = period - self.s;
        self.g = self.loop_gain(cfg.decay, hz);
        self.prev = 0.0;
        // ---- excitation: velocity-bright filtered noise, combed by the pick position
        self.rng = Rng::new(n.id.wrapping_mul(0x2545_F491) ^ n.pitch.to_bits() as u64);
        let len = (period.ceil() as usize).min(self.buf.len() - 4);
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        let b = (cfg.bright * (0.55 + 0.45 * n.vel)).clamp(0.0, 1.0);
        let cut = 200.0 + 14_000.0 * b * b;
        let a = 1.0 - (-std::f32::consts::TAU * cut / sr).exp();
        let mut lp = 0.0;
        for i in 0..len {
            lp += a * (self.rng.bipolar() - lp);
            self.buf[i] = lp;
        }
        let p = ((cfg.pick.clamp(0.0, 0.5) * len as f32) as usize).max(1);
        if p < len {
            for i in (p..len).rev() {
                self.buf[i] -= self.buf[i - p];
            }
        }
        let mean = self.buf[..len].iter().sum::<f32>() / len as f32;
        let mut sq = 0.0;
        for x in &mut self.buf[..len] {
            *x -= mean;
            sq += *x * *x;
        }
        let rms = (sq / len as f32).sqrt().max(1e-6);
        let k = 0.4 / rms;
        self.buf[..len].iter_mut().for_each(|x| *x *= k);
        self.w = len;
        self.amp = vel_amp(n.vel, cfg.vel_sens);
        let pan = ((n.pitch - 60.0) / 30.0 * cfg.spread).clamp(-1.0, 1.0);
        let (gl, gr) = pan_gains(pan);
        self.gl = gl;
        self.gr = gr;
        self.age = 0;
        self.period_n = len;
        self.on = true;
    }
    fn release(&mut self, _: &Ctx) {
        if !self.cfg.ring && self.on {
            let hz = self.sr / (self.delay + self.s);
            self.g = self.g.min(self.loop_gain(self.cfg.release, hz));
        }
    }
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if !self.on {
            return false;
        }
        let (s, g) = (self.s, self.g);
        let mut pk = 0.0f32;
        for f in out.iter_mut() {
            let pos = self.w as f32 - self.delay;
            let x = self.read(pos);
            let y = ((1.0 - s) * x + s * self.prev) * g;
            self.prev = x;
            self.buf[self.w & self.mask] = y;
            self.w = (self.w + 1) & self.mask;
            let o = x * self.amp;
            pk = pk.max(o.abs());
            *f += Frame::new(o * self.gl, o * self.gr);
        }
        self.age += out.len();
        if self.age > self.period_n + 64 && pk < 2e-5 {
            self.on = false;
        }
        self.on
    }
}

/// Plucked string (Karplus–Strong) with a resonant body filter.
///
/// ```
/// use uwu_wave::prelude::*;
/// let gtr = Pluck::guitar().bright(0.7).decay(3.0);
/// let harp = Pluck::harp().gain(-3.db());
/// let custom = Pluck::new().pick(0.1).bright(0.9).decay(1.5).body(&[(180.0, 5.0, 2.0), (2500.0, 3.0, 1.0)]);
/// ```
pub struct Pluck {
    poly: Poly<PluckVoice>,
    gain: Param,
    ramp: Ramp,
    body: Vec<(f32, f32, f32)>,
    eq: Vec<Biquad>,
    scratch: Vec<Frame>,
    sr: f32,
}

impl Clone for Pluck {
    fn clone(&self) -> Pluck {
        let mut poly = self.poly.clone();
        poly.panic();
        Pluck {
            poly,
            gain: self.gain.clone(),
            ramp: Ramp::default(),
            body: self.body.clone(),
            eq: vec![],
            scratch: vec![Frame::ZERO; CHUNK],
            sr: self.sr,
        }
    }
}

impl Pluck {
    /// Neutral string, no body.
    pub fn new() -> Pluck {
        let v = PluckVoice {
            cfg: PluckCfg {
                decay: 2.5,
                bright: 0.6,
                pick: 0.2,
                release: 0.25,
                ring: false,
                spread: 0.3,
                vel_sens: 0.7,
            },
            sr: 48_000.0,
            buf: vec![],
            mask: 0,
            w: 0,
            delay: 100.0,
            g: 0.99,
            s: 0.5,
            prev: 0.0,
            amp: 1.0,
            gl: 1.0,
            gr: 1.0,
            rng: Rng::new(11),
            age: 0,
            period_n: 0,
            on: false,
        };
        Pluck {
            poly: Poly::new(v).named("pluck"),
            gain: Param::Const(0.5),
            ramp: Ramp::default(),
            body: vec![],
            eq: vec![],
            scratch: vec![Frame::ZERO; CHUNK],
            sr: 48_000.0,
        }
    }
    fn cfg(mut self, f: impl FnOnce(&mut PluckCfg)) -> Pluck {
        f(&mut self.poly.proto_mut().cfg);
        self
    }
    /// Nylon-ish acoustic guitar with a wooden body.
    pub fn guitar() -> Pluck {
        Pluck::new()
            .named("guitar")
            .decay(3.5)
            .bright(0.55)
            .pick(0.18)
            .release(0.3)
            .body(&[(105.0, 5.0, 1.8), (220.0, 3.0, 1.5), (2800.0, 2.5, 0.9)])
            .gain(0.4)
    }
    /// Harp: bright, long, strings ring after release.
    pub fn harp() -> Pluck {
        Pluck::new()
            .named("harp")
            .decay(5.0)
            .bright(0.65)
            .pick(0.45)
            .ring()
            .spread(0.6)
            .body(&[(180.0, 3.0, 1.2), (900.0, 2.0, 1.0)])
            .gain(0.45)
    }
    /// Koto-ish: very bright, plucked near the bridge, nasal body.
    pub fn koto() -> Pluck {
        Pluck::new()
            .named("koto")
            .decay(2.2)
            .bright(0.92)
            .pick(0.07)
            .release(0.4)
            .body(&[(320.0, 4.0, 2.0), (1300.0, 4.0, 2.5), (4000.0, -3.0, 0.8)])
            .gain(0.4)
    }
    /// Round plucked bass.
    pub fn bass() -> Pluck {
        Pluck::new()
            .named("pluck bass")
            .decay(2.0)
            .bright(0.2)
            .pick(0.3)
            .release(0.12)
            .spread(0.0)
            .body(&[(90.0, 4.0, 1.2)])
            .gain(0.55)
    }
    /// Ring time (seconds to -60 dB) while held.
    pub fn decay(self, secs: f32) -> Pluck {
        self.cfg(|c| c.decay = secs)
    }
    /// Brightness 0..1 (pluck spectrum and string damping).
    pub fn bright(self, b: f32) -> Pluck {
        self.cfg(|c| c.bright = b)
    }
    /// Pick position 0..0.5 (0.5 = middle of the string = hollow; small = near the bridge = twangy).
    pub fn pick(self, pos: f32) -> Pluck {
        self.cfg(|c| c.pick = pos)
    }
    /// Mute time after note-off.
    pub fn release(self, secs: f32) -> Pluck {
        self.cfg(|c| c.release = secs)
    }
    /// Let strings ring after note-off (harp / sustain pedal).
    pub fn ring(self) -> Pluck {
        self.cfg(|c| c.ring = true)
    }
    /// Stereo spread by pitch (0..1).
    pub fn spread(self, s: f32) -> Pluck {
        self.cfg(|c| c.spread = s)
    }
    /// Velocity sensitivity 0..1.
    pub fn velocity(self, sens: f32) -> Pluck {
        self.cfg(|c| c.vel_sens = sens)
    }
    /// Body resonances: up to 4 peaks of (Hz, dB, Q).
    pub fn body(mut self, peaks: &[(f32, f32, f32)]) -> Pluck {
        self.body = peaks.iter().take(4).copied().collect();
        self
    }
    /// Remove the body filter.
    pub fn no_body(mut self) -> Pluck {
        self.body.clear();
        self
    }
    /// Output gain (automatable).
    pub fn gain(mut self, g: impl Into<Param>) -> Pluck {
        self.gain = g.into();
        self
    }
    pub fn voices(mut self, n: usize) -> Pluck {
        self.poly.max = n.max(1);
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> Pluck {
        self.poly = self.poly.named(name);
        self
    }
}

impl Default for Pluck {
    fn default() -> Pluck {
        Pluck::new()
    }
}

impl Instrument for Pluck {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.poly.prepare(sr);
        self.eq = self
            .body
            .iter()
            .map(|&(f, db, q)| Biquad::new(BiquadKind::Peak(db), f, q, sr))
            .collect();
        self.scratch.resize(CHUNK, Frame::ZERO);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.poly.note_on(note, ctx);
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        self.poly.note_off(id, ctx);
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        let mut pos = 0;
        while pos < out.len() {
            let n = CHUNK.min(out.len() - pos);
            let cx = ctx.advanced(pos);
            let buf = &mut self.scratch[..n];
            clear(buf);
            self.poly.render(buf, &cx);
            self.ramp.start(cx.p(&self.gain), n);
            for (o, x) in out[pos..pos + n].iter_mut().zip(buf.iter()) {
                let mut y = *x;
                for eq in &mut self.eq {
                    y = eq.process(y);
                }
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
        for mut p in [
            Pluck::new(),
            Pluck::guitar(),
            Pluck::harp(),
            Pluck::koto(),
            Pluck::bass(),
        ] {
            let name = p.name();
            let out = play(&mut p, 52.0, 0.9, 0.5, 12.0);
            assert!(finite(&out), "{name}");
            let pk = peak(&out);
            assert!(pk > 0.05 && pk < 1.5, "{name}: {pk}");
            assert!(!p.active(), "{name} never ended");
        }
    }

    #[test]
    fn string_is_in_tune() {
        let mut p = Pluck::new().bright(0.5).decay(4.0);
        let out = play(&mut p, 57.0, 1.0, 1.0, 1.0); // A3 = 220 Hz
        // autocorrelation peak near the expected period
        let s: Vec<f32> = out[9600..19200].iter().map(|f| f.mid()).collect();
        let ac = |lag: usize| s.iter().zip(&s[lag..]).map(|(a, b)| a * b).sum::<f32>();
        let best = (150..300).max_by(|&a, &b| ac(a).total_cmp(&ac(b))).unwrap();
        let hz = 48_000.0 / best as f32;
        assert!((hz - 220.0).abs() < 2.5, "{hz}");
    }

    #[test]
    fn release_mutes_faster() {
        let held = rms(&play(&mut Pluck::guitar(), 50.0, 1.0, 2.0, 1.5)[60_000..72_000]);
        let muted = rms(&play(&mut Pluck::guitar(), 50.0, 1.0, 0.2, 1.5)[60_000..72_000]);
        assert!(muted < held * 0.1, "{muted} vs {held}");
    }
}
