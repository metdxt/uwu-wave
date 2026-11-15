//! Grouping: [`Chain`] (serial), [`Parallel`] (dry/wet blend of any effect) and [`chain!`](crate::chain).

use super::Effect;
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::engine::MAX_BLOCK;
use crate::param::Param;

/// Upper bound on named sidechain buffers re-sliced by [`Parallel`] for oversized buffers
/// (the engine delivers at most 8 per insert).
const MAX_SIDECHAINS: usize = 16;

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
/// Latency: the chain reports the sum of its effects' [`Effect::latency`].
///
/// Sidechains: the chain requests every source its effects declare ([`Effect::sidechains`]) and
/// hands each effect a context whose [`Ctx::sidechain`] is that effect's own source, so
/// `chain![Ducker::new("kick"), Compressor::new().sidechain("snare")]` just works.
pub struct Chain {
    fx: Vec<Box<dyn Effect>>,
    label: Option<String>,
    /// Per effect: its sidechain source (cached so processing never allocates).
    sources: Vec<Option<String>>,
}

impl Chain {
    pub fn new() -> Chain {
        Chain {
            fx: vec![],
            label: None,
            sources: vec![],
        }
    }
    /// Append an effect.
    pub fn then(mut self, e: impl Effect + 'static) -> Chain {
        self.push(e);
        self
    }
    /// Append an effect in place.
    pub fn push(&mut self, e: impl Effect + 'static) -> &mut Chain {
        self.sources.push(e.sidechain().map(str::to_owned));
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
        self.sources = self
            .fx
            .iter()
            .map(|f| f.sidechain().map(str::to_owned))
            .collect();
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let first = self.sources.iter().flatten().next().map(String::as_str);
        for (f, src) in self.fx.iter_mut().zip(&self.sources) {
            let sc = match src.as_deref() {
                None => None,
                // engine-provided named buffers; fall back to the single legacy buffer for the
                // first source (e.g. a hand-built Ctx)
                Some(name) => ctx.sidechain_from(name).or(if Some(name) == first {
                    ctx.sidechain
                } else {
                    None
                }),
            };
            f.process(
                buf,
                &Ctx {
                    sidechain: sc,
                    ..*ctx
                },
            );
        }
    }
    fn reset(&mut self) {
        self.fx.iter_mut().for_each(|f| f.reset());
    }
    fn tail(&self) -> f32 {
        self.fx.iter().map(|f| f.tail()).sum()
    }
    fn latency(&self) -> usize {
        self.fx.iter().map(|f| f.latency()).sum()
    }
    fn sidechain(&self) -> Option<&str> {
        self.fx.iter().find_map(|f| f.sidechain())
    }
    fn sidechains(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for s in self.fx.iter().flat_map(|f| f.sidechains()) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
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
/// The dry path is delayed by the wrapped effect's [`Effect::latency`] so both paths stay
/// sample-aligned (e.g. around a [`Limiter`](super::Limiter) or an oversampled
/// [`Distortion`](super::Distortion)); the latency is reported to the engine unchanged.
pub struct Parallel<E: Effect> {
    fx: E,
    mix: Param,
    scratch: [Frame; MAX_BLOCK],
    dry_delay: Vec<Frame>,
    dpos: usize,
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
            dry_delay: vec![],
            dpos: 0,
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
        self.fx.prepare(sr);
        let lat = self.fx.latency();
        self.dry_delay = if lat > 0 {
            vec![Frame::ZERO; lat + 1]
        } else {
            vec![]
        };
        self.dpos = 0;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let mix = ctx.p(&self.mix).clamp(0.0, 1.0);
        let dl = self.dry_delay.len();
        let mut off = 0;
        for chunk in buf.chunks_mut(MAX_BLOCK) {
            let n = chunk.len();
            let mut named: [(&str, &[Frame]); MAX_SIDECHAINS] = [("", &[]); MAX_SIDECHAINS];
            let mut cx = ctx.advanced(off);
            if off > 0 {
                // re-slice every sidechain buffer to this chunk (stack only, no allocation)
                cx.sidechain = ctx.sidechain.and_then(|s| s.get(off..off + n));
                let k = ctx.sidechains.len().min(MAX_SIDECHAINS);
                for (dst, (name, b)) in named.iter_mut().zip(&ctx.sidechains[..k]) {
                    *dst = (name, b.get(off..off + n).unwrap_or(&[]));
                }
                cx.sidechains = &named[..k];
            }
            self.scratch[..n].copy_from_slice(chunk);
            if dl > 0 {
                // ring of latency+1 frames: write now, read the frame written `latency` ago
                for d in &mut self.scratch[..n] {
                    self.dry_delay[self.dpos] = *d;
                    self.dpos = (self.dpos + 1) % dl;
                    *d = self.dry_delay[self.dpos];
                }
            }
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
        self.fx.reset();
        self.dry_delay.iter_mut().for_each(|f| *f = Frame::ZERO);
    }
    fn tail(&self) -> f32 {
        self.fx.tail()
    }
    fn latency(&self) -> usize {
        self.fx.latency()
    }
    fn sidechain(&self) -> Option<&str> {
        self.fx.sidechain()
    }
    fn sidechains(&self) -> Vec<String> {
        self.fx.sidechains()
    }
    fn name(&self) -> String {
        format!("parallel({})", self.fx.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Compressor, Distortion, Ducker, FnFx, Limiter, Utility};

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

    #[test]
    fn latency_is_reported_and_summed() {
        let mut l = Limiter::new().lookahead(0.002);
        l.prepare(48_000.0);
        assert_eq!(l.latency(), 95);
        let mut d = Distortion::soft(2.0);
        d.prepare(48_000.0);
        assert_eq!(d.latency(), crate::fx::util::Os2x::LATENCY);
        assert_eq!(Distortion::soft(2.0).oversample(false).latency(), 0);
        let mut c = crate::chain![
            Distortion::soft(2.0),
            Utility::new(),
            Limiter::new().lookahead(0.002)
        ];
        c.prepare(48_000.0);
        assert_eq!(c.latency(), 95 + crate::fx::util::Os2x::LATENCY);
        let b: Box<dyn Effect> = Box::new(c);
        assert_eq!(b.latency(), 95 + crate::fx::util::Os2x::LATENCY);
    }

    #[test]
    fn parallel_dry_path_is_aligned_with_latency() {
        // a pure-delay "effect" with declared latency: dry and wet must line up exactly
        struct Lag(Vec<Frame>, usize);
        impl Effect for Lag {
            fn process(&mut self, buf: &mut [Frame], _: &Ctx) {
                for f in buf.iter_mut() {
                    self.0[self.1] = *f;
                    self.1 = (self.1 + 1) % self.0.len();
                    *f = self.0[self.1];
                }
            }
            fn latency(&self) -> usize {
                self.0.len() - 1
            }
        }
        let mut p = Parallel::new(Lag(vec![Frame::ZERO; 11], 0)).mix(0.5);
        p.prepare(48_000.0);
        assert_eq!(p.latency(), 10);
        let ctx = Ctx::offline(48_000.0);
        let input: Vec<Frame> = (0..256)
            .map(|i| Frame::mono((i as f32 * 0.37).sin()))
            .collect();
        let mut buf = input.clone();
        p.process(&mut buf, &ctx);
        for i in 10..256 {
            assert!((buf[i].l - input[i - 10].l).abs() < 1e-6, "{i}");
        }
    }

    #[test]
    fn chain_routes_each_sidechain_to_its_effect() {
        let mut c = crate::chain![
            Ducker::new("kick").amount(1.0).attack(0.0).release(0.0),
            Utility::new(),
            Parallel::new(Ducker::new("snare").amount(1.0).attack(0.0).release(0.0)).mix(1.0),
            Ducker::new("kick").amount(0.0),
        ];
        assert_eq!(
            c.sidechains(),
            vec!["kick".to_string(), "snare".to_string()]
        );
        assert_eq!(Effect::sidechain(&c), Some("kick"));
        let boxed: Box<dyn Effect> = Box::new(Chain::new().then(Ducker::new("hat")));
        assert_eq!(boxed.sidechains(), vec!["hat".to_string()]);
        c.prepare(48_000.0);

        let loud = [Frame::mono(1.0); 64];
        let quiet = [Frame::ZERO; 64];
        let run = |c: &mut Chain, kick: &[Frame], snare: &[Frame]| {
            let named = [("kick", kick), ("snare", snare)];
            let mut ctx = Ctx::offline(48_000.0);
            ctx.sidechain = Some(kick);
            ctx.sidechains = &named;
            let mut buf = [Frame::mono(0.5); 64];
            c.process(&mut buf, &ctx);
            buf[63].l
        };
        // nothing playing: signal passes
        assert!((run(&mut c, &quiet, &quiet) - 0.5).abs() < 1e-3);
        // only the snare plays: the snare-keyed ducker (inside Parallel) must duck
        assert!(run(&mut c, &quiet, &loud) < 0.01);
        // recover, then only the kick plays: the kick-keyed ducker ducks
        for _ in 0..60 {
            run(&mut c, &quiet, &quiet); // let the detector envelope fall (~80 ms)
        }
        assert!((run(&mut c, &quiet, &quiet) - 0.5).abs() < 1e-3);
        assert!(run(&mut c, &loud, &quiet) < 0.01);
    }
}
