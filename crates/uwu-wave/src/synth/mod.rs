//! Synthesizers: virtual-analog [`Synth`], [`Fm`], Karplus–Strong [`Pluck`], drawbar [`Organ`],
//! and an analog-style drum machine ([`DrumMachine`] built from [`Kick`], [`Snare`], [`Clap`],
//! [`HiHat`], [`Tom`], [`Cymbal`], [`Cowbell`], [`Rimshot`], [`Shaker`]).
//!
//! Every instrument is a builder: start from a preset and tweak it.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let song = Song::new("synths").bpm(124);
//! let bass = song.track("bass", Synth::acid().cutoff(lfo::sine(2.bars()).range(250.0, 1800.0)).res(0.8));
//! let pad = song.track("pad", Synth::pad().cutoff(2400.0));
//! let keys = song.track("keys", Fm::epiano());
//! let gtr = song.track("gtr", Pluck::guitar());
//! let drums = song.track("drums", DrumMachine::tr909());
//! ```
//!
//! Knobs that make sense to automate (cutoff, resonance, drive, gain, FM index, rotary speed...)
//! accept `impl Into<Param>`: constants, `auto![...]` curves, lanes, LFOs or expressions. They are
//! evaluated once per block by the instrument and smoothed inside the voices.

mod analog;
mod drums;
mod fm;
mod machine;
mod organ;
mod pluck;

pub use analog::{OscSpec, Synth, SynthVoice, Vcf};
pub use drums::{Clap, Cowbell, Cymbal, DrumSound, HiHat, Kick, Rimshot, Shaker, Snare, Tom};
pub use fm::{Fm, FmAlgo, FmOp, FmVoice};
pub use machine::DrumMachine;
pub use organ::{Organ, OrganVoice};
pub use pluck::{Pluck, PluckVoice};

pub mod prelude {
    //! Synth names re-exported by `uwu_wave::prelude`.
    pub use super::{
        Clap, Cowbell, Cymbal, DrumMachine, DrumSound, Fm, FmAlgo, FmOp, HiHat, Kick, Organ,
        OscSpec, Pluck, Rimshot, Shaker, Snare, Synth, Tom,
    };
    pub use crate::dsp::Wave;
}

use crate::dsp::Frame;
use std::sync::atomic::{AtomicU32, Ordering};

/// Frames processed per chunk when an instrument needs a private scratch buffer.
pub(crate) const CHUNK: usize = 64;

/// A lock-free `f32` cell used to hand block-rate parameter values from an instrument to its
/// voices (voices share it through an `Arc`).
#[derive(Debug, Default)]
pub(crate) struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub(crate) fn new(v: f32) -> AtomicF32 {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    #[inline]
    pub(crate) fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    #[inline]
    pub(crate) fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// Velocity → amplitude. `sens` 0 = ignore velocity, 1 = full range (roughly -12 dB at vel 0.5).
#[inline]
pub(crate) fn vel_amp(vel: f32, sens: f32) -> f32 {
    let v = vel.clamp(0.0, 1.0);
    let curve = 0.35 * v + 0.65 * v * v;
    1.0 - sens.clamp(0.0, 1.0) * (1.0 - curve)
}

/// Exponential decay coefficient reaching -60 dB after `t` seconds.
#[inline]
pub(crate) fn t60(t: f32, sr: f32) -> f32 {
    if t <= 0.0 {
        0.0
    } else {
        (-6.907_755 / (t * sr)).exp()
    }
}

/// Fill `buf` with zeros.
#[inline]
pub(crate) fn clear(buf: &mut [Frame]) {
    buf.iter_mut().for_each(|f| *f = Frame::ZERO);
}

#[cfg(test)]
pub(crate) mod test_util {
    use crate::ctx::Ctx;
    use crate::dsp::Frame;
    use crate::instrument::{Instrument, NoteOn};

    /// Play one note on an instrument for `secs` (note-off after `hold` seconds) and return the
    /// rendered audio.
    pub fn play(
        inst: &mut dyn Instrument,
        pitch: f32,
        vel: f32,
        hold: f32,
        secs: f32,
    ) -> Vec<Frame> {
        play_notes(inst, &[(0.0, pitch, vel, hold)], secs)
    }

    /// Play several notes `(start_secs, pitch, vel, hold_secs)` and return the rendered audio.
    pub fn play_notes(
        inst: &mut dyn Instrument,
        notes: &[(f32, f32, f32, f32)],
        secs: f32,
    ) -> Vec<Frame> {
        let sr = 48_000.0;
        inst.prepare(sr);
        let total = (secs * sr) as usize;
        // (frame, on?, idx)
        let mut evs: Vec<(usize, bool, usize)> = vec![];
        for (i, n) in notes.iter().enumerate() {
            evs.push(((n.0 * sr) as usize, true, i));
            evs.push((((n.0 + n.3) * sr) as usize, false, i));
        }
        evs.sort_by_key(|e| (e.0, e.1));
        let mut out = vec![Frame::ZERO; total];
        let mut ctx = Ctx::offline(sr);
        let mut pos = 0;
        let mut ei = 0;
        while pos < total {
            while ei < evs.len() && evs[ei].0 <= pos {
                let (_, on, i) = evs[ei];
                let n = notes[i];
                if on {
                    inst.note_on(
                        &NoteOn {
                            id: i as u64 + 1,
                            pitch: n.1,
                            vel: n.2,
                            dur: n.3,
                        },
                        &ctx,
                    );
                } else {
                    inst.note_off(i as u64 + 1, &ctx);
                }
                ei += 1;
            }
            let mut n = 64.min(total - pos);
            if ei < evs.len() {
                n = n.min(evs[ei].0.saturating_sub(pos).max(1));
            }
            inst.render(&mut out[pos..pos + n], &ctx);
            ctx = ctx.advanced(n);
            pos += n;
        }
        out
    }

    pub fn peak(buf: &[Frame]) -> f32 {
        buf.iter().fold(0.0f32, |m, f| m.max(f.peak()))
    }

    pub fn rms(buf: &[Frame]) -> f32 {
        if buf.is_empty() {
            return 0.0;
        }
        (buf.iter().map(|f| f.l * f.l + f.r * f.r).sum::<f32>() / (2.0 * buf.len() as f32)).sqrt()
    }

    pub fn finite(buf: &[Frame]) -> bool {
        buf.iter().all(|f| f.l.is_finite() && f.r.is_finite())
    }
}
