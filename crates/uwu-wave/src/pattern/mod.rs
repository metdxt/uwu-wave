//! Patterns: Strudel/TidalCycles-style cyclic note patterns with a mini-notation.
//!
//! A [`Pattern`] describes what happens in each **cycle**. By default one cycle is one bar
//! (`crate::time::beats_per_bar()`, read when the pattern is played), so a sequence of four
//! steps plays quarter notes in 4/4. Change it with [`Pattern::cycle`]. Patterns implement
//! [`Seq`](crate::clip::Seq), and strings are parsed automatically wherever a sequence is
//! expected:
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! # let song = Song::new("x"); let lead = song.track("lead", Silence); let bass = song.track("bass", Silence);
//! lead.play(bars(0..8), "c4 e4 g4 [b4 c5]");
//! song.section("verse", 8.bars(), |s| {
//!     s.play(&bass, "c2 ~ c2 <eb2 g1>");
//!     s.play(&lead, p("0 2 4 <6 7>").scale("C4 minor").fast(2).sometimes(|p| p.up(12)));
//! });
//! ```
//!
//! # Mini-notation
//!
//! | syntax            | meaning                                                                  |
//! |-------------------|--------------------------------------------------------------------------|
//! | `a b c`           | sequence: the steps divide the cycle equally                             |
//! | `~` or `-`        | rest                                                                     |
//! | `[a b]`           | subdivision: a group occupying one step                                  |
//! | `<a b c>`         | alternation: one per cycle (`a` in cycle 0, `b` in cycle 1, ...)          |
//! | `[a, b, c]`       | stack (play together); also works at top level: `"c4 e4, c3"`           |
//! | `{a b c, d e}`    | polymeter: every layer runs at the first layer's step rate (`{..}%4` sets it) |
//! | `a*2` / `a/2`     | faster / slower; any group (`[a b]*1.5`); patterned: `a*<2 3>`, `hh*[2 4]`, `a/<1 2>` |
//! | `a!3`, `a !`      | replicate the step (3 copies / one more copy)                            |
//! | `a@3`             | elongate: the step gets weight 3                                         |
//! | `a _ _`           | tie: each `_` extends the previous step by one step                      |
//! | `a?` / `a?0.3`    | drop the event randomly (50% / 30%), deterministic, varies per cycle     |
//! | `bd(3,8)` / `bd(3,8,2)` | euclidean rhythm: 3 hits over 8 steps, rotated left by 2; patterned: `bd(<3 5>,8)` |
//! | `c4^0.6`          | **velocity** (0..1) for a step or group; default velocity is 0.8         |
//!
//! **Atoms**: note names `c4`, `C#4`, `eb3`, `cs4` (`s` = sharp), `a` (no octave = octave 4);
//! MIDI numbers `60`, `61.5` (with [`n`] they are scale degrees, may be negative `-1`);
//! drum names `bd`/`kick` 36, `rim`/`rs` 37, `sd`/`snare` 38, `cp`/`clap` 39, `lt` 41, `hh`/`ch` 42,
//! `ph` 44, `mt` 45, `oh` 46, `ht` 48, `cr`/`crash` 49, `rd`/`ride` 51, `cb` 56, `sh` 70;
//! **chords**: a chord symbol without octave `Cmaj7`, `F#m7b5`, `Bb/D`, `Am` (root in octave 4), or
//! a note with octave + `'` + quality: `c3'maj7`, `g2'7`, `e3'` (major triad). Note: digits right
//! after a root are an octave, so `G7` is the *note* G7 — write `G'7` or `Gdom7` for the chord.
//!
//! Errors come with a column and a caret (see [`PatternError`]); [`p`], [`pat!`](crate::pat) and
//! string conversions panic with that message, [`Pattern::parse`] returns it.
//!
//! # Sugar
//! * [`p`]`("...")`, [`n`]`("0 2 4")` (scale degrees, use with [`Pattern::scale`]),
//!   [`pat!`](crate::pat)`[c4 e4 [g4 b4]*2 ~]` (no quotes), [`seq!`](crate::seq)`[C4, E4, "g4 b4", C4.maj()]`
//! * [`drums!`](crate::drums)`{ bd: "x...x...", sd: "....x...", hh: "x.x.x.x." }` and [`grid`]`(pitch, "x..x")`:
//!   `x` hit, `X` accent, `o` ghost, `1`-`9` velocity levels, `.`/`-`/`~` rest, `_` hold, `|` next cycle.
//! * operators: `a & b` stack, `a + b` play `b` after `a`, `a * 4` repeat — for owned and
//!   borrowed patterns alike (`&beat & "~ ~ ~ cr"`); every combinator takes `&self`, so reusing a
//!   pattern never needs `.clone()`.
//! * [`ToPattern`]: `"c4 e4".pat()`, `C4.maj7().pat()`, `chords.pat()`.

mod grid;
mod mini;
mod ops;
mod rat;

pub use grid::{drum_grid, grid};
pub use mini::PatternError;
pub use ops::ArpMode;

use crate::clip::{IntoSeq, Note, Seq};
use crate::theory::{Chord, Pitch};
use rat::Rat;
use std::sync::Arc;

/// Default velocity of pattern events.
pub const DEFAULT_VEL: f32 = 0.8;
pub(crate) const EPS: f64 = 1e-9;

/// One event of a pattern, in **cycles** (see [`Pattern::events`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub start: f64,
    pub dur: f64,
    /// (Fractional) MIDI pitch — or a scale degree for [`n`] patterns before [`Pattern::scale`].
    pub pitch: f32,
    /// Velocity 0..1.
    pub vel: f32,
}

impl Event {
    pub fn end(&self) -> f64 {
        self.start + self.dur
    }
}

/// A query window in cycles, plus the beats-per-cycle in effect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Q {
    pub from: f64,
    pub to: f64,
    pub bpc: f64,
}

impl Q {
    pub fn span(&self, from: f64, to: f64) -> Q {
        Q {
            from,
            to,
            bpc: self.bpc,
        }
    }
    /// Does an onset belong to this window? (half-open, with a tiny epsilon shift so that
    /// adjacent windows partition the timeline exactly)
    pub fn has(&self, t: f64) -> bool {
        t >= self.from - EPS && t < self.to - EPS
    }
    /// Whole cycles touched by this window.
    pub fn cycles(&self) -> std::ops::Range<i64> {
        if self.to <= self.from {
            return 0..0;
        }
        let c0 = (self.from + EPS).floor() as i64;
        let c1 = (self.to - EPS).ceil() as i64;
        c0..c1.max(c0 + 1)
    }
    /// Intersection with cycle `c` (if non-empty).
    pub fn in_cycle(&self, c: i64) -> Option<Q> {
        let lo = self.from.max(c as f64);
        let hi = self.to.min(c as f64 + 1.0);
        (hi > lo).then(|| self.span(lo, hi))
    }
}

type QueryFn = dyn Fn(&Q) -> Vec<Event> + Send + Sync;

/// A cyclic pattern of notes. Cheap to clone (the query is shared). See the [module docs](self).
///
/// ```
/// use uwu_wave::prelude::*;
/// let a = p("c4 e4 g4 <b4 c5>");
/// assert_eq!(a.cycles(), 2.0);                 // the alternation needs 2 cycles to repeat
/// let b = p("bd(3,8)").fast(2) & p("~ sd");   // stack
/// let c = a.clone() + b.clone();               // a, then b
/// let d = p("0 2 4 6").scale("D3 dorian").every(4, |p| p.rev());
/// let e = p("<Cmaj7 Am7 Dm7 G'7>").arp("updown", 8);
/// # let _ = (c, d, e);
/// ```
#[derive(Clone)]
pub struct Pattern {
    query: Arc<QueryFn>,
    period: Rat,
    cycle_beats: Option<f64>,
    seed: u64,
}

impl std::fmt::Debug for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ev = self.events(0.0, self.cycles().min(4.0));
        f.debug_struct("Pattern")
            .field("cycles", &self.cycles())
            .field("cycle_beats", &self.cycle_beats)
            .field("events", &ev)
            .finish()
    }
}

impl Pattern {
    pub(crate) fn from_query(
        period: Rat,
        f: impl Fn(&Q) -> Vec<Event> + Send + Sync + 'static,
    ) -> Pattern {
        Pattern {
            query: Arc::new(f),
            period,
            cycle_beats: None,
            seed: 0,
        }
    }
    /// New pattern sharing this one's cycle length and seed.
    pub(crate) fn derive(
        &self,
        period: Rat,
        f: impl Fn(&Q) -> Vec<Event> + Send + Sync + 'static,
    ) -> Pattern {
        Pattern {
            query: Arc::new(f),
            period,
            cycle_beats: self.cycle_beats,
            seed: self.seed,
        }
    }
    pub(crate) fn q(&self, q: &Q) -> Vec<Event> {
        (self.query)(q)
    }
    pub(crate) fn period(&self) -> Rat {
        self.period
    }
    pub(crate) fn with_period(mut self, p: Rat) -> Pattern {
        self.period = p;
        self
    }
    pub(crate) fn seed_value(&self) -> u64 {
        self.seed
    }

    /// Parse mini-notation (see the [module docs](self)).
    pub fn parse(src: &str) -> Result<Pattern, PatternError> {
        mini::parse(src, mini::Mode::Notes)
    }
    /// Parse, panicking with a caret-pointed message on error (used by [`p`] and [`pat!`](crate::pat)).
    #[track_caller]
    pub fn parse_or_panic(src: &str) -> Pattern {
        Pattern::parse(src).unwrap_or_else(|e| panic!("{e}"))
    }
    /// A pattern from a custom query: `f(from, to)` must return the events (in cycles) whose start
    /// lies in `[from, to)`. `cycles` is the loop length.
    pub fn from_fn(
        cycles: f64,
        f: impl Fn(f64, f64) -> Vec<Event> + Send + Sync + 'static,
    ) -> Pattern {
        Pattern::from_query(Rat::from_f64(cycles), move |q| f(q.from, q.to))
    }
    /// Silence (one empty cycle).
    pub fn silence() -> Pattern {
        Pattern::from_query(Rat::ONE, |_| vec![])
    }
    /// One note filling every cycle.
    pub fn pure(pitch: impl Into<Pitch>) -> Pattern {
        Pattern::atom(pitch.into().0, DEFAULT_VEL)
    }
    pub(crate) fn atom(pitch: f32, vel: f32) -> Pattern {
        Pattern::from_query(Rat::ONE, move |q| {
            q.cycles()
                .filter(|&c| q.has(c as f64))
                .map(|c| Event {
                    start: c as f64,
                    dur: 1.0,
                    pitch,
                    vel,
                })
                .collect()
        })
    }
    /// Sequence: the items share one cycle equally (like `"a b c"`).
    pub fn seq<P: Into<Pattern>>(items: impl IntoIterator<Item = P>) -> Pattern {
        Pattern::fastcat_w(items.into_iter().map(|p| (p.into(), 1.0)).collect())
    }
    /// Alternation: one item per cycle (like `"<a b c>"`).
    pub fn alt<P: Into<Pattern>>(items: impl IntoIterator<Item = P>) -> Pattern {
        Pattern::slowcat(items.into_iter().map(Into::into).collect())
    }
    /// Stack: all items at once (like `"[a, b, c]"`).
    pub fn layers<P: Into<Pattern>>(items: impl IntoIterator<Item = P>) -> Pattern {
        let items: Vec<Pattern> = items.into_iter().map(Into::into).collect();
        if items.is_empty() {
            return Pattern::silence();
        }
        let (items, cb) = Pattern::unify_cycles(items);
        let period = items.iter().fold(items[0].period, |a, p| a.lcm(p.period));
        let mut out =
            Pattern::from_query(period, move |q| items.iter().flat_map(|p| p.q(q)).collect());
        out.cycle_beats = cb;
        out
    }
    /// Concatenate in time: each item plays for its full length, then the next (like `a + b`).
    pub fn concat<P: Into<Pattern>>(items: impl IntoIterator<Item = P>) -> Pattern {
        let items: Vec<Pattern> = items.into_iter().map(Into::into).collect();
        if items.is_empty() {
            return Pattern::silence();
        }
        let (items, cb) = Pattern::unify_cycles(items);
        let period = items
            .iter()
            .skip(1)
            .fold(items[0].period, |a, p| a.add(p.period));
        let total = period.f64();
        let mut starts = vec![];
        let mut acc = 0.0;
        for p in &items {
            starts.push(acc);
            acc += p.period.f64();
        }
        let mut out = Pattern::from_query(period, move |q| {
            let mut out = vec![];
            if q.to <= q.from {
                return out;
            }
            let k0 = (q.from / total).floor() as i64;
            let k1 = (q.to / total).ceil() as i64;
            for k in k0..k1.max(k0 + 1) {
                let kt = k as f64 * total;
                for (p, &s) in items.iter().zip(&starts) {
                    let len = p.period.f64();
                    let lo = q.from.max(kt + s);
                    let hi = q.to.min(kt + s + len);
                    if hi <= lo {
                        continue;
                    }
                    // local time keeps advancing across loops (k * len) so random choices vary
                    let off = kt + s - k as f64 * len;
                    out.extend(p.q(&q.span(lo - off, hi - off)).into_iter().map(|e| Event {
                        start: e.start + off,
                        ..e
                    }));
                }
            }
            out
        });
        out.cycle_beats = cb;
        out
    }

    pub(crate) fn fastcat_w(items: Vec<(Pattern, f64)>) -> Pattern {
        let items: Vec<(Pattern, f64)> = items.into_iter().filter(|(_, w)| *w > 0.0).collect();
        if items.is_empty() {
            return Pattern::silence();
        }
        let period = items.iter().fold(Rat::ONE, |a, (p, _)| a.lcm(p.period));
        if items.len() == 1 {
            return items.into_iter().next().unwrap().0.with_period(period);
        }
        let total: f64 = items.iter().map(|(_, w)| w).sum();
        let mut kids = vec![];
        let mut acc = 0.0;
        for (p, w) in items {
            kids.push((p, acc / total, (acc + w) / total));
            acc += w;
        }
        Pattern::from_query(period, move |q| {
            let mut out = vec![];
            for c in q.cycles() {
                let cf = c as f64;
                for (p, a, b) in &kids {
                    let (sa, sb) = (cf + a, cf + b);
                    let lo = q.from.max(sa);
                    let hi = q.to.min(sb);
                    if hi <= lo {
                        continue;
                    }
                    let w = b - a;
                    let sub = q.span(cf + (lo - sa) / w, cf + (hi - sa) / w);
                    out.extend(p.q(&sub).into_iter().map(|e| Event {
                        start: sa + (e.start - cf) * w,
                        dur: e.dur * w,
                        ..e
                    }));
                }
            }
            out
        })
    }

    pub(crate) fn slowcat(items: Vec<Pattern>) -> Pattern {
        if items.is_empty() {
            return Pattern::silence();
        }
        let n = items.len() as i64;
        let period = items
            .iter()
            .fold(Rat::ONE, |a, p| a.lcm(p.period))
            .mul(Rat::int(n as u64));
        Pattern::from_query(period, move |q| {
            let mut out = vec![];
            for c in q.cycles() {
                let Some(sub) = q.in_cycle(c) else { continue };
                let p = &items[c.rem_euclid(n) as usize];
                // child i sees its own cycle count advance once per full alternation
                let off = (c - c.div_euclid(n)) as f64;
                out.extend(
                    p.q(&sub.span(sub.from - off, sub.to - off))
                        .into_iter()
                        .map(|e| Event {
                            start: e.start + off,
                            ..e
                        }),
                );
            }
            out
        })
    }

    /// Beats per cycle in effect right now.
    pub fn beats_per_cycle(&self) -> f64 {
        self.cycle_beats.unwrap_or_else(crate::time::beats_per_bar)
    }
    /// Set the cycle length (default: one bar): `p("c e g").cycle(2.beats())` plays three notes
    /// per two beats. Stacking (`&`) or concatenating (`+`) patterns with different cycle lengths
    /// keeps every part's timing in beats: the result uses the first explicit cycle length.
    pub fn cycle(&self, len: impl Into<crate::time::Time>) -> Pattern {
        let b = len.into().0;
        let mut out = self.clone();
        out.cycle_beats = (b > 0.0).then_some(b);
        out
    }
    /// This pattern expressed in another cycle length (`None` = one bar), keeping its timing in
    /// beats. A "bar" is resolved with the meter in effect at construction time.
    pub(crate) fn rescaled_to(&self, target: Option<f64>) -> Pattern {
        let bpb = crate::time::beats_per_bar();
        let src = self.cycle_beats.unwrap_or(bpb);
        let dst = target.unwrap_or(bpb);
        let mut out = if (src - dst).abs() < 1e-9 {
            self.clone()
        } else {
            self.fast_rat(Rat::from_f64(dst / src))
        };
        out.cycle_beats = target;
        out
    }
    /// The first explicit cycle length among `items`, with every item rescaled into it.
    fn unify_cycles(items: Vec<Pattern>) -> (Vec<Pattern>, Option<f64>) {
        let target = items.iter().find_map(|p| p.cycle_beats);
        if items.iter().all(|p| p.cycle_beats == target) {
            return (items, target);
        }
        (
            items.iter().map(|p| p.rescaled_to(target)).collect(),
            target,
        )
    }
    /// Loop length in cycles (the full alternation period).
    pub fn cycles(&self) -> f64 {
        self.period.f64()
    }
    /// All events whose start lies in `[from, to)` (in cycles), sorted by start then pitch.
    pub fn events(&self, from: f64, to: f64) -> Vec<Event> {
        self.events_with_bpc(from, to, self.beats_per_cycle())
    }
    pub(crate) fn events_with_bpc(&self, from: f64, to: f64, bpc: f64) -> Vec<Event> {
        let q = Q { from, to, bpc };
        let mut ev: Vec<Event> = self
            .q(&q)
            .into_iter()
            .filter(|e| q.has(e.start) && e.dur > 0.0 && e.pitch.is_finite())
            .collect();
        ev.sort_by(|a, b| {
            a.start
                .total_cmp(&b.start)
                .then(a.pitch.total_cmp(&b.pitch))
        });
        ev
    }
    /// Events of the first cycle (handy in tests and for debugging).
    pub fn first_cycle(&self) -> Vec<Event> {
        self.events(0.0, 1.0)
    }
    /// Render one full loop into a [`Clip`](crate::clip::Clip).
    pub fn bake(&self) -> crate::clip::Clip {
        crate::clip::Clip::bake(self, self.len())
    }
}

impl Seq for Pattern {
    fn len(&self) -> f64 {
        self.period.f64() * self.beats_per_cycle()
    }
    fn notes(&self, from: f64, to: f64) -> Vec<Note> {
        let bpc = self.beats_per_cycle();
        self.events(from / bpc, to / bpc)
            .into_iter()
            .filter(|e| e.vel > 0.0)
            .map(|e| Note {
                start: e.start * bpc,
                dur: e.dur * bpc,
                pitch: e.pitch,
                vel: e.vel.clamp(0.0, 1.0),
                artic: Default::default(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------- conversions

impl From<&str> for Pattern {
    /// Parses mini-notation; **panics** with a caret-pointed message on error.
    #[track_caller]
    fn from(s: &str) -> Pattern {
        Pattern::parse_or_panic(s)
    }
}
impl From<String> for Pattern {
    #[track_caller]
    fn from(s: String) -> Pattern {
        Pattern::parse_or_panic(&s)
    }
}
impl From<&String> for Pattern {
    #[track_caller]
    fn from(s: &String) -> Pattern {
        Pattern::parse_or_panic(s)
    }
}
impl From<&Pattern> for Pattern {
    fn from(p: &Pattern) -> Pattern {
        p.clone()
    }
}
impl From<Pitch> for Pattern {
    /// One note per cycle.
    fn from(p: Pitch) -> Pattern {
        Pattern::pure(p)
    }
}
macro_rules! pat_from_num { ($($t:ty),*) => { $(impl From<$t> for Pattern { fn from(v: $t) -> Pattern { Pattern::pure(Pitch(v as f32)) } })* } }
pat_from_num!(f32, f64, i32, u8);

impl From<Chord> for Pattern {
    /// The chord held for one cycle.
    fn from(c: Chord) -> Pattern {
        Pattern::layers(
            c.notes()
                .iter()
                .map(|&p| Pattern::pure(p))
                .collect::<Vec<_>>(),
        )
    }
}
impl From<&Chord> for Pattern {
    fn from(c: &Chord) -> Pattern {
        Pattern::from(c.clone())
    }
}
impl From<Vec<Pitch>> for Pattern {
    /// A sequence: the pitches share one cycle.
    fn from(v: Vec<Pitch>) -> Pattern {
        Pattern::seq(v)
    }
}
impl From<&[Pitch]> for Pattern {
    fn from(v: &[Pitch]) -> Pattern {
        Pattern::seq(v.to_vec())
    }
}
impl From<Vec<Chord>> for Pattern {
    /// A progression: one chord per cycle.
    fn from(v: Vec<Chord>) -> Pattern {
        Pattern::alt(v)
    }
}
impl From<Vec<Pattern>> for Pattern {
    /// A sequence of patterns sharing one cycle.
    fn from(v: Vec<Pattern>) -> Pattern {
        Pattern::seq(v)
    }
}

impl From<&Pitch> for Pattern {
    fn from(p: &Pitch) -> Pattern {
        Pattern::pure(*p)
    }
}
impl From<&Vec<Pitch>> for Pattern {
    fn from(v: &Vec<Pitch>) -> Pattern {
        Pattern::seq(v.clone())
    }
}
impl<const N: usize> From<[Pitch; N]> for Pattern {
    /// A sequence: `Pattern::from([C4, E4, G4])`.
    fn from(v: [Pitch; N]) -> Pattern {
        Pattern::seq(v)
    }
}
impl From<&Vec<Chord>> for Pattern {
    /// A progression: one chord per cycle.
    fn from(v: &Vec<Chord>) -> Pattern {
        Pattern::alt(v.iter())
    }
}
impl From<&[Chord]> for Pattern {
    /// A progression: one chord per cycle.
    fn from(v: &[Chord]) -> Pattern {
        Pattern::alt(v.iter())
    }
}
impl<const N: usize> From<[Chord; N]> for Pattern {
    /// A progression: one chord per cycle.
    fn from(v: [Chord; N]) -> Pattern {
        Pattern::alt(v)
    }
}
impl From<&Vec<Pattern>> for Pattern {
    /// A sequence of patterns sharing one cycle.
    fn from(v: &Vec<Pattern>) -> Pattern {
        Pattern::seq(v.iter())
    }
}

/// `.pat()` — turn strings, chords, progressions and pitches into a [`Pattern`] without
/// ceremony: `"c4 e4".pat().fast(2)`, `C4.maj7().pat().arp("up", 8)`, `chords.pat().open()`.
///
/// ```
/// use uwu_wave::prelude::*;
/// let chords = prog("i VI III VII", "D4 minor");
/// let pads = chords.pat().open();
/// let arp = C4.maj7().pat().arp("up", 8);
/// let riff = "c4 eb4 g4".pat().fast(2);
/// # let _ = (pads, arp, riff);
/// ```
pub trait ToPattern {
    fn pat(&self) -> Pattern;
}
impl ToPattern for str {
    /// Parses mini-notation (panics with a caret-pointed message on error).
    #[track_caller]
    fn pat(&self) -> Pattern {
        Pattern::parse_or_panic(self)
    }
}
impl ToPattern for String {
    #[track_caller]
    fn pat(&self) -> Pattern {
        Pattern::parse_or_panic(self)
    }
}
impl ToPattern for Pattern {
    fn pat(&self) -> Pattern {
        self.clone()
    }
}
impl ToPattern for Pitch {
    /// One note per cycle.
    fn pat(&self) -> Pattern {
        Pattern::pure(*self)
    }
}
impl ToPattern for Chord {
    /// The chord held for one cycle.
    fn pat(&self) -> Pattern {
        Pattern::from(self)
    }
}
impl ToPattern for [Chord] {
    /// A progression: one chord per cycle.
    fn pat(&self) -> Pattern {
        Pattern::alt(self.iter())
    }
}
impl ToPattern for Vec<Chord> {
    /// A progression: one chord per cycle.
    fn pat(&self) -> Pattern {
        Pattern::alt(self.iter())
    }
}
impl ToPattern for [Pitch] {
    /// A sequence sharing one cycle.
    fn pat(&self) -> Pattern {
        Pattern::seq(self.to_vec())
    }
}
impl ToPattern for Vec<Pitch> {
    /// A sequence sharing one cycle.
    fn pat(&self) -> Pattern {
        Pattern::seq(self.clone())
    }
}

impl IntoSeq for &str {
    #[track_caller]
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::parse_or_panic(self))
    }
}
impl IntoSeq for String {
    #[track_caller]
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::parse_or_panic(&self))
    }
}
impl IntoSeq for &String {
    #[track_caller]
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::parse_or_panic(self))
    }
}
impl IntoSeq for &Pattern {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(self.clone())
    }
}
impl IntoSeq for Pitch {
    /// One note per cycle (bar).
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for Chord {
    /// The chord held for a cycle (bar), repeated.
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for Vec<Chord> {
    /// A progression, one chord per cycle (bar).
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for &Chord {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for &Vec<Chord> {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for &[Chord] {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for &Vec<Pitch> {
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}
impl IntoSeq for Vec<Pitch> {
    /// A sequence sharing one cycle.
    fn into_seq(self) -> Arc<dyn Seq> {
        Arc::new(Pattern::from(self))
    }
}

/// Parse a mini-notation pattern (panics with a caret-pointed message on error).
///
/// ```
/// use uwu_wave::prelude::*;
/// let riff = p("c3 [eb3 g3] <bb3 c4>*2 ~");
/// # let _ = riff;
/// ```
#[track_caller]
pub fn p(src: &str) -> Pattern {
    Pattern::parse_or_panic(src)
}

/// A scale-degree pattern: numbers are 0-based degrees, resolved with [`Pattern::scale`].
///
/// ```
/// use uwu_wave::prelude::*;
/// let mel = n("0 2 4 <6 7> -1").scale("C4 minor");
/// assert_eq!(mel.first_cycle()[4].pitch, 58.0); // degree -1 = Bb3
/// ```
#[track_caller]
pub fn n(src: &str) -> Pattern {
    Pattern::parse_or_panic(src)
}

/// Alias of [`n`].
#[track_caller]
pub fn degrees(src: &str) -> Pattern {
    Pattern::parse_or_panic(src)
}

/// Mini-notation without quotes: `pat![c4 e4 [g4 b4]*2 ~]`. Panics with a caret-pointed message on
/// a parse error. Inside the macro use `cs4` instead of `c#4` and bare chord symbols (`Cmaj7`)
/// instead of the `'` form.
#[macro_export]
macro_rules! pat {
    ($($t:tt)*) => { $crate::pattern::Pattern::parse_or_panic(stringify!($($t)*)) };
}

/// A sequence (items share one cycle) from anything convertible into a pattern: pitches, chords,
/// numbers, mini-notation strings or patterns: `seq![C4, E4, "g4 b4", C4.maj7()]`.
#[macro_export]
macro_rules! seq {
    ($($e:expr),* $(,)?) => {{
        let v: ::std::vec::Vec<$crate::pattern::Pattern> = vec![$($crate::pattern::Pattern::from($e)),*];
        $crate::pattern::Pattern::seq(v)
    }};
}

/// Drum-machine grid: one row per drum (GM name, note name or MIDI number), each row spans one
/// cycle (`|` starts the next cycle). `x` hit, `X` accent, `o` ghost, `1`..`9` velocity levels,
/// `.` `-` `~` rest, `_` hold the previous hit. Whitespace is ignored.
///
/// ```
/// use uwu_wave::prelude::*;
/// let beat = drums! {
///     bd: "x . . . x . . .",
///     sd: ". . X . . . X o",
///     hh: "x x x x x x x x",
/// };
/// assert_eq!(beat.first_cycle().len(), 13);
/// ```
#[macro_export]
macro_rules! drums {
    ($($name:tt : $grid:expr),* $(,)?) => {
        $crate::pattern::drum_grid(&[$((stringify!($name), ::std::convert::AsRef::<str>::as_ref(&$grid))),*])
    };
}

pub mod prelude {
    pub use super::{ArpMode, Pattern, PatternError, ToPattern, degrees, grid, n, p};
    pub use crate::{drums, pat, seq};
}

#[cfg(test)]
mod tests;
