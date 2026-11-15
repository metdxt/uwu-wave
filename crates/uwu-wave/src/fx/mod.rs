//! Effects: everything that processes a stereo buffer in place.
//!
//! | area | effects |
//! |---|---|
//! | filters | [`LowPass`], [`HighPass`], [`BandPass`], [`Notch`], [`Ladder`], [`Eq`](struct@Eq) |
//! | space | [`Reverb`], [`Delay`] |
//! | modulation | [`Chorus`], [`Flanger`], [`Phaser`], [`Tremolo`], [`AutoPan`] |
//! | colour | [`Distortion`], [`TapeSat`], [`Bitcrush`] |
//! | dynamics | [`Compressor`], [`Limiter`], [`Gate`], [`Ducker`], [`TransientShaper`] |
//! | utility | [`Utility`], [`Width`], [`Chain`] / [`chain!`](crate::chain), [`Parallel`], [`FnFx`] |
//!
//! Every knob takes `impl Into<Param>`: a number, `-6.db()`, an `auto![..]` curve, an `lfo::…`,
//! a [`Lane`](crate::param::Lane) or an expression. Levels (thresholds, ceilings, EQ gains) are
//! linear gains written with `.db()`; plain non-positive numbers are read as dB.
//!
//! ```ignore
//! let verb = song.bus("verb").fx(Reverb::hall().wet());
//! let bass = song.track("bass", bass)
//!     .fx(chain![Ladder::new(lfo::sine(2.bars()).range(300.0, 3000.0)).resonance(0.5), Distortion::tube(2.0)])
//!     .fx(Ducker::new("kick").amount(0.8).release(150.ms()));
//! song.master().fx(Eq::new().low_cut(30.0)).fx(Compressor::glue()).fx(Limiter::new().ceiling(-1.db()));
//! ```

use crate::ctx::Ctx;
use crate::dsp::Frame;

pub mod chain;
pub mod delay;
pub mod distortion;
pub mod dynamics;
pub mod eq;
pub mod filter;
pub mod meter;
pub mod modulation;
pub mod reverb;
pub mod util;
pub mod utility;

pub use chain::{Chain, Parallel};
pub use delay::Delay;
pub use distortion::{Bitcrush, DistMode, Distortion, TapeSat};
pub use dynamics::{Compressor, Ducker, Gate, Limiter, TransientShaper};
pub use eq::Eq;
pub use filter::{BandPass, HighPass, Ladder, LowPass, Notch};
pub use meter::Meter;
pub use modulation::{AutoPan, Chorus, Flanger, Phaser, Tremolo};
pub use reverb::Reverb;
pub use util::DelayTime;
pub use utility::{Utility, Width};

pub mod prelude {
    //! Effects re-exported by `uwu_wave::prelude`.
    pub use super::chain::{Chain, Parallel};
    pub use super::delay::Delay;
    pub use super::distortion::{Bitcrush, DistMode, Distortion, TapeSat};
    pub use super::dynamics::{Compressor, Ducker, Gate, Limiter, TransientShaper};
    pub use super::eq::Eq;
    pub use super::filter::{BandPass, HighPass, Ladder, LowPass, Notch};
    pub use super::modulation::{AutoPan, Chorus, Flanger, Phaser, Tremolo};
    pub use super::reverb::Reverb;
    pub use super::util::DelayTime;
    pub use super::utility::{Utility, Width};
    pub use super::{Effect, FnFx};
    pub use crate::chain;
}

/// The effect interface. Processes `buf` in place.
pub trait Effect: Send {
    /// Called once before rendering with the sample rate (allocate delay lines here).
    fn prepare(&mut self, _sr: f32) {}
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx);
    /// Clear internal state.
    fn reset(&mut self) {}
    /// How long (seconds) the effect keeps ringing after input stops.
    fn tail(&self) -> f32 {
        0.0
    }
    /// Name of a track/bus whose output this effect wants as sidechain input
    /// (delivered through [`Ctx::sidechain`]).
    fn sidechain(&self) -> Option<&str> {
        None
    }
    fn name(&self) -> String {
        "fx".into()
    }
}

impl Effect for Box<dyn Effect> {
    fn prepare(&mut self, sr: f32) {
        (**self).prepare(sr)
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        (**self).process(buf, ctx)
    }
    fn reset(&mut self) {
        (**self).reset()
    }
    fn tail(&self) -> f32 {
        (**self).tail()
    }
    fn sidechain(&self) -> Option<&str> {
        (**self).sidechain()
    }
    fn name(&self) -> String {
        (**self).name()
    }
}

/// A quick custom effect from a per-frame closure: `FnFx::new(|f, _ctx| f * 0.5)`.
pub struct FnFx<F: FnMut(Frame, &Ctx) -> Frame + Send>(pub F);
impl<F: FnMut(Frame, &Ctx) -> Frame + Send> FnFx<F> {
    pub fn new(f: F) -> Self {
        FnFx(f)
    }
}
impl<F: FnMut(Frame, &Ctx) -> Frame + Send> Effect for FnFx<F> {
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        for f in buf.iter_mut() {
            *f = (self.0)(*f, ctx);
        }
    }
    fn name(&self) -> String {
        "fn".into()
    }
}
