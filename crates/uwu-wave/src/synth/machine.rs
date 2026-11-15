//! [`DrumMachine`]: a General-MIDI-mapped kit of synthesized drum voices.

use super::drums::{Clap, Cowbell, Cymbal, DrumSound, HiHat, Kick, Rimshot, Shaker, Snare, Tom};
use super::{CHUNK, clear};
use crate::ctx::Ctx;
use crate::dsp::{Biquad, BiquadKind, Frame, Ramp, pan_gains, soft_clip};
use crate::instrument::{Instrument, NoteOn, Voice};
use crate::param::Param;
use crate::units::Val;

const NONE: u16 = u16::MAX;
/// Choke fade time (seconds).
const CHOKE: f32 = 0.006;

#[derive(Clone)]
struct Pad {
    v: [DrumSound; 2],
    alive: [bool; 2],
    fade: [f32; 2],
    choking: [bool; 2],
    cur: usize,
    level: f32,
    pan: f32,
    group: u8,
}

impl Pad {
    fn new(d: DrumSound) -> Pad {
        Pad {
            v: [d.clone(), d],
            alive: [false; 2],
            fade: [1.0; 2],
            choking: [false; 2],
            cur: 0,
            level: 1.0,
            pan: 0.0,
            group: 0,
        }
    }
    fn choke(&mut self) {
        for k in 0..2 {
            if self.alive[k] {
                self.choking[k] = true;
            }
        }
    }
}

/// A drum machine: maps note pitches (General MIDI drum layout) to synthesized drum voices.
///
/// | pitch | sound | pattern name |
/// |---|---|---|
/// | 35, 36 | kick | `bd` |
/// | 37 | rimshot | `rim` |
/// | 38, 40 | snare | `sd` |
/// | 39 | clap | `cp` |
/// | 42 | closed hat | `hh` |
/// | 44 | pedal hat | |
/// | 46 | open hat (choked by 42/44) | `oh` |
/// | 41, 43 | low tom | `lt` |
/// | 45, 47 | mid tom | `mt` |
/// | 48, 50 | high tom | `ht` |
/// | 49, 57 | crash | `cr` |
/// | 51, 59 | ride (53 = ride bell) | `rd` |
/// | 52 / 55 | china / splash | |
/// | 54 | tambourine | |
/// | 56 | cowbell | `cb` |
/// | 69, 70 | shaker / maracas | `sh` |
///
/// ```
/// use uwu_wave::prelude::*;
/// let song = Song::new("beat").bpm(126);
/// let kit = DrumMachine::tr909()
///     .kick(Kick::tr909().decay(0.5).drive(0.8))
///     .level(DrumMachine::OPEN_HAT, -4.db())
///     .pan(DrumMachine::CLOSED_HAT, 0.25);
/// let drums = song.track("drums", kit);
/// ```
pub struct DrumMachine {
    pads: Vec<Pad>,
    map: [u16; 128],
    gain: Param,
    ramp: Ramp,
    drive: f32,
    bits: f32,
    down: u32,
    hold: Frame,
    hold_n: u32,
    lowpass: Option<f32>,
    lp: Biquad,
    sr: f32,
    name: String,
    tmp: Vec<Frame>,
    mix: Vec<Frame>,
}

impl Clone for DrumMachine {
    fn clone(&self) -> DrumMachine {
        DrumMachine {
            pads: self.pads.clone(),
            map: self.map,
            gain: self.gain.clone(),
            ramp: Ramp::default(),
            drive: self.drive,
            bits: self.bits,
            down: self.down,
            hold: Frame::ZERO,
            hold_n: 0,
            lowpass: self.lowpass,
            lp: Biquad::default(),
            sr: self.sr,
            name: self.name.clone(),
            tmp: vec![Frame::ZERO; CHUNK],
            mix: vec![Frame::ZERO; CHUNK],
        }
    }
}

impl Default for DrumMachine {
    fn default() -> Self {
        DrumMachine::new()
    }
}

impl DrumMachine {
    pub const KICK: u8 = 36;
    pub const RIM: u8 = 37;
    pub const SNARE: u8 = 38;
    pub const CLAP: u8 = 39;
    pub const LOW_TOM: u8 = 41;
    pub const CLOSED_HAT: u8 = 42;
    pub const PEDAL_HAT: u8 = 44;
    pub const MID_TOM: u8 = 45;
    pub const OPEN_HAT: u8 = 46;
    pub const HIGH_TOM: u8 = 48;
    pub const CRASH: u8 = 49;
    pub const RIDE: u8 = 51;
    pub const CHINA: u8 = 52;
    pub const RIDE_BELL: u8 = 53;
    pub const TAMBOURINE: u8 = 54;
    pub const SPLASH: u8 = 55;
    pub const COWBELL: u8 = 56;
    pub const SHAKER: u8 = 70;

    /// An empty machine (map sounds with [`DrumMachine::map`] or the named setters).
    pub fn new() -> DrumMachine {
        DrumMachine {
            pads: vec![],
            map: [NONE; 128],
            gain: Param::Const(1.0),
            ramp: Ramp::default(),
            drive: 0.0,
            bits: 0.0,
            down: 1,
            hold: Frame::ZERO,
            hold_n: 0,
            lowpass: None,
            lp: Biquad::default(),
            sr: 48_000.0,
            name: "drums".into(),
            tmp: vec![Frame::ZERO; CHUNK],
            mix: vec![Frame::ZERO; CHUNK],
        }
    }

    /// A machine where every pitch triggers the same sound (e.g. a lone kick track).
    pub fn only(sound: impl Into<DrumSound>) -> DrumMachine {
        let mut m = DrumMachine::new();
        m.pads.push(Pad::new(sound.into()));
        m.map = [0; 128];
        m
    }

    /// Classic Roland TR-808 flavour.
    pub fn tr808() -> DrumMachine {
        DrumMachine::new()
            .named("tr808")
            .kick(Kick::tr808())
            .snare(Snare::tr808())
            .clap(Clap::tr808())
            .rim(Rimshot::tr808())
            .closed_hat(HiHat::closed())
            .pedal_hat(HiHat::pedal())
            .open_hat(HiHat::open())
            .low_tom(Tom::low().noise(0.03))
            .mid_tom(Tom::mid().noise(0.03))
            .high_tom(Tom::high().noise(0.03))
            .crash(Cymbal::crash())
            .ride(Cymbal::ride())
            .map_with(Self::CHINA, Cymbal::china())
            .map_with(Self::SPLASH, Cymbal::splash())
            .map_with(Self::RIDE_BELL, Cymbal::ride().bell(1.0).decay(1.2))
            .map_with(Self::TAMBOURINE, Shaker::tambourine())
            .cowbell(Cowbell::tr808())
            .shaker(Shaker::maracas())
            .default_pans()
    }

    /// Roland TR-909 flavour: punchier kick, noisier snare/hats, brighter clap.
    pub fn tr909() -> DrumMachine {
        DrumMachine::new()
            .named("tr909")
            .kick(Kick::tr909())
            .snare(Snare::tr909())
            .clap(Clap::tr909())
            .rim(Rimshot::tr909())
            .closed_hat(HiHat::closed().noise(0.55).tone(9500.0).decay(0.05))
            .pedal_hat(HiHat::pedal().noise(0.55))
            .open_hat(HiHat::open().noise(0.55).tone(9500.0).decay(0.38))
            .low_tom(Tom::low().noise(0.35).sweep(9.0))
            .mid_tom(Tom::mid().noise(0.35).sweep(9.0))
            .high_tom(Tom::high().noise(0.35).sweep(9.0))
            .crash(Cymbal::crash().noise(0.65).tone(7500.0))
            .ride(Cymbal::ride().noise(0.4))
            .map_with(Self::CHINA, Cymbal::china())
            .map_with(Self::SPLASH, Cymbal::splash())
            .map_with(Self::RIDE_BELL, Cymbal::ride().bell(1.0).decay(1.2))
            .map_with(Self::TAMBOURINE, Shaker::tambourine())
            .cowbell(Cowbell::tr808())
            .shaker(Shaker::new())
            .default_pans()
    }

    /// Dusty boom-bap kit: tight kick, short snare, dark hats, bit-crushed and saturated.
    pub fn lofi() -> DrumMachine {
        DrumMachine::new()
            .named("lofi")
            .kick(Kick::tight().decay(0.35).drive(0.5))
            .snare(Snare::lofi())
            .clap(Clap::tr808().decay(0.18))
            .rim(Rimshot::tr808().decay(0.03))
            .closed_hat(HiHat::closed().tone(7500.0).noise(0.35).level(0.4))
            .pedal_hat(HiHat::pedal().tone(6500.0).level(0.3))
            .open_hat(
                HiHat::open()
                    .tone(7000.0)
                    .noise(0.35)
                    .decay(0.3)
                    .level(0.32),
            )
            .low_tom(Tom::low().decay(0.4))
            .mid_tom(Tom::mid().decay(0.35))
            .high_tom(Tom::high().decay(0.3))
            .crash(Cymbal::crash().tone(5500.0).decay(1.5))
            .ride(Cymbal::ride().tone(6500.0))
            .cowbell(Cowbell::tr808().level(0.3))
            .shaker(Shaker::new().tone(4500.0))
            .map_with(Self::TAMBOURINE, Shaker::tambourine())
            .default_pans()
            .crush(10.0, 3)
            .lowpass(7000.0)
            .drive(0.5)
    }

    fn default_pans(self) -> DrumMachine {
        self.pan(Self::CLOSED_HAT, 0.2)
            .pan(Self::OPEN_HAT, 0.2)
            .pan(Self::PEDAL_HAT, 0.2)
            .pan(Self::LOW_TOM, -0.3)
            .pan(Self::MID_TOM, 0.0)
            .pan(Self::HIGH_TOM, 0.3)
            .pan(Self::CRASH, -0.35)
            .pan(Self::RIDE, 0.35)
            .pan(Self::COWBELL, -0.15)
            .pan(Self::SHAKER, -0.25)
    }

    /// Assign a sound to several pitches (one shared pad), with a choke group (0 = none).
    fn set(mut self, pitches: &[u8], d: DrumSound, group: u8) -> DrumMachine {
        let mut pad = Pad::new(d);
        pad.group = group;
        // keep level/pan of the pad being replaced
        if let Some(old) = pitches.iter().filter_map(|&p| self.pad_of(p)).next() {
            pad.level = self.pads[old].level;
            pad.pan = self.pads[old].pan;
        }
        pad.v.iter_mut().for_each(|v| v.prepare(self.sr));
        self.pads.push(pad);
        let idx = (self.pads.len() - 1) as u16;
        for &p in pitches {
            if (p as usize) < 128 {
                self.map[p as usize] = idx;
            }
        }
        self
    }
    fn map_with(self, pitch: u8, d: impl Into<DrumSound>) -> DrumMachine {
        self.set(&[pitch], d.into(), 0)
    }
    fn pad_of(&self, pitch: u8) -> Option<usize> {
        self.map
            .get(pitch as usize)
            .filter(|&&i| i != NONE)
            .map(|&i| i as usize)
    }

    /// Map any drum sound to a pitch: `.map(60, Cowbell::new().tune(700.0))`.
    pub fn map(self, pitch: u8, sound: impl Into<DrumSound>) -> DrumMachine {
        self.map_with(pitch, sound)
    }
    /// Kick on 35 & 36.
    pub fn kick(self, k: Kick) -> DrumMachine {
        self.set(&[35, 36], k.into(), 0)
    }
    /// Snare on 38 & 40.
    pub fn snare(self, s: Snare) -> DrumMachine {
        self.set(&[38, 40], s.into(), 0)
    }
    /// Clap on 39.
    pub fn clap(self, c: Clap) -> DrumMachine {
        self.set(&[39], c.into(), 0)
    }
    /// Rimshot on 37.
    pub fn rim(self, r: Rimshot) -> DrumMachine {
        self.set(&[37], r.into(), 0)
    }
    /// Closed hat on 42 (chokes the open hat).
    pub fn closed_hat(self, h: HiHat) -> DrumMachine {
        self.set(&[42], h.into(), 1)
    }
    /// Alias of [`DrumMachine::closed_hat`].
    pub fn hat(self, h: HiHat) -> DrumMachine {
        self.closed_hat(h)
    }
    /// Pedal hat on 44 (chokes the open hat).
    pub fn pedal_hat(self, h: HiHat) -> DrumMachine {
        self.set(&[44], h.into(), 1)
    }
    /// Open hat on 46 (choke group with 42/44).
    pub fn open_hat(self, h: HiHat) -> DrumMachine {
        self.set(&[46], h.into(), 1)
    }
    /// Low tom on 41 & 43.
    pub fn low_tom(self, t: Tom) -> DrumMachine {
        self.set(&[41, 43], t.into(), 0)
    }
    /// Mid tom on 45 & 47.
    pub fn mid_tom(self, t: Tom) -> DrumMachine {
        self.set(&[45, 47], t.into(), 0)
    }
    /// High tom on 48 & 50.
    pub fn high_tom(self, t: Tom) -> DrumMachine {
        self.set(&[48, 50], t.into(), 0)
    }
    /// Crash on 49 & 57.
    pub fn crash(self, c: Cymbal) -> DrumMachine {
        self.set(&[49, 57], c.into(), 0)
    }
    /// Ride on 51 & 59.
    pub fn ride(self, c: Cymbal) -> DrumMachine {
        self.set(&[51, 59], c.into(), 0)
    }
    /// Cowbell on 56.
    pub fn cowbell(self, c: Cowbell) -> DrumMachine {
        self.set(&[56], c.into(), 0)
    }
    /// Shaker/maracas on 69 & 70.
    pub fn shaker(self, s: Shaker) -> DrumMachine {
        self.set(&[69, 70], s.into(), 0)
    }

    /// Level of the sound on `pitch` (linear or `-3.db()`).
    pub fn level(mut self, pitch: u8, g: impl Val) -> DrumMachine {
        if let Some(i) = self.pad_of(pitch) {
            self.pads[i].level = g.val();
        }
        self
    }
    /// Pan (-1..1) of the sound on `pitch`.
    pub fn pan(mut self, pitch: u8, p: f32) -> DrumMachine {
        if let Some(i) = self.pad_of(pitch) {
            self.pads[i].pan = p.clamp(-1.0, 1.0);
        }
        self
    }
    /// Put the sounds on `pitches` into a choke group (1..=255): hitting one silences the others.
    pub fn choke(mut self, pitches: &[u8], group: u8) -> DrumMachine {
        for &p in pitches {
            if let Some(i) = self.pad_of(p) {
                self.pads[i].group = group;
            }
        }
        self
    }
    /// Master gain (automatable).
    pub fn gain(mut self, g: impl Into<Param>) -> DrumMachine {
        self.gain = g.into();
        self
    }
    /// Saturation of the whole kit (0..2).
    pub fn drive(mut self, d: f32) -> DrumMachine {
        self.drive = d.max(0.0);
        self
    }
    /// Bit depth reduction (`bits`, 0 = off) and sample-rate reduction (hold every `down` samples).
    pub fn crush(mut self, bits: f32, down: u32) -> DrumMachine {
        self.bits = bits.max(0.0);
        self.down = down.max(1);
        self
    }
    /// Gentle 12 dB lowpass on the whole kit.
    pub fn lowpass(mut self, hz: f32) -> DrumMachine {
        self.lowpass = Some(hz);
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> DrumMachine {
        self.name = name.into();
        self
    }

    fn render_chunk(&mut self, out: &mut [Frame], ctx: &Ctx) {
        let n = out.len();
        let sr = self.sr;
        let mix = &mut self.mix[..n];
        clear(mix);
        let step = 1.0 / (CHOKE * sr);
        for pad in &mut self.pads {
            if !pad.alive[0] && !pad.alive[1] {
                continue;
            }
            let (gl, gr) = pan_gains(pad.pan);
            for k in 0..2 {
                if !pad.alive[k] {
                    continue;
                }
                let tmp = &mut self.tmp[..n];
                clear(tmp);
                pad.alive[k] = pad.v[k].render(tmp, ctx);
                for (m, t) in mix.iter_mut().zip(tmp.iter()) {
                    let mut g = pad.level;
                    if pad.choking[k] {
                        pad.fade[k] = (pad.fade[k] - step).max(0.0);
                        g *= pad.fade[k];
                    }
                    m.l += t.l * g * gl;
                    m.r += t.r * g * gr;
                }
                if pad.choking[k] && pad.fade[k] <= 0.0 {
                    pad.alive[k] = false;
                }
            }
        }
        let gain = ctx.p(&self.gain);
        self.ramp.start(gain, n);
        let dg = 1.0 + 3.0 * self.drive;
        let dnorm = 1.0 / soft_clip(dg);
        let q = if self.bits > 0.0 {
            2f32.powf(self.bits - 1.0)
        } else {
            0.0
        };
        for (o, m) in out.iter_mut().zip(mix.iter()) {
            let mut x = *m;
            if self.down > 1 {
                if self.hold_n == 0 {
                    self.hold = x;
                }
                self.hold_n = (self.hold_n + 1) % self.down;
                x = self.hold;
            }
            if q > 0.0 {
                x = x.map(|v| (v * q).round() / q);
            }
            if self.lowpass.is_some() {
                x = self.lp.process(x);
            }
            if self.drive > 0.0 {
                x = x.map(|v| soft_clip(v * dg) * dnorm);
            }
            *o += x * self.ramp.next();
        }
    }
}

impl Instrument for DrumMachine {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        for p in &mut self.pads {
            p.v.iter_mut().for_each(|v| v.prepare(sr));
        }
        if let Some(hz) = self.lowpass {
            self.lp = Biquad::new(BiquadKind::LowPass, hz, 0.707, sr);
        }
        self.tmp.resize(CHUNK, Frame::ZERO);
        self.mix.resize(CHUNK, Frame::ZERO);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        if note.vel <= 0.0 {
            return;
        }
        let pitch = note.pitch.round();
        if !(0.0..128.0).contains(&pitch) {
            return;
        }
        let Some(i) = self.pad_of(pitch as u8) else {
            return;
        };
        let group = self.pads[i].group;
        if group != 0 {
            for (j, p) in self.pads.iter_mut().enumerate() {
                if j != i && p.group == group {
                    p.choke();
                }
            }
        }
        let pad = &mut self.pads[i];
        // retrigger on the other slot; fade out the previous hit to avoid clicks
        if pad.alive[pad.cur] {
            pad.choking[pad.cur] = true;
        }
        let k = 1 - pad.cur;
        pad.cur = k;
        pad.alive[k] = true;
        pad.choking[k] = false;
        pad.fade[k] = 1.0;
        pad.v[k].start(note, ctx);
    }
    fn note_off(&mut self, _id: u64, _ctx: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        let mut pos = 0;
        while pos < out.len() {
            let n = CHUNK.min(out.len() - pos);
            let cx = ctx.advanced(pos);
            self.render_chunk(&mut out[pos..pos + n], &cx);
            pos += n;
        }
    }
    fn active(&self) -> bool {
        self.pads.iter().any(|p| p.alive[0] || p.alive[1])
    }
    fn panic(&mut self) {
        for p in &mut self.pads {
            p.alive = [false; 2];
        }
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::test_util::*;

    #[test]
    fn kits_play_every_gm_sound() {
        for mut kit in [
            DrumMachine::tr808(),
            DrumMachine::tr909(),
            DrumMachine::lofi(),
        ] {
            let pitches = [
                35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 53, 54, 56, 57,
                59, 69, 70,
            ];
            let notes: Vec<_> = pitches
                .iter()
                .enumerate()
                .map(|(i, &p)| (i as f32 * 0.15, p as f32, 0.9, 0.1))
                .collect();
            let out = play_notes(&mut kit, &notes, 8.0);
            assert!(finite(&out));
            for (i, _) in pitches.iter().enumerate() {
                let s = (i as f32 * 0.15 * 48_000.0) as usize;
                assert!(
                    peak(&out[s..s + 2400]) > 0.01,
                    "{} pitch {}",
                    kit.name(),
                    pitches[i]
                );
            }
            assert!(peak(&out) < 2.0);
            assert!(!kit.active(), "{} tail never ended", kit.name());
        }
    }

    #[test]
    fn closed_hat_chokes_open_hat() {
        let mut kit = DrumMachine::tr808();
        let out = play_notes(
            &mut kit,
            &[(0.0, 46.0, 1.0, 0.1), (0.1, 42.0, 1.0, 0.1)],
            0.6,
        );
        // after the closed hat has decayed, the open hat must be gone too
        let tail = &out[(0.3 * 48_000.0) as usize..];
        assert!(peak(tail) < 1e-3, "{}", peak(tail));
        let mut kit = DrumMachine::tr808();
        let out = play_notes(&mut kit, &[(0.0, 46.0, 1.0, 0.1)], 0.6);
        assert!(peak(&out[(0.3 * 48_000.0) as usize..]) > 1e-3);
    }

    #[test]
    fn unmapped_and_level() {
        let mut kit = DrumMachine::new().kick(Kick::tr909()).level(36, 0.5);
        let out = play_notes(&mut kit, &[(0.0, 100.0, 1.0, 0.1)], 0.2);
        assert_eq!(peak(&out), 0.0);
        let a = peak(&play(
            &mut DrumMachine::new().kick(Kick::tr909()),
            36.0,
            1.0,
            0.1,
            0.3,
        ));
        let b = peak(&play(
            &mut DrumMachine::new().kick(Kick::tr909()).level(36, 0.5),
            36.0,
            1.0,
            0.1,
            0.3,
        ));
        assert!((b / a - 0.5).abs() < 0.05);
    }

    #[test]
    fn only_plays_everywhere() {
        let mut kit = DrumMachine::only(Kick::tr808());
        assert!(peak(&play(&mut kit, 60.0, 1.0, 0.1, 0.3)) > 0.1);
    }
}
