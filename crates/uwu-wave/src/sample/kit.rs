//! [`SampleKit`]: a drum kit made of one-shot samples mapped to (GM) drum pitches.

use super::load::find_file;
use super::sampler::Src;
use super::voice::{Fader, Play, Region};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::engine::MAX_BLOCK;
use crate::instrument::{Instrument, NoteOn};
use crate::param::Param;
use crate::units::Val;
use std::path::Path;
use std::sync::Arc;

/// GM drum pitches used by the presets.
pub mod gm {
    pub const KICK: u8 = 36;
    pub const KICK_2: u8 = 35;
    pub const RIM: u8 = 37;
    pub const SNARE: u8 = 38;
    pub const CLAP: u8 = 39;
    pub const SNARE_2: u8 = 40;
    pub const TOM_LOW_FLOOR: u8 = 41;
    pub const HAT_CLOSED: u8 = 42;
    pub const TOM_HIGH_FLOOR: u8 = 43;
    pub const HAT_PEDAL: u8 = 44;
    pub const TOM_LOW: u8 = 45;
    pub const HAT_OPEN: u8 = 46;
    pub const TOM_LOW_MID: u8 = 47;
    pub const TOM_HIGH_MID: u8 = 48;
    pub const CRASH: u8 = 49;
    pub const TOM_HIGH: u8 = 50;
    pub const RIDE: u8 = 51;
    pub const TAMBOURINE: u8 = 54;
    pub const CRASH_2: u8 = 57;
}

/// Pitches tried (in order) when a note has no pad of its own.
fn fallback(n: u8) -> &'static [u8] {
    match n {
        35 => &[36],
        36 => &[35],
        40 => &[38],
        38 => &[40],
        39 => &[38],
        37 => &[38],
        44 => &[42],
        42 => &[44],
        43 => &[41, 45],
        41 => &[43, 45],
        45 => &[47, 41],
        47 => &[45, 48],
        48 => &[50, 47],
        50 => &[48],
        52 | 55 | 57 => &[49],
        49 => &[57],
        53 | 59 => &[51],
        _ => &[],
    }
}

#[derive(Clone)]
struct Pad {
    note: u8,
    srcs: Vec<Arc<Src>>,
    gain: f32,
    tune: f32,
    pan: f32,
    choke: Option<u8>,
    decay: Option<f32>,
    region: Region,
}

#[derive(Clone, Default)]
struct KitVoice {
    play: Play,
    fade: Fader,
    amp: f32,
    gl: f32,
    gr: f32,
    choke: Option<u8>,
    env: f32,
    coef: f32,
    age: u64,
}

impl KitVoice {
    fn alive(&self) -> bool {
        self.play.is_playing() || self.fade.active()
    }
    fn kill(&mut self, secs: f32, sr: f32) {
        self.fade.begin(&mut self.play, self.amp * self.env, secs, sr);
    }
}

/// A sample-based drum kit. Each pad maps a pitch (GM layout: 36 kick, 38 snare, 42 closed
/// hat...) to one or more one-shot samples (several = round-robin). Missing GM pitches fall back
/// to related pads (35→36, 40→38, 44→42, toms → nearest tom...).
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let kit = SampleKit::acoustic().level(38, -2.db()).tune(36, -1.0);
/// let mine = SampleKit::from_files([(36, "drums/kick.wav"), (38, "drums/snare.wav"), (42, "drums/hh.wav")])
///     .choke(&[42, 46])
///     .decay(49, 1.5);
/// ```
pub struct SampleKit {
    pads: Vec<Pad>,
    map: [i16; 128],
    voices: Vec<KitVoice>,
    rr: Vec<u32>,
    scratch: Vec<Frame>,
    gain: Param,
    ramp: Ramp,
    vel_sens: f32,
    vel_curve: f32,
    sr: f32,
    counter: u64,
    next_choke: u8,
    max_voices: usize,
    name: String,
}

impl SampleKit {
    /// An empty kit.
    pub fn new() -> SampleKit {
        SampleKit {
            pads: vec![],
            map: [-1; 128],
            voices: vec![],
            rr: vec![],
            scratch: vec![],
            gain: Param::Const(1.0),
            ramp: Ramp::default(),
            vel_sens: 1.0,
            vel_curve: 1.4,
            sr: 48000.0,
            counter: 0,
            next_choke: 1,
            max_voices: 32,
            name: "sample kit".into(),
        }
    }
    /// A kit from `(pitch, file)` pairs. Relative paths also search the assets folder.
    pub fn from_files<P: AsRef<Path>>(pads: impl IntoIterator<Item = (u8, P)>) -> SampleKit {
        pads.into_iter()
            .fold(SampleKit::new(), |k, (n, p)| k.pad(n, p))
    }
    fn pad_mut(&mut self, note: u8) -> Option<&mut Pad> {
        self.pads.iter_mut().find(|p| p.note == note)
    }
    fn with_pad(mut self, note: u8, f: impl FnOnce(&mut Pad)) -> SampleKit {
        match self.pad_mut(note) {
            Some(p) => f(p),
            None => eprintln!("uwu-wave: SampleKit has no pad {note}"),
        }
        self
    }
    /// Set (replace) the sample of pad `note`.
    pub fn pad(mut self, note: u8, path: impl AsRef<Path>) -> SampleKit {
        let p = find_file(path.as_ref()).unwrap_or_else(|_| path.as_ref().to_path_buf());
        let src = Src::path(p);
        match self.pad_mut(note) {
            Some(pad) => pad.srcs = vec![src],
            None => self.pads.push(Pad {
                note,
                srcs: vec![src],
                gain: 1.0,
                tune: 0.0,
                pan: 0.0,
                choke: None,
                decay: None,
                region: Region {
                    trim: true,
                    ..Region::default()
                },
            }),
        }
        self
    }
    /// Set pad `note` to an in-memory sample.
    pub fn pad_sample(mut self, note: u8, s: impl Into<Arc<super::Sample>>) -> SampleKit {
        let src = Src::sample(s.into());
        match self.pad_mut(note) {
            Some(pad) => pad.srcs = vec![src],
            None => self.pads.push(Pad {
                note,
                srcs: vec![src],
                gain: 1.0,
                tune: 0.0,
                pan: 0.0,
                choke: None,
                decay: None,
                region: Region {
                    trim: true,
                    ..Region::default()
                },
            }),
        }
        self
    }
    /// Add a round-robin alternative sample to pad `note` (creates the pad if needed).
    pub fn alt(mut self, note: u8, path: impl AsRef<Path>) -> SampleKit {
        if self.pad_mut(note).is_none() {
            return self.pad(note, path);
        }
        let p = find_file(path.as_ref()).unwrap_or_else(|_| path.as_ref().to_path_buf());
        self.pad_mut(note).unwrap().srcs.push(Src::path(p));
        self
    }
    /// Copy pad `from` to a new pitch `to`, retuned by `st` semitones (e.g. build three toms
    /// from one sample).
    pub fn derive(mut self, to: u8, from: u8, st: f32) -> SampleKit {
        if let Some(mut p) = self.pads.iter().find(|p| p.note == from).cloned() {
            p.note = to;
            p.tune += st;
            p.choke = None;
            self.pads.retain(|x| x.note != to);
            self.pads.push(p);
        }
        self
    }
    /// Pad level (linear or `-3.db()`).
    pub fn level(self, note: u8, g: impl Val) -> SampleKit {
        let g = g.val();
        self.with_pad(note, |p| p.gain = g)
    }
    /// Pad tuning in semitones.
    pub fn tune(self, note: u8, st: f32) -> SampleKit {
        self.with_pad(note, |p| p.tune = st)
    }
    /// Pad stereo position -1..1.
    pub fn pan(self, note: u8, pan: f32) -> SampleKit {
        self.with_pad(note, |p| p.pan = pan)
    }
    /// Fade a pad out after `secs` (exponential, -60 dB at `secs`) — shortens long cymbals.
    pub fn decay(self, note: u8, secs: f32) -> SampleKit {
        self.with_pad(note, |p| p.decay = Some(secs))
    }
    /// Start offset (seconds) of a pad.
    pub fn start(self, note: u8, secs: f32) -> SampleKit {
        self.with_pad(note, |p| p.region.start = secs)
    }
    /// Play a pad backwards.
    pub fn reverse(self, note: u8) -> SampleKit {
        self.with_pad(note, |p| p.region.reverse = true)
    }
    /// Put pads in a choke group: hitting any of them silences the others (and itself) —
    /// e.g. `.choke(&[42, 44, 46])` lets a closed hat cut an open hat.
    pub fn choke(mut self, notes: &[u8]) -> SampleKit {
        let g = self.next_choke;
        self.next_choke = self.next_choke.wrapping_add(1).max(1);
        for &n in notes {
            if let Some(p) = self.pad_mut(n) {
                p.choke = Some(g);
            }
        }
        self
    }
    /// Kit output gain, automatable.
    pub fn gain(mut self, g: impl Into<Param>) -> SampleKit {
        self.gain = g.into();
        self
    }
    /// Velocity sensitivity 0..1.
    pub fn velocity(mut self, sens: f32) -> SampleKit {
        self.vel_sens = sens.clamp(0.0, 1.0);
        self
    }
    /// Velocity → gain exponent (default 1.4).
    pub fn velocity_curve(mut self, k: f32) -> SampleKit {
        self.vel_curve = k.max(0.05);
        self
    }
    /// Maximum simultaneous hits (default 32).
    pub fn voices(mut self, n: usize) -> SampleKit {
        self.max_voices = n.max(1);
        self
    }
    /// Instrument name.
    pub fn named(mut self, name: impl Into<String>) -> SampleKit {
        self.name = name.into();
        self
    }
    /// Mapped pitches.
    pub fn pitches(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self.pads.iter().map(|p| p.note).collect();
        v.sort();
        v
    }
    fn lookup(&self, n: u8) -> Option<usize> {
        if self.map[n as usize] >= 0 {
            return Some(self.map[n as usize] as usize);
        }
        fallback(n)
            .iter()
            .find_map(|&f| (self.map[f as usize] >= 0).then(|| self.map[f as usize] as usize))
    }
}

impl Instrument for SampleKit {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.map = [-1; 128];
        for (i, p) in self.pads.iter().enumerate() {
            if (p.note as usize) < 128 {
                self.map[p.note as usize] = i as i16;
            }
            for s in &p.srcs {
                s.get(); // kits are small: load (and fetch) everything up front
            }
        }
        self.voices = vec![KitVoice::default(); self.max_voices];
        self.rr = vec![0; self.pads.len()];
        self.scratch = vec![Frame::ZERO; MAX_BLOCK];
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        if self.voices.is_empty() {
            self.prepare(ctx.sr);
        }
        let key = note.pitch.round().clamp(0.0, 127.0) as u8;
        let Some(pi) = self.lookup(key) else { return };
        let sr = self.sr;
        let pad = &self.pads[pi];
        if let Some(g) = pad.choke {
            for v in self
                .voices
                .iter_mut()
                .filter(|v| v.choke == Some(g) && v.play.is_playing())
            {
                v.kill(0.012, sr);
            }
        }
        let k = self.rr[pi] as usize % pad.srcs.len().max(1);
        self.rr[pi] = self.rr[pi].wrapping_add(1);
        let Some(smp) = pad.srcs.get(k).and_then(|s| s.get()) else {
            return;
        };
        // pick a free voice, else steal the oldest
        let vi = match self.voices.iter().position(|v| !v.alive()) {
            Some(i) => i,
            None => self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.age)
                .map(|(i, _)| i)
                .unwrap_or(0),
        };
        self.counter += 1;
        let v = &mut self.voices[vi];
        if v.play.is_playing() {
            v.kill(0.004, sr);
        }
        let ratio = 2f64.powf(((pad.tune + (note.pitch - key as f32)) / 12.0) as f64);
        v.play = Play::start(smp, &pad.region, ratio, sr);
        let vel = note.vel.clamp(0.0, 1.0);
        v.amp = pad.gain * ((1.0 - self.vel_sens) + self.vel_sens * vel.powf(self.vel_curve));
        let (gl, gr) = crate::dsp::pan_gains(pad.pan);
        v.gl = gl.min(1.0);
        v.gr = gr.min(1.0);
        v.choke = pad.choke;
        v.env = 1.0;
        v.coef = pad
            .decay
            .map(|d| (-6.9 / (d.max(0.001) * sr)).exp())
            .unwrap_or(1.0);
        v.age = self.counter;
    }
    fn note_off(&mut self, _id: u64, _ctx: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        if self.scratch.len() < out.len() {
            self.scratch.resize(out.len().max(MAX_BLOCK), Frame::ZERO);
        }
        let n = out.len();
        let buf = &mut self.scratch[..n];
        buf.fill(Frame::ZERO);
        for v in self.voices.iter_mut() {
            if !v.alive() {
                continue;
            }
            let (gl, gr) = (v.gl, v.gr);
            v.fade.render(buf, gl, gr);
            if !v.play.is_playing() {
                continue;
            }
            for f in buf.iter_mut() {
                let Some(x) = v.play.next() else { break };
                let g = v.amp * v.env;
                v.env *= v.coef;
                f.l += x.l * g * gl;
                f.r += x.r * g * gr;
            }
            if v.env < 1e-4 {
                v.play.stop();
            }
        }
        self.ramp.start(ctx.p(&self.gain), n);
        for (o, x) in out.iter_mut().zip(buf.iter()) {
            *o += *x * self.ramp.next();
        }
    }
    fn active(&self) -> bool {
        self.voices.iter().any(|v| v.alive())
    }
    fn panic(&mut self) {
        for v in &mut self.voices {
            *v = KitVoice::default();
        }
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;

    fn noise(secs: f32) -> Arc<Sample> {
        let n = (48000.0 * secs) as usize;
        let mut r = crate::dsp::Rng::new(3);
        Arc::new(Sample::from_mono(
            &(0..n).map(|_| r.bipolar() * 0.5).collect::<Vec<_>>(),
            48000.0,
        ))
    }

    #[test]
    fn choke_cuts_open_hat_and_fallbacks_work() {
        let sr = 48000.0;
        let ctx = Ctx::offline(sr);
        let mut k = SampleKit::new()
            .pad_sample(42, noise(0.05))
            .pad_sample(46, noise(2.0))
            .pad_sample(36, noise(0.3))
            .choke(&[42, 46]);
        k.prepare(sr);
        k.note_on(
            &NoteOn {
                id: 1,
                pitch: 46.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        let mut buf = [Frame::ZERO; 64];
        k.render(&mut buf, &ctx);
        assert!(buf.iter().any(|f| f.l.abs() > 0.01));
        k.note_on(
            &NoteOn {
                id: 2,
                pitch: 42.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        // after the closed hat (50 ms) and the choke fade, everything is silent long before 2 s
        for _ in 0..100 {
            buf = [Frame::ZERO; 64];
            k.render(&mut buf, &ctx);
        }
        assert!(!k.active());
        // 35 (acoustic bass drum) falls back to the kick pad
        k.note_on(
            &NoteOn {
                id: 3,
                pitch: 35.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        assert!(k.active());
        // unmapped & no fallback: ignored
        k.panic();
        k.note_on(
            &NoteOn {
                id: 4,
                pitch: 90.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        assert!(!k.active());
    }

    #[test]
    fn decay_shortens() {
        let sr = 48000.0;
        let ctx = Ctx::offline(sr);
        let mut k = SampleKit::new().pad_sample(49, noise(3.0)).decay(49, 0.1);
        k.prepare(sr);
        k.note_on(
            &NoteOn {
                id: 1,
                pitch: 49.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        let mut buf = [Frame::ZERO; 64];
        for _ in 0..(48000 / 64) {
            buf = [Frame::ZERO; 64];
            k.render(&mut buf, &ctx);
        }
        assert!(!k.active());
        assert!(buf.iter().all(|f| f.l == 0.0));
    }
}
