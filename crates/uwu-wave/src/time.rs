//! Musical time. Everything on the timeline is measured in **beats** (quarter notes).
//!
//! * `bar(4)` – the start of bar 4 (bars are **0-based**: `bar(0)` is the very beginning)
//! * `bars(0..8)` – a range covering bars 0..8 (use it to loop a pattern)
//! * `2.bars()`, `3.beats()`, `quarter()`, `eighth()`, `dotted(eighth())`, `triplet(quarter())`, `note(1, 16)`
//!
//! Bars depend on the meter, which is stored thread-locally and set by `Song::meter`
//! (default 4/4). Set the meter *before* creating bar positions.

use std::cell::Cell;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Range, Sub, SubAssign};

thread_local! {
    static BEATS_PER_BAR: Cell<f64> = const { Cell::new(4.0) };
}

/// Beats (quarter notes) per bar of the current meter.
pub fn beats_per_bar() -> f64 {
    BEATS_PER_BAR.with(|b| b.get())
}

/// Set the current meter, e.g. `set_meter(6, 8)` → 3 beats per bar.
pub fn set_meter(num: u32, den: u32) {
    BEATS_PER_BAR.with(|b| b.set(num as f64 * 4.0 / den as f64));
}

/// A position or a duration on the musical timeline, in beats.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct Time(pub f64);

impl Time {
    pub const ZERO: Time = Time(0.0);
    pub fn beats(self) -> f64 {
        self.0
    }
    pub fn in_bars(self) -> f64 {
        self.0 / beats_per_bar()
    }
    pub fn max(self, o: Time) -> Time {
        if o.0 > self.0 { o } else { self }
    }
    pub fn min(self, o: Time) -> Time {
        if o.0 < self.0 { o } else { self }
    }
}

impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bpb = beats_per_bar();
        let bar = (self.0 / bpb).floor();
        let beat = self.0 - bar * bpb;
        write!(f, "{}:{:.2}", bar as i64, beat)
    }
}

macro_rules! time_from { ($($t:ty),*) => { $(impl From<$t> for Time { fn from(v: $t) -> Time { Time(v as f64) } })* } }
time_from!(f32, f64, i32, i64, u32, u64, usize);

impl Add for Time { type Output = Time; fn add(self, o: Time) -> Time { Time(self.0 + o.0) } }
impl Sub for Time { type Output = Time; fn sub(self, o: Time) -> Time { Time(self.0 - o.0) } }
impl AddAssign for Time { fn add_assign(&mut self, o: Time) { self.0 += o.0 } }
impl SubAssign for Time { fn sub_assign(&mut self, o: Time) { self.0 -= o.0 } }
impl Neg for Time { type Output = Time; fn neg(self) -> Time { Time(-self.0) } }
impl Mul<f64> for Time { type Output = Time; fn mul(self, k: f64) -> Time { Time(self.0 * k) } }
impl Mul<i32> for Time { type Output = Time; fn mul(self, k: i32) -> Time { Time(self.0 * k as f64) } }
impl Mul<Time> for f64 { type Output = Time; fn mul(self, t: Time) -> Time { Time(self * t.0) } }
impl Mul<Time> for i32 { type Output = Time; fn mul(self, t: Time) -> Time { Time(self as f64 * t.0) } }
impl Div<f64> for Time { type Output = Time; fn div(self, k: f64) -> Time { Time(self.0 / k) } }
impl Div<i32> for Time { type Output = Time; fn div(self, k: i32) -> Time { Time(self.0 / k as f64) } }
impl Div<Time> for Time { type Output = f64; fn div(self, o: Time) -> f64 { self.0 / o.0 } }

/// Start of bar `n` (0-based).
pub fn bar(n: impl Into<f64Like>) -> Time {
    Time(n.into().0 * beats_per_bar())
}
/// Beat position/duration.
pub fn beat(n: impl Into<f64Like>) -> Time {
    Time(n.into().0)
}
/// A range of bars, e.g. `bars(0..8)`.
pub fn bars(r: Range<i32>) -> Range<Time> {
    bar(r.start)..bar(r.end)
}
/// `note(1, 8)` = an eighth note = 0.5 beats.
pub fn note(num: u32, den: u32) -> Time {
    Time(4.0 * num as f64 / den as f64)
}
pub fn whole() -> Time { Time(4.0) }
pub fn half() -> Time { Time(2.0) }
pub fn quarter() -> Time { Time(1.0) }
pub fn eighth() -> Time { Time(0.5) }
pub fn sixteenth() -> Time { Time(0.25) }
pub fn thirty_second() -> Time { Time(0.125) }
pub fn dotted(t: Time) -> Time { Time(t.0 * 1.5) }
pub fn triplet(t: Time) -> Time { Time(t.0 * 2.0 / 3.0) }

/// Helper accepted by [`bar`]/[`beat`] so both integers and floats work.
#[derive(Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
pub struct f64Like(pub f64);
macro_rules! f64like { ($($t:ty),*) => { $(impl From<$t> for f64Like { fn from(v: $t) -> f64Like { f64Like(v as f64) } })* } }
f64like!(f32, f64, i32, i64, u32, u64, usize);

/// Where a clip goes: once at a position, looped over a range, or looped from a position to the song end.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    At(Time),
    Range(Time, Time),
    From(Time),
}
impl From<Time> for Span { fn from(t: Time) -> Span { Span::At(t) } }
impl From<Range<Time>> for Span { fn from(r: Range<Time>) -> Span { Span::Range(r.start, r.end) } }
impl From<std::ops::RangeFrom<Time>> for Span { fn from(r: std::ops::RangeFrom<Time>) -> Span { Span::From(r.start) } }
