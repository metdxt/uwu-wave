//! Grouping: [`Chain`] (serial), [`Parallel`] (dry/wet blend of any effect) and [`chain!`](crate::chain).

use super::Effect;
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::engine::MAX_BLOCK;
use crate::param::Param;

/// Several effects in series, usable anywhere a single effect is expected.
///
/// ```ignore
/// let vocal_chain = Chain::new()
///     .then(HighPass::new(90.0))
///     .then(Compressor::new())
///     .then(Reverb::plate().mix(0.2));
/// let same = chain![HighPass::new(90.0), Compressor::new(), Reverb::plate().mix(0.2)];
/// ```
///
/// Sidechains: the chain reports the first sidechain source declared by one of its effects;
/// only effects declaring that same source receive it.
pub struct Chain {
    fx: Vec<Box<dyn Effect>>,
    label: Option<String>,
}

impl Chain {
    pub fn new() -> Chain {
        Chain {
            fx: vec![],
            label: None,
        }
    }
    /// Append an effect.
    pub fn then(mut self, e: impl Effect + 'static) -> Chain {
        self.fx.push(Box::new(e));
        self
    }
    /// Append an effect in place.
    pub fn push(&mut self, e: impl Effect + 'static) -> &mut Chain {
        self.fx.push(Box::new(e));
        self
    }
    /// Give the chain a name (shown in reports/debugging).
    pub fn named(mut self, name: impl Into<String>) -> Chain {
        self.label = Some(name.into());
        self
    }
    pub fn len(&self) -> usize {
        self.fx.len()
    }
    pub fn is_empty(&self) -> bool {
        self.fx.is_empty()
    }
}

impl Effect for Chain {
    fn prepare(&mut self, sr: f32) {
        self.fx.iter_mut().for_each(|f| f.prepare(sr));
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let src = self
            .fx
            .iter()
            .find_map(|f| f.sidechain())
            .map(|s| s.to_owned());
        for f in &mut self.fx {
            let wants = f.sidechain().is_some() && f.sidechain() == src.as_deref();
            if wants || ctx.sidechain.is_none() {
                f.process(buf, ctx);
            } else {
                f.process(
                    buf,
                    &Ctx {
                        sidechain: None,
                        ..*ctx
                    },
                );
            }
        }
    }
    fn reset(&mut self) {
        self.fx.iter_mut().for_each(|f| f.reset());
    }
    fn tail(&self) -> f32 {
        self.fx.iter().map(|f| f.tail()).sum()
    }
    fn sidechain(&self) -> Option<&str> {
        self.fx.iter().find_map(|f| f.sidechain())
    }
    fn name(&self) -> String {
        match &self.label {
            Some(l) => l.clone(),
            None => format!(
                "chain[{}]",
                self.fx
                    .iter()
                    .map(|f| f.name())
                    .collect::<Vec<_>>()
                    .join(" > ")
            ),
        }
    }
}

/// Build a [`Chain`]: `chain![HighPass::new(80.0), Compressor::new(), Delay::new(eighth())]`.
#[macro_export]
macro_rules! chain {
    ($($e:expr),* $(,)?) => {
        $crate::fx::chain::Chain::new()$(.then($e))*
    };
}

/// Run an effect in parallel with the dry signal and blend them (linear crossfade).
///
/// ```ignore
/// Parallel::new(Compressor::new().ratio(10.0).threshold(-30.db())).mix(0.35)   // NY compression
/// Parallel::new(Distortion::fuzz(20.0)).mix(lfo::sine(2.bars()).range(0.0, 0.5))
/// ```
///
/// The dry path is not latency-compensated, so avoid wrapping look-ahead effects (e.g. [`Limiter`](super::Limiter)).
pub struct Parallel<E: Effect> {
    fx: E,
    mix: Param,
    scratch: [Frame; MAX_BLOCK],
    dry_r: Ramp,
    wet_r: Ramp,
}

impl<E: Effect> Parallel<E> {
    /// 50/50 blend by default.
    pub fn new(fx: E) -> Parallel<E> {
        Parallel {
            fx,
            mix: Param::Const(0.5),
            scratch: [Frame::ZERO; MAX_BLOCK],
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
        }
    }
    /// Wet amount 0..1, automatable.
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// The wrapped effect.
    pub fn inner(&mut self) -> &mut E {
        &mut self.fx
    }
}

impl<E: Effect> Effect for Parallel<E> {
    fn prepare(&mut self, sr: f32) {
        self.fx.prepare(sr)
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let mix = ctx.p(&self.mix).clamp(0.0, 1.0);
        let mut off = 0;
        for chunk in buf.chunks_mut(MAX_BLOCK) {
            let n = chunk.len();
            let mut cx = ctx.advanced(off);
            cx.sidechain = ctx.sidechain.and_then(|s| s.get(off..off + n));
            self.scratch[..n].copy_from_slice(chunk);
            self.fx.process(chunk, &cx);
            self.dry_r.start(1.0 - mix, n);
            self.wet_r.start(mix, n);
            for (w, d) in chunk.iter_mut().zip(&self.scratch[..n]) {
                *w = *d * self.dry_r.next() + *w * self.wet_r.next();
            }
            off += n;
        }
    }
    fn reset(&mut self) {
        self.fx.reset()
    }
    fn tail(&self) -> f32 {
        self.fx.tail()
    }
    fn sidechain(&self) -> Option<&str> {
        self.fx.sidechain()
    }
    fn name(&self) -> String {
        format!("parallel({})", self.fx.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Compressor, FnFx, Utility};

    #[test]
    fn chain_runs_in_order_and_reports_sidechain() {
        let mut c = crate::chain![
            FnFx::new(|f, _| f + Frame::mono(1.0)),
            FnFx::new(|f, _| f * 2.0)
        ];
        let ctx = Ctx::offline(48_000.0);
        let mut buf = [Frame::ZERO; 16];
        c.process(&mut buf, &ctx);
        assert_eq!(buf[3], Frame::mono(2.0));
        let c = Chain::new()
            .then(Utility::new())
            .then(Compressor::new().sidechain("kick"));
        assert_eq!(Effect::sidechain(&c), Some("kick"));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn parallel_blends() {
        let mut p = Parallel::new(FnFx::new(|_, _| Frame::ZERO)).mix(0.25);
        let ctx = Ctx::offline(48_000.0);
        let mut buf = vec![Frame::mono(1.0); 200];
        p.process(&mut buf, &ctx);
        assert!((buf[150].l - 0.75).abs() < 1e-6);
    }
}
