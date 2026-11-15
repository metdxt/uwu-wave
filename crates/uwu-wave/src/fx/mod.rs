//! Effects: everything that processes a stereo buffer in place.

use crate::ctx::Ctx;
use crate::dsp::Frame;

pub mod meter;
pub mod prelude {
    //! Effects re-exported by `uwu_wave::prelude`.
    pub use super::{Effect, FnFx};
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
