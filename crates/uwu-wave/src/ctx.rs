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
    /// Track mod wheel 0..1 (from [`Track::modwheel`](crate::song::Track::modwheel)); instruments
    /// map it to something expressive (vibrato depth, brightness...).
    pub modwheel: f32,
}

impl<'a> Ctx<'a> {
    /// A context for tests/offline use.
    pub fn offline(sr: f32) -> Ctx<'static> {
        Ctx { sr, clock: Clock::default(), bpm: 120.0, frame: 0, sidechain: None, sidechains: &[], bend: 0.0, modwheel: 0.0 }
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
