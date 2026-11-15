//! Parametric equaliser.

use super::Effect;
use super::filter::{SUB, SvfMode, SvfStage};
use super::util::{Smooth, level_db};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::Param;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    LowCut(usize),
    HighCut(usize),
    LowShelf,
    HighShelf,
    Peak,
    Notch,
}

#[derive(Clone, Debug)]
struct Band {
    kind: Kind,
    freq: Param,
    gain: Param,
    q: Param,
    st: [SvfStage; 4],
    sm_f: Smooth,
    sm_g: Smooth,
    sm_q: Smooth,
    last: (f32, f32, f32),
}

impl Band {
    fn new(kind: Kind, freq: Param, gain: Param, q: Param) -> Band {
        Band {
            kind,
            freq,
            gain,
            q,
            st: [SvfStage::default(); 4],
            sm_f: Smooth::default(),
            sm_g: Smooth::default(),
            sm_q: Smooth::default(),
            last: (f32::NAN, f32::NAN, f32::NAN),
        }
    }
    fn stages(&self) -> usize {
        match self.kind {
            Kind::LowCut(n) | Kind::HighCut(n) => n,
            _ => 1,
        }
    }
    fn update(&mut self, sr: f32, lf: f32, db: f32, q: f32) {
        if (lf, db, q) == self.last {
            return;
        }
        self.last = (lf, db, q);
        let f = lf.exp2();
        let n = self.stages();
        for i in 0..n {
            let (mode, sq) = match self.kind {
                Kind::LowCut(_) => (SvfMode::HighPass, butter_q(n, i)),
                Kind::HighCut(_) => (SvfMode::LowPass, butter_q(n, i)),
                Kind::LowShelf => (SvfMode::LowShelf(db), q),
                Kind::HighShelf => (SvfMode::HighShelf(db), q),
                Kind::Peak => (SvfMode::Peak(db), q),
                Kind::Notch => (SvfMode::Notch, q),
            };
            self.st[i].set(mode, f, sq, sr);
        }
    }
}

fn butter_q(n: usize, i: usize) -> f32 {
    match n {
        2 => [0.541_196_1, 1.306_563][i],
        4 => [0.509_795_6, 0.601_344_9, 0.899_976_3, 2.562_915_4][i],
        _ => std::f32::consts::FRAC_1_SQRT_2,
    }
}

fn stages_for(db: u32) -> usize {
    match db {
        0..=12 => 1,
        13..=24 => 2,
        _ => 4,
    }
}

/// Parametric EQ: any number of cuts, shelves, bells and notches in series.
///
/// Gains are linear factors, written most naturally with `.db()` (non-positive plain numbers are
/// read as dB, see [`level`](super::util::level)). Frequencies, gains and Qs are all automatable
/// [`Param`]s and smoothed.
///
/// ```ignore
/// Eq::new()
///     .low_cut(30.0)                    // 24 dB/oct Butterworth high-pass
///     .low_shelf(100.0, 3.db())
///     .peak(2500.0, -2.db(), 1.0)       // freq, gain, Q
///     .high_shelf(10_000.0, 2.db())
///     .high_cut(18_000.0)
/// ```
#[derive(Clone, Debug)]
pub struct Eq {
    bands: Vec<Band>,
    output: Param,
    out_ramp: Ramp,
    sr: f32,
}

impl Eq {
    /// An empty (transparent) EQ.
    pub fn new() -> Eq {
        Eq {
            bands: vec![],
            output: Param::Const(1.0),
            out_ramp: Ramp::default(),
            sr: 48_000.0,
        }
    }
    fn band(mut self, b: Band) -> Eq {
        self.bands.push(b);
        self
    }
    /// Orchestral mix polish: rumble cut at 25 Hz, a touch less low-mid mud around 300 Hz, a
    /// gentle presence lift and an airy high shelf.
    pub fn orchestral_air() -> Eq {
        Eq::new()
            .low_cut_slope(25.0, 12)
            .peak(300.0, -1.5, 0.8)
            .peak(3_000.0, 0.8, 0.7)
            .high_shelf(11_000.0, 2.0f32.powf(2.5 / 6.02))
    }
    /// 24 dB/oct high-pass ("low cut") at `freq`.
    pub fn low_cut(self, freq: impl Into<Param>) -> Eq {
        self.low_cut_slope(freq, 24)
    }
    /// High-pass with a chosen slope (12, 24 or 48 dB/oct).
    pub fn low_cut_slope(self, freq: impl Into<Param>, db_per_oct: u32) -> Eq {
        self.band(Band::new(
            Kind::LowCut(stages_for(db_per_oct)),
            freq.into(),
            Param::Const(1.0),
            Param::Const(0.707),
        ))
    }
    /// 24 dB/oct low-pass ("high cut") at `freq`.
    pub fn high_cut(self, freq: impl Into<Param>) -> Eq {
        self.high_cut_slope(freq, 24)
    }
    /// Low-pass with a chosen slope (12, 24 or 48 dB/oct).
    pub fn high_cut_slope(self, freq: impl Into<Param>, db_per_oct: u32) -> Eq {
        self.band(Band::new(
            Kind::HighCut(stages_for(db_per_oct)),
            freq.into(),
            Param::Const(1.0),
            Param::Const(0.707),
        ))
    }
    /// Low shelf (Q 0.707).
    pub fn low_shelf(self, freq: impl Into<Param>, gain: impl Into<Param>) -> Eq {
        self.band(Band::new(
            Kind::LowShelf,
            freq.into(),
            gain.into(),
            Param::Const(0.707),
        ))
    }
    /// High shelf (Q 0.707).
    pub fn high_shelf(self, freq: impl Into<Param>, gain: impl Into<Param>) -> Eq {
        self.band(Band::new(
            Kind::HighShelf,
            freq.into(),
            gain.into(),
            Param::Const(0.707),
        ))
    }
    /// Bell: boost/cut `gain` around `freq` with bandwidth `q` (0.5 broad … 10 surgical).
    pub fn peak(self, freq: impl Into<Param>, gain: impl Into<Param>, q: impl Into<Param>) -> Eq {
        self.band(Band::new(Kind::Peak, freq.into(), gain.into(), q.into()))
    }
    /// Alias of [`Eq::peak`].
    pub fn bell(self, freq: impl Into<Param>, gain: impl Into<Param>, q: impl Into<Param>) -> Eq {
        self.peak(freq, gain, q)
    }
    /// Notch out `freq`.
    pub fn notch(self, freq: impl Into<Param>, q: impl Into<Param>) -> Eq {
        self.band(Band::new(
            Kind::Notch,
            freq.into(),
            Param::Const(1.0),
            q.into(),
        ))
    }
    /// Output gain (linear, e.g. `-1.db()`).
    pub fn output(mut self, gain: impl Into<Param>) -> Eq {
        self.output = gain.into();
        self
    }
    /// Number of bands.
    pub fn len(&self) -> usize {
        self.bands.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }
}

impl Effect for Eq {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        for b in &mut self.bands {
            b.sm_f.time(0.015, sr);
            b.sm_g.time(0.015, sr);
            b.sm_q.time(0.015, sr);
            b.sm_f.reset();
            b.sm_g.reset();
            b.sm_q.reset();
            b.last = (f32::NAN, f32::NAN, f32::NAN);
        }
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        for b in &mut self.bands {
            b.sm_f.set(ctx.p(&b.freq).clamp(10.0, sr * 0.49).log2());
            b.sm_g.set(level_db(ctx.p(&b.gain)).clamp(-48.0, 30.0));
            b.sm_q.set(ctx.p(&b.q).clamp(0.05, 40.0));
        }
        let out = ctx.p(&self.output);
        self.out_ramp.start(out, buf.len());
        for chunk in buf.chunks_mut(SUB) {
            let n = chunk.len();
            for b in &mut self.bands {
                let (f, g, q) = (b.sm_f.advance(n), b.sm_g.advance(n), b.sm_q.advance(n));
                b.update(sr, f, g, q);
            }
            for fr in chunk.iter_mut() {
                let (mut l, mut r) = (fr.l, fr.r);
                for b in &mut self.bands {
                    let k = b.stages();
                    for s in &mut b.st[..k] {
                        l = s.tick(0, l);
                        r = s.tick(1, r);
                    }
                }
                let g = self.out_ramp.next();
                *fr = Frame::new(l * g, r * g);
            }
        }
    }
    fn reset(&mut self) {
        for b in &mut self.bands {
            b.st.iter_mut().for_each(|s| s.reset());
        }
    }
    fn tail(&self) -> f32 {
        0.05
    }
    fn name(&self) -> String {
        "eq".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Units;

    fn gain_at(eq: Eq, hz: f32) -> f32 {
        let sr = 48_000.0;
        let mut eq = eq;
        eq.prepare(sr);
        let ctx = Ctx::offline(sr);
        let (mut acc, mut cnt, mut t) = (0.0f32, 0, 0usize);
        for blk in 0..300 {
            let mut buf = [Frame::ZERO; 64];
            for f in buf.iter_mut() {
                *f = Frame::mono((t as f32 * std::f32::consts::TAU * hz / sr).sin());
                t += 1;
            }
            eq.process(&mut buf, &ctx);
            if blk > 150 {
                for f in &buf {
                    acc += f.l * f.l;
                    cnt += 1;
                }
            }
        }
        20.0 * ((acc / cnt as f32).sqrt() * std::f32::consts::SQRT_2).log10()
    }

    #[test]
    fn bands_do_what_they_say() {
        let db = gain_at(Eq::new().peak(2500.0, -6.db(), 1.0), 2500.0);
        assert!((db + 6.0).abs() < 0.3, "{db}");
        let db = gain_at(Eq::new().low_shelf(100.0, 3.db()), 30.0);
        assert!((db - 3.0).abs() < 0.4, "{db}");
        let db = gain_at(Eq::new().high_shelf(10_000.0, 2.db()), 20_000.0);
        assert!((db - 2.0).abs() < 0.4, "{db}");
        let db = gain_at(Eq::new().low_cut(100.0), 25.0);
        assert!(db < -40.0, "{db}");
        let db = gain_at(Eq::new().low_cut(30.0).high_cut(18_000.0), 1000.0);
        assert!(db.abs() < 0.1, "{db}");
        let db = gain_at(Eq::new().peak(1000.0, -3.0, 2.0), 1000.0); // plain negative = dB
        assert!((db + 3.0).abs() < 0.3, "{db}");
    }
}
