//! Parameters: every knob in uwu-wave is a [`Param`].
//!
//! A `Param` is a constant, an automation curve, a writable automation [`Lane`], an LFO, or an
//! arithmetic expression combining any of those:
//!
//! ```ignore
//! LowPass::new(800.0)                                   // constant
//! LowPass::new(auto![0 => 200.0, bar(8) => 8000.0 => exp]) // automation
//! LowPass::new(lfo::sine(2.bars()).range(300.0, 3000.0)) // tempo-synced LFO
//! LowPass::new(&cutoff_lane)                            // lane written later by sections
//! LowPass::new(lfo::tri(1.bars()) * 400.0 + 1200.0)     // expressions
//! ```
//!
//! Params are evaluated at control rate (once per processing block, ≤64 samples) via
//! [`Ctx::p`](crate::Ctx::p).

use crate::time::Time;
use crate::units::{Db, Val};
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::sync::{Arc, RwLock};

/// Where we are on the timeline: musical position (beats) and wall-clock seconds since song start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clock {
    pub beat: f64,
    pub secs: f64,
}

/// How a segment of an automation curve travels *into* a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// Straight line.
    Linear,
    /// Exponential (geometric) – the right curve for frequencies and gains. Falls back to linear
    /// when the values have different signs or touch zero.
    Exp,
    /// Hold the previous value, jump at the point.
    Step,
    /// Cosine ease-in-out.
    Smooth,
    /// Power curve `t^k` (k > 1 = slow start, k < 1 = fast start).
    Pow(f32),
}

impl Curve {
    #[inline]
    pub fn interp(self, a: f32, b: f32, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => a + (b - a) * t,
            Curve::Exp => {
                if a > 0.0 && b > 0.0 {
                    a * (b / a).powf(t)
                } else if a < 0.0 && b < 0.0 {
                    -((-a) * (b / a).powf(t))
                } else {
                    a + (b - a) * t
                }
            }
            Curve::Step => {
                if t >= 1.0 { b } else { a }
            }
            Curve::Smooth => a + (b - a) * (0.5 - 0.5 * (std::f32::consts::PI * t).cos()),
            Curve::Pow(k) => a + (b - a) * t.powf(k.max(1e-3)),
        }
    }
}

/// One automation breakpoint. `curve` describes the segment arriving at this point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub beat: f64,
    pub value: f32,
    pub curve: Curve,
}

/// A breakpoint automation curve on the musical timeline.
///
/// Before the first point the curve holds the first value, after the last it holds the last value.
/// Use [`Automation::looped`] to repeat it (great for per-beat "pump" shapes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Automation {
    pub points: Vec<Point>,
    pub default: f32,
    pub loop_len: Option<f64>,
}

impl Automation {
    pub fn new() -> Self {
        Automation { points: vec![], default: 0.0, loop_len: None }
    }
    /// A curve that starts at `v` at beat 0 (and stays there until more points are added),
    /// so `Automation::from_value(100).to(8.beats(), 140)` ramps 100 → 140.
    pub fn from_value(v: impl Val) -> Self {
        let v = v.val();
        Automation { points: vec![Point { beat: 0.0, value: v, curve: Curve::Step }], default: v, loop_len: None }
    }
    /// Insert a point (kept sorted; points at equal times keep insertion order so you can jump).
    pub fn push(&mut self, at: impl Into<Time>, v: impl Val, curve: Curve) -> &mut Self {
        let p = Point { beat: at.into().0, value: v.val(), curve };
        let idx = self.points.partition_point(|q| q.beat <= p.beat);
        self.points.insert(idx, p);
        self
    }
    /// Builder: linear ramp arriving at `(at, v)`.
    pub fn to(mut self, at: impl Into<Time>, v: impl Val) -> Self {
        self.push(at, v, Curve::Linear);
        self
    }
    /// Builder: exponential ramp arriving at `(at, v)`.
    pub fn exp(mut self, at: impl Into<Time>, v: impl Val) -> Self {
        self.push(at, v, Curve::Exp);
        self
    }
    /// Builder: smooth (cosine) ramp arriving at `(at, v)`.
    pub fn smooth(mut self, at: impl Into<Time>, v: impl Val) -> Self {
        self.push(at, v, Curve::Smooth);
        self
    }
    /// Builder: jump to `v` at `at`.
    pub fn step(mut self, at: impl Into<Time>, v: impl Val) -> Self {
        self.push(at, v, Curve::Step);
        self
    }
    /// Builder: curve with an explicit shape.
    pub fn curve(mut self, at: impl Into<Time>, v: impl Val, c: Curve) -> Self {
        self.push(at, v, c);
        self
    }
    /// Repeat the curve every `len` (measured from beat 0).
    pub fn looped(mut self, len: impl Into<Time>) -> Self {
        self.loop_len = Some(len.into().0.max(1e-9));
        self
    }
    /// Move every point by `dt`.
    pub fn shift(mut self, dt: impl Into<Time>) -> Self {
        let d = dt.into().0;
        for p in &mut self.points {
            p.beat += d;
        }
        self
    }
    /// Multiply every value by `k`.
    pub fn scale(mut self, k: f32) -> Self {
        for p in &mut self.points {
            p.value *= k;
        }
        self.default *= k;
        self
    }

    /// Evaluate at a beat position.
    pub fn at_beat(&self, beat: f64) -> f32 {
        let pts = &self.points;
        if pts.is_empty() {
            return self.default;
        }
        let beat = match self.loop_len {
            Some(l) => beat.rem_euclid(l),
            None => beat,
        };
        if beat < pts[0].beat {
            return pts[0].value;
        }
        // first index with p.beat > beat
        let i = pts.partition_point(|p| p.beat <= beat);
        if i >= pts.len() {
            return pts[pts.len() - 1].value;
        }
        let a = pts[i - 1];
        let b = pts[i];
        let span = b.beat - a.beat;
        if span <= 0.0 {
            return b.value;
        }
        let t = ((beat - a.beat) / span) as f32;
        b.curve.interp(a.value, b.value, t)
    }

    /// Last breakpoint position (beats).
    pub fn end(&self) -> f64 {
        self.points.last().map(|p| p.beat).unwrap_or(0.0)
    }
}

/// A shared, writable automation lane. Create it up-front, hand it to any parameter
/// (`LowPass::new(&lane)`), and write points later — e.g. from inside song sections.
///
/// ```ignore
/// let cutoff = Lane::new(18_000.0);
/// let pad = song.track("pad", Synth::pad()).fx(LowPass::new(&cutoff));
/// song.section("build", 8.bars(), |s| { s.ramp(&cutoff, 300.0, 12_000.0); });
/// ```
#[derive(Clone)]
pub struct Lane(Arc<RwLock<Automation>>);

impl Lane {
    pub fn new(default: impl Val) -> Lane {
        Lane(Arc::new(RwLock::new(Automation::from_value(default))))
    }
    fn write(&self, f: impl FnOnce(&mut Automation)) -> &Lane {
        f(&mut self.0.write().unwrap());
        self
    }
    /// Add a point with an explicit curve.
    pub fn point(&self, at: impl Into<Time>, v: impl Val, c: Curve) -> &Lane {
        let at = at.into();
        let v = v.val();
        self.write(|a| {
            a.push(at, v, c);
        })
    }
    /// Jump to `v` at `at`.
    pub fn set(&self, at: impl Into<Time>, v: impl Val) -> &Lane {
        self.point(at, v, Curve::Step)
    }
    /// Linear ramp arriving at `(at, v)`.
    pub fn to(&self, at: impl Into<Time>, v: impl Val) -> &Lane {
        self.point(at, v, Curve::Linear)
    }
    /// Exponential ramp arriving at `(at, v)`.
    pub fn exp_to(&self, at: impl Into<Time>, v: impl Val) -> &Lane {
        self.point(at, v, Curve::Exp)
    }
    /// Jump to `from` at `start` then ramp with `curve` to `to` at `end`.
    pub fn ramp(&self, start: impl Into<Time>, end: impl Into<Time>, from: impl Val, to: impl Val, curve: Curve) -> &Lane {
        let (s, e, f, t) = (start.into(), end.into(), from.val(), to.val());
        self.write(|a| {
            a.push(s, f, Curve::Step);
            a.push(e, t, curve);
        })
    }
    /// Copy all points of an [`Automation`] into the lane.
    pub fn merge(&self, auto: &Automation) -> &Lane {
        self.write(|a| {
            for p in &auto.points {
                a.push(Time(p.beat), p.value, p.curve);
            }
        })
    }
    pub fn at_beat(&self, beat: f64) -> f32 {
        self.0.read().unwrap().at_beat(beat)
    }
    /// Snapshot of the current curve.
    pub fn snapshot(&self) -> Automation {
        self.0.read().unwrap().clone()
    }
}

impl fmt::Debug for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lane({:?})", self.0.read().unwrap())
    }
}

/// LFO rate: tempo-synced period (from a [`Time`]) or free-running Hz (from a number).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// Period in beats.
    Period(f64),
    Hz(f64),
}
impl From<Time> for Rate {
    fn from(t: Time) -> Rate {
        Rate::Period(t.0.max(1e-9))
    }
}
macro_rules! rate_from { ($($t:ty),*) => { $(impl From<$t> for Rate { fn from(v: $t) -> Rate { Rate::Hz(v as f64) } })* } }
rate_from!(f32, f64, i32, u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Sine,
    Triangle,
    /// Rising ramp.
    Saw,
    /// Falling ramp.
    Ramp,
    Square,
    /// Random value held for each period.
    SampleHold,
    /// Random values smoothly interpolated (cosine) each period.
    Drift,
}

/// Low-frequency oscillator. Outputs `-1..1` by default; use [`Lfo::range`] / [`Lfo::unipolar`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lfo {
    pub shape: Shape,
    pub rate: Rate,
    pub phase: f64,
    pub lo: f32,
    pub hi: f32,
    pub seed: u64,
}

impl Lfo {
    pub fn new(shape: Shape, rate: impl Into<Rate>) -> Lfo {
        Lfo { shape, rate: rate.into(), phase: 0.0, lo: -1.0, hi: 1.0, seed: 0x5eed }
    }
    /// Map the output to `lo..hi`.
    pub fn range(mut self, lo: impl Val, hi: impl Val) -> Lfo {
        self.lo = lo.val();
        self.hi = hi.val();
        self
    }
    /// Map the output to `0..1`.
    pub fn unipolar(self) -> Lfo {
        self.range(0.0, 1.0)
    }
    /// Output `center ± depth`.
    pub fn around(self, center: f32, depth: f32) -> Lfo {
        self.range(center - depth, center + depth)
    }
    /// Phase offset in cycles (0..1).
    pub fn phase(mut self, p: f64) -> Lfo {
        self.phase = p;
        self
    }
    pub fn seed(mut self, s: u64) -> Lfo {
        self.seed = s;
        self
    }
    /// Evaluate (bipolar shape mapped into lo..hi).
    pub fn at(&self, c: Clock) -> f32 {
        let cyc = match self.rate {
            Rate::Period(p) => c.beat / p,
            Rate::Hz(hz) => c.secs * hz,
        } + self.phase;
        let ph = cyc.rem_euclid(1.0) as f32;
        let n = cyc.floor() as i64;
        let tau = std::f32::consts::TAU;
        let bi = match self.shape {
            Shape::Sine => (ph * tau).sin(),
            Shape::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
            Shape::Saw => 2.0 * ph - 1.0,
            Shape::Ramp => 1.0 - 2.0 * ph,
            Shape::Square => {
                if ph < 0.5 { 1.0 } else { -1.0 }
            }
            Shape::SampleHold => crate::dsp::hash_bipolar(self.seed, n as u64),
            Shape::Drift => {
                let a = crate::dsp::hash_bipolar(self.seed, n as u64);
                let b = crate::dsp::hash_bipolar(self.seed, (n + 1) as u64);
                Curve::Smooth.interp(a, b, ph)
            }
        };
        self.lo + (self.hi - self.lo) * (bi * 0.5 + 0.5)
    }
}

/// LFO constructors: `lfo::sine(1.bars())` (synced) or `lfo::sine(5.0)` (5 Hz).
pub mod lfo {
    use super::*;
    pub fn sine(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Sine, rate) }
    pub fn tri(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Triangle, rate) }
    pub fn saw(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Saw, rate) }
    pub fn ramp(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Ramp, rate) }
    pub fn square(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Square, rate) }
    pub fn random(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::SampleHold, rate) }
    pub fn drift(rate: impl Into<Rate>) -> Lfo { Lfo::new(Shape::Drift, rate) }
}

type MapFn = Arc<dyn Fn(f32) -> f32 + Send + Sync>;
type ClockFn = Arc<dyn Fn(Clock) -> f32 + Send + Sync>;

#[derive(Clone)]
enum Expr {
    Add(Param, Param),
    Sub(Param, Param),
    Mul(Param, Param),
    Div(Param, Param),
    Min(Param, Param),
    Max(Param, Param),
    Map(Param, MapFn),
    Func(ClockFn),
    DbToGain(Param),
    /// Re-time the inner param: evaluate at `beat - offset`.
    Delay(Param, f64),
}

/// A (possibly time-varying) control value. See the [module docs](self).
#[derive(Clone)]
pub enum Param {
    Const(f32),
    Auto(Arc<Automation>),
    Lane(Lane),
    Lfo(Lfo),
    #[doc(hidden)]
    Expr(Arc<ExprBox>),
}

#[doc(hidden)]
#[derive(Clone)]
pub struct ExprBox(Expr);

impl Param {
    /// A parameter computed by an arbitrary function of the clock.
    pub fn func(f: impl Fn(Clock) -> f32 + Send + Sync + 'static) -> Param {
        Param::Expr(Arc::new(ExprBox(Expr::Func(Arc::new(f)))))
    }
    fn expr(e: Expr) -> Param {
        Param::Expr(Arc::new(ExprBox(e)))
    }

    /// Evaluate at a clock position.
    pub fn at(&self, c: Clock) -> f32 {
        match self {
            Param::Const(v) => *v,
            Param::Auto(a) => a.at_beat(c.beat),
            Param::Lane(l) => l.at_beat(c.beat),
            Param::Lfo(l) => l.at(c),
            Param::Expr(e) => match &e.0 {
                Expr::Add(a, b) => a.at(c) + b.at(c),
                Expr::Sub(a, b) => a.at(c) - b.at(c),
                Expr::Mul(a, b) => a.at(c) * b.at(c),
                Expr::Div(a, b) => {
                    let d = b.at(c);
                    if d == 0.0 { 0.0 } else { a.at(c) / d }
                }
                Expr::Min(a, b) => a.at(c).min(b.at(c)),
                Expr::Max(a, b) => a.at(c).max(b.at(c)),
                Expr::Map(a, f) => f(a.at(c)),
                Expr::Func(f) => f(c),
                Expr::DbToGain(a) => crate::units::db_to_gain(a.at(c)),
                Expr::Delay(a, off) => a.at(Clock { beat: c.beat - off, secs: c.secs }),
            },
        }
    }
    /// Evaluate at beat position only (seconds = 0).
    pub fn at_beat(&self, beat: f64) -> f32 {
        self.at(Clock { beat, secs: 0.0 })
    }
    /// Constant value, if this param is constant.
    pub fn as_const(&self) -> Option<f32> {
        match self {
            Param::Const(v) => Some(*v),
            _ => None,
        }
    }
    pub fn is_const(&self) -> bool {
        matches!(self, Param::Const(_))
    }
    /// Apply a function to the value.
    pub fn map(self, f: impl Fn(f32) -> f32 + Send + Sync + 'static) -> Param {
        match self {
            Param::Const(v) => Param::Const(f(v)),
            p => Param::expr(Expr::Map(p, Arc::new(f))),
        }
    }
    pub fn clamp(self, lo: f32, hi: f32) -> Param {
        self.map(move |v| v.clamp(lo, hi))
    }
    pub fn min(self, o: impl Into<Param>) -> Param {
        Param::expr(Expr::Min(self, o.into()))
    }
    pub fn max(self, o: impl Into<Param>) -> Param {
        Param::expr(Expr::Max(self, o.into()))
    }
    /// Interpret this param's values as decibels and convert to linear gain.
    pub fn db_to_gain(self) -> Param {
        match self {
            Param::Const(v) => Param::Const(crate::units::db_to_gain(v)),
            p => Param::expr(Expr::DbToGain(p)),
        }
    }
    /// Shift the param later in time by `dt`.
    pub fn delayed(self, dt: impl Into<Time>) -> Param {
        Param::expr(Expr::Delay(self, dt.into().0))
    }
}

impl Default for Param {
    fn default() -> Param {
        Param::Const(0.0)
    }
}

impl fmt::Debug for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Param::Const(v) => write!(f, "{v}"),
            Param::Auto(a) => write!(f, "Auto({} pts)", a.points.len()),
            Param::Lane(_) => write!(f, "Lane"),
            Param::Lfo(l) => write!(f, "{l:?}"),
            Param::Expr(_) => write!(f, "Expr"),
        }
    }
}

macro_rules! param_from_num { ($($t:ty),*) => { $(impl From<$t> for Param { fn from(v: $t) -> Param { Param::Const(v as f32) } })* } }
param_from_num!(f32, f64, i32, i64, u32, usize);
impl From<Db> for Param {
    fn from(d: Db) -> Param {
        Param::Const(d.gain())
    }
}
impl From<Automation> for Param {
    fn from(a: Automation) -> Param {
        Param::Auto(Arc::new(a))
    }
}
impl From<&Automation> for Param {
    fn from(a: &Automation) -> Param {
        Param::Auto(Arc::new(a.clone()))
    }
}
impl From<Lane> for Param {
    fn from(l: Lane) -> Param {
        Param::Lane(l)
    }
}
impl From<&Lane> for Param {
    fn from(l: &Lane) -> Param {
        Param::Lane(l.clone())
    }
}
impl From<Lfo> for Param {
    fn from(l: Lfo) -> Param {
        Param::Lfo(l)
    }
}
impl From<&Param> for Param {
    fn from(p: &Param) -> Param {
        p.clone()
    }
}

// ---- arithmetic sugar -------------------------------------------------------------------------

macro_rules! param_ops {
    ($($ty:ty),*) => {$(
        impl<R: Into<Param>> Add<R> for $ty { type Output = Param; fn add(self, r: R) -> Param { bin(self.into(), r.into(), |a, b| a + b, Expr::Add) } }
        impl<R: Into<Param>> Sub<R> for $ty { type Output = Param; fn sub(self, r: R) -> Param { bin(self.into(), r.into(), |a, b| a - b, Expr::Sub) } }
        impl<R: Into<Param>> Mul<R> for $ty { type Output = Param; fn mul(self, r: R) -> Param { bin(self.into(), r.into(), |a, b| a * b, Expr::Mul) } }
        impl<R: Into<Param>> Div<R> for $ty { type Output = Param; fn div(self, r: R) -> Param { bin(self.into(), r.into(), |a, b| if b == 0.0 { 0.0 } else { a / b }, Expr::Div) } }
        impl Neg for $ty { type Output = Param; fn neg(self) -> Param { bin(Param::Const(0.0), self.into(), |a, b| a - b, Expr::Sub) } }
    )*};
}
param_ops!(Param, Lfo, Automation, Lane);

macro_rules! num_param_ops {
    ($($n:ty),*) => {$(
        impl Add<Param> for $n { type Output = Param; fn add(self, r: Param) -> Param { Param::from(self) + r } }
        impl Sub<Param> for $n { type Output = Param; fn sub(self, r: Param) -> Param { Param::from(self) - r } }
        impl Mul<Param> for $n { type Output = Param; fn mul(self, r: Param) -> Param { Param::from(self) * r } }
        impl Add<Lfo> for $n { type Output = Param; fn add(self, r: Lfo) -> Param { Param::from(self) + r } }
        impl Mul<Lfo> for $n { type Output = Param; fn mul(self, r: Lfo) -> Param { Param::from(self) * r } }
        impl Mul<Automation> for $n { type Output = Param; fn mul(self, r: Automation) -> Param { Param::from(self) * r } }
    )*};
}
num_param_ops!(f32, f64);

fn bin(a: Param, b: Param, f: fn(f32, f32) -> f32, mk: fn(Param, Param) -> Expr) -> Param {
    match (&a, &b) {
        (Param::Const(x), Param::Const(y)) => Param::Const(f(*x, *y)),
        _ => Param::expr(mk(a, b)),
    }
}

/// Build an [`Automation`] curve.
///
/// ```ignore
/// auto![0 => 200.0, bar(8) => 8000.0 => exp, bar(12) => 400.0 => smooth, bar(16) => 400.0 => step]
/// ```
/// Curve names: `lin`, `exp`, `step`, `smooth`. The curve describes how the value travels *into* that point.
#[macro_export]
macro_rules! auto {
    ($($t:expr => $v:expr $(=> $c:ident)?),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut a = $crate::param::Automation::new();
        $( a.push($t, $v, $crate::auto!(@curve $($c)?)); )*
        a
    }};
    (@curve) => { $crate::param::Curve::Linear };
    (@curve lin) => { $crate::param::Curve::Linear };
    (@curve linear) => { $crate::param::Curve::Linear };
    (@curve exp) => { $crate::param::Curve::Exp };
    (@curve step) => { $crate::param::Curve::Step };
    (@curve hold) => { $crate::param::Curve::Step };
    (@curve smooth) => { $crate::param::Curve::Smooth };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Units;

    #[test]
    fn automation_interp() {
        let a = crate::auto![0 => 0.0, 4 => 1.0, 8 => 100.0 => exp, 9 => 5.0 => step];
        assert_eq!(a.at_beat(-1.0), 0.0);
        assert!((a.at_beat(2.0) - 0.5).abs() < 1e-6);
        assert!((a.at_beat(6.0) - 10.0).abs() < 1e-3);
        assert_eq!(a.at_beat(8.5), 100.0);
        assert_eq!(a.at_beat(9.0), 5.0);
        assert_eq!(a.at_beat(100.0), 5.0);
    }

    #[test]
    fn db_neg() {
        let p: Param = (-6.db()).into();
        assert!((p.at_beat(0.0) - 0.501).abs() < 0.01);
        let p: Param = (-6).db().into();
        assert!((p.at_beat(0.0) - 0.501).abs() < 0.01);
    }

    #[test]
    fn exprs() {
        let p = lfo::square(4.beats()).range(0.0, 1.0) * 10.0 + 1.0;
        assert_eq!(p.at_beat(0.5), 11.0);
        assert_eq!(p.at_beat(2.5), 1.0);
        let lane = Lane::new(3.0);
        let q: Param = (&lane).into();
        assert_eq!(q.at_beat(1.0), 3.0);
        lane.set(2.0, 7.0);
        assert_eq!(q.at_beat(5.0), 7.0);
    }
}
