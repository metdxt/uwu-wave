//! Unit sugar: `-6.db()`, `440.hz()`, `120.ms()`, `2.bars()`, `3.beats()`, `50.pct()`.
//!
//! `.db()` returns a [`Db`] newtype so that `-6.db()` really means *minus six decibels*
//! (method calls bind tighter than unary minus; `Db` implements `Neg`).

use crate::time::Time;
use std::ops::{Add, Neg, Sub};

/// Decibels. Converts to a linear gain wherever an `f32`/[`Param`](crate::param::Param) is expected.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct Db(pub f32);

impl Db {
    /// Linear amplitude factor. Anything at or below -144 dB is treated as silence.
    pub fn gain(self) -> f32 {
        db_to_gain(self.0)
    }
}
impl Neg for Db {
    type Output = Db;
    fn neg(self) -> Db {
        Db(-self.0)
    }
}
impl Add for Db {
    type Output = Db;
    fn add(self, o: Db) -> Db {
        Db(self.0 + o.0)
    }
}
impl Sub for Db {
    type Output = Db;
    fn sub(self, o: Db) -> Db {
        Db(self.0 - o.0)
    }
}
impl From<Db> for f32 {
    fn from(d: Db) -> f32 {
        d.gain()
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -144.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}
pub fn gain_to_db(g: f32) -> f32 {
    if g <= 1e-8 { -160.0 } else { 20.0 * g.log10() }
}

/// Anything that can be read as a plain `f32` value (numbers, and [`Db`] as linear gain).
pub trait Val {
    fn val(self) -> f32;
}
macro_rules! impl_val { ($($t:ty),*) => { $(impl Val for $t { #[inline] fn val(self) -> f32 { self as f32 } })* } }
impl_val!(f32, f64, i32, i64, u32, u64, usize, i16, u8, u16);
impl Val for Db {
    fn val(self) -> f32 {
        self.gain()
    }
}

/// Number extension methods.
pub trait Units: Copy {
    fn as_f64(self) -> f64;
    /// Decibels (see [`Db`]).
    fn db(self) -> Db {
        Db(self.as_f64() as f32)
    }
    /// Hertz (identity, for readability).
    fn hz(self) -> f32 {
        self.as_f64() as f32
    }
    /// Kilohertz → Hz.
    fn khz(self) -> f32 {
        (self.as_f64() * 1000.0) as f32
    }
    /// Milliseconds → seconds.
    fn ms(self) -> f32 {
        (self.as_f64() / 1000.0) as f32
    }
    /// Seconds (identity).
    fn secs(self) -> f32 {
        self.as_f64() as f32
    }
    /// Percent → 0..1.
    fn pct(self) -> f32 {
        (self.as_f64() / 100.0) as f32
    }
    /// Semitones → frequency ratio.
    fn st(self) -> f32 {
        2f32.powf(self.as_f64() as f32 / 12.0)
    }
    /// Musical time in beats (quarter notes).
    fn beats(self) -> Time {
        Time(self.as_f64())
    }
    /// Musical time in bars (uses the current meter, see [`crate::time::set_meter`]).
    fn bars(self) -> Time {
        Time(self.as_f64() * crate::time::beats_per_bar())
    }
}
macro_rules! impl_units { ($($t:ty),*) => { $(impl Units for $t { #[inline] fn as_f64(self) -> f64 { self as f64 } })* } }
impl_units!(f32, f64, i32, i64, u32, u64, usize);
