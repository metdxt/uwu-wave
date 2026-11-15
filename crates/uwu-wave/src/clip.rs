//! Notes, clips and the [`Seq`] trait that everything you can place on a track implements.

use crate::time::Time;
use std::sync::Arc;

/// A note on a clip-local timeline (beats). `pitch` is a (fractional) MIDI note number;
/// drum kits map pitches to sounds (GM layout: 36 kick, 38 snare, 42 closed hat...).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub start: f64,
    pub dur: f64,
    pub pitch: f32,
    /// Velocity 0..1.
    pub vel: f32,
}

impl Note {
    pub fn new(start: impl Into<Time>, dur: impl Into<Time>, pitch: f32, vel: f32) -> Note {
        Note { start: start.into().0, dur: dur.into().0, pitch, vel }
    }
    pub fn end(&self) -> f64 {
        self.start + self.dur
    }
}

/// Anything that can produce notes on a looping timeline: clips, patterns, generators...
pub trait Seq: Send + Sync {
    /// Natural length in beats (one loop iteration).
    fn len(&self) -> f64;
    /// All notes whose start lies in `[from, to)`, in local beats (0 = sequence start). The
    /// sequence is conceptually repeated forever, so `to` may exceed `len()`.
    fn notes(&self, from: f64, to: f64) -> Vec<Note>;
}

/// Conversion into a shareable [`Seq`]. Implemented for every `Seq`, and for strings (parsed
/// with the pattern mini-notation).
pub trait IntoSeq {
    fn into_seq(self) -> Arc<dyn Seq>;
}

impl<T: Seq + 'static> IntoSeq for T {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(self)
    }
}

/// Borrowed clips are cloned, so you can write `s.play(&bass, &riff)` without `.clone()`.
impl IntoSeq for &Clip {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(self.clone())
    }
}

impl IntoSeq for &Arc<dyn Seq> {
    fn into_seq(self) -> Arc<dyn Seq> {
        self.clone()
    }
}

impl IntoSeq for Arc<dyn Seq> {
    fn into_seq(self) -> Arc<dyn Seq> {
        self
    }
}

/// A fixed list of notes with a loop length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub notes: Vec<Note>,
    pub len: f64,
}

impl Clip {
    /// Empty clip of a given length.
    pub fn new(len: impl Into<Time>) -> Clip {
        Clip { notes: vec![], len: len.into().0 }
    }
    /// Builder: add a note.
    pub fn note(mut self, start: impl Into<Time>, dur: impl Into<Time>, pitch: impl Into<f32Pitch>, vel: f32) -> Clip {
        self.notes.push(Note::new(start, dur, pitch.into().0, vel));
        self
    }
    /// Push a note.
    pub fn push(&mut self, n: Note) {
        self.notes.push(n);
    }
    /// Bake any sequence into a clip covering `len` beats.
    pub fn bake(seq: &dyn Seq, len: impl Into<Time>) -> Clip {
        let len = len.into().0;
        Clip { notes: seq.notes(0.0, len), len }
    }
    pub fn sorted(mut self) -> Clip {
        self.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        self
    }
    /// Transpose by semitones.
    pub fn transpose(mut self, st: f32) -> Clip {
        self.notes.iter_mut().for_each(|n| n.pitch += st);
        self
    }
    /// Scale velocities.
    pub fn vel(mut self, k: f32) -> Clip {
        self.notes.iter_mut().for_each(|n| n.vel = (n.vel * k).clamp(0.0, 1.0));
        self
    }
    /// Play the clip back `k` times faster.
    pub fn fast(mut self, k: f64) -> Clip {
        self.notes.iter_mut().for_each(|n| {
            n.start /= k;
            n.dur /= k;
        });
        self.len /= k;
        self
    }
    /// Concatenate in time.
    pub fn then(mut self, other: Clip) -> Clip {
        let off = self.len;
        self.notes.extend(other.notes.into_iter().map(|mut n| {
            n.start += off;
            n
        }));
        self.len += other.len;
        self
    }
    /// Layer (play simultaneously). Length is the max of both.
    pub fn with(mut self, other: Clip) -> Clip {
        self.len = self.len.max(other.len);
        self.notes.extend(other.notes);
        self
    }
    /// Repeat `k` times.
    pub fn times(self, k: usize) -> Clip {
        let len = self.len;
        Clip { notes: self.notes(0.0, len * k as f64), len: len * k as f64 }
    }
}

impl Seq for Clip {
    fn len(&self) -> f64 {
        self.len
    }
    fn notes(&self, from: f64, to: f64) -> Vec<Note> {
        let mut out = vec![];
        if self.len <= 0.0 || to <= from {
            return out;
        }
        let k0 = (from / self.len).floor() as i64;
        let k1 = (to / self.len).ceil() as i64;
        for k in k0..=k1 {
            let off = k as f64 * self.len;
            for n in &self.notes {
                let s = n.start + off;
                if s >= from && s < to && n.start < self.len {
                    out.push(Note { start: s, ..*n });
                }
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    }
}

/// Helper so `Clip::note` accepts both raw MIDI numbers and pitch types.
#[derive(Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
pub struct f32Pitch(pub f32);
macro_rules! pitch_from { ($($t:ty),*) => { $(impl From<$t> for f32Pitch { fn from(v: $t) -> f32Pitch { f32Pitch(v as f32) } })* } }
pitch_from!(f32, f64, i32, u8, u32);

impl std::ops::Add for Clip {
    type Output = Clip;
    /// `a + b`: play `b` after `a`.
    fn add(self, o: Clip) -> Clip {
        self.then(o)
    }
}
impl std::ops::BitAnd for Clip {
    type Output = Clip;
    /// `a & b`: layer.
    fn bitand(self, o: Clip) -> Clip {
        self.with(o)
    }
}
impl std::ops::Mul<usize> for Clip {
    type Output = Clip;
    /// `a * 4`: repeat.
    fn mul(self, k: usize) -> Clip {
        self.times(k)
    }
}

/// An empty sequence (silence) of a given length.
pub fn rest(len: impl Into<Time>) -> Clip {
    Clip::new(len)
}
