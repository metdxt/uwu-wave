//! Processing context handed to instruments and effects for every block.

use crate::dsp::Frame;
use crate::param::{Clock, Param};
use crate::time::Time;

/// Everything a processor needs to know about "now".
///
/// Blocks are at most [`crate::engine::MAX_BLOCK`] frames. `clock` is the position of the
/// block's *first* frame. Evaluate parameters with [`Ctx::p`].
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    /// Sample rate (Hz).
    pub sr: f32,
    /// Timeline position of the first frame in this block.
    pub clock: Clock,
    /// Current tempo.
    pub bpm: f64,
    /// Absolute frame index of the first frame of this block.
    pub frame: u64,
    /// Sidechain input for this block (same length as the processed buffer), if this effect
    /// declared one via [`Effect::sidechain`](crate::fx::Effect::sidechain).
    pub sidechain: Option<&'a [Frame]>,
    /// Every sidechain input of this insert by source name (see [`Effect::sidechains`](crate::fx::Effect::sidechains)).
    pub sidechains: &'a [(&'a str, &'a [Frame])],
    /// Track pitch bend in semitones (from [`Track::bend`](crate::song::Track::bend)); instruments
    /// add it to every sounding note's pitch.
    pub bend: f32,
    /// All 128 MIDI-style continuous controllers of the track, 0..1 (see [`Track::cc`](crate::song::Track::cc)).
    /// Defaults: everything 0 except CC7 (volume) and CC11 (expression), which are 1.
    pub cc: &'a [f32; 128],
    /// Track mod wheel 0..1 (from [`Track::modwheel`](crate::song::Track::modwheel)); instruments
    /// map it to something expressive (vibrato depth, brightness...).
    pub modwheel: f32,
}

/// Default controller values (CC7 and CC11 at full, everything else 0).
pub static DEFAULT_CC: [f32; 128] = {
    let mut a = [0.0f32; 128];
    a[7] = 1.0;
    a[11] = 1.0;
    a
};

/// Well-known controller numbers.
pub mod cc {
    /// Mod wheel – orchestral instruments use it as **dynamics** (layer crossfade).
    pub const MOD: u8 = 1;
    pub const DYNAMICS: u8 = 1;
    pub const BREATH: u8 = 2;
    pub const VOLUME: u8 = 7;
    pub const PAN: u8 = 10;
    /// Expression: fine loudness (swells, hairpins) inside a dynamic.
    pub const EXPRESSION: u8 = 11;
    /// Vibrato depth (orchestral instruments).
    pub const VIBRATO: u8 = 21;
    pub const SUSTAIN: u8 = 64;
    /// Legato on/off (≥ 0.5 = on) for instruments with a legato engine.
    pub const LEGATO: u8 = 68;
    pub const BRIGHTNESS: u8 = 74;
}

impl<'a> Ctx<'a> {
    /// Controller value 0..1.
    #[inline]
    pub fn cc(&self, n: u8) -> f32 {
        self.cc[(n & 127) as usize]
    }
    /// CC11 expression 0..1 (1 = full).
    #[inline]
    pub fn expression(&self) -> f32 {
        self.cc[11]
    }
    /// CC1 dynamics 0..1 (same as `modwheel`).
    #[inline]
    pub fn dynamics(&self) -> f32 {
        self.modwheel
    }
    /// A context for tests/offline use.
    pub fn offline(sr: f32) -> Ctx<'static> {
        Ctx { sr, clock: Clock::default(), bpm: 120.0, frame: 0, sidechain: None, sidechains: &[], cc: &DEFAULT_CC, bend: 0.0, modwheel: 0.0 }
    }
    /// The sidechain buffer coming from the track/bus called `name`, if this insert requested it.
    pub fn sidechain_from(&self, name: &str) -> Option<&'a [Frame]> {
        self.sidechains.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
    }
    /// Evaluate a parameter now.
    #[inline]
    pub fn p(&self, p: &Param) -> f32 {
        p.at(self.clock)
    }
    #[inline]
    pub fn beat(&self) -> f64 {
        self.clock.beat
    }
    #[inline]
    pub fn secs(&self) -> f64 {
        self.clock.secs
    }
    /// Seconds per beat at the current tempo.
    #[inline]
    pub fn secs_per_beat(&self) -> f64 {
        60.0 / self.bpm
    }
    /// Convert a musical duration to seconds at the current tempo (for tempo-synced delays etc).
    #[inline]
    pub fn to_secs(&self, t: Time) -> f32 {
        (t.0 * 60.0 / self.bpm) as f32
    }
    /// Beats advanced per sample.
    #[inline]
    pub fn beats_per_sample(&self) -> f64 {
        self.bpm / 60.0 / self.sr as f64
    }
    /// Context advanced by `n` frames (same tempo).
    pub fn advanced(&self, n: usize) -> Ctx<'a> {
        let mut c = *self;
        c.frame += n as u64;
        c.clock.beat += n as f64 * self.beats_per_sample();
        c.clock.secs += n as f64 / self.sr as f64;
        c
    }
}
