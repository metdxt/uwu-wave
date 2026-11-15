//! Pattern combinators.

use super::mini::{Mode, euclid_bools};
use super::rat::Rat;
use super::{DEFAULT_VEL, EPS, Event, Pattern, Q, mini};
use crate::dsp::{hash_bipolar, hash01, splitmix};
use crate::theory::{Chord, Key};
use crate::time::Time;
use crate::units::Val;
use std::ops::{Add, BitAnd, Mul};

/// Arpeggio note order for [`Pattern::arp`]. Strings convert too: `"up"`, `"down"`, `"updown"`,
/// `"downup"`, `"converge"`, `"diverge"`, `"random"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    /// Up then down, without repeating the top/bottom notes.
    UpDown,
    DownUp,
    /// Outside in: lowest, highest, 2nd lowest, 2nd highest...
    Converge,
    /// Inside out.
    Diverge,
    /// Seeded random note each step.
    Random,
}

impl From<&str> for ArpMode {
    #[track_caller]
    fn from(s: &str) -> ArpMode {
        match s.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "up" => ArpMode::Up,
            "down" => ArpMode::Down,
            "updown" => ArpMode::UpDown,
            "downup" => ArpMode::DownUp,
            "converge" => ArpMode::Converge,
            "diverge" => ArpMode::Diverge,
            "random" | "rand" => ArpMode::Random,
            _ => panic!(
                "unknown arp mode {s:?} (use up, down, updown, downup, converge, diverge or random)"
            ),
        }
    }
}

impl ArpMode {
    /// Index order over `n` notes sorted low → high (Random is handled per step).
    fn order(self, n: usize) -> Vec<usize> {
        let up: Vec<usize> = (0..n).collect();
        match self {
            ArpMode::Up | ArpMode::Random => up,
            ArpMode::Down => up.into_iter().rev().collect(),
            ArpMode::UpDown => {
                let mut v = up.clone();
                if n > 2 {
                    v.extend((1..n - 1).rev());
                }
                v
            }
            ArpMode::DownUp => {
                let mut v: Vec<usize> = up.iter().rev().copied().collect();
                if n > 2 {
                    v.extend(1..n - 1);
                }
                v
            }
            ArpMode::Converge | ArpMode::Diverge => {
                let mut v = Vec::with_capacity(n);
                let (mut lo, mut hi) = (0usize, n);
                while lo < hi {
                    v.push(lo);
                    lo += 1;
                    if lo < hi {
                        hi -= 1;
                        v.push(hi);
                    }
                }
                if self == ArpMode::Diverge {
                    v.reverse();
                }
                v
            }
        }
    }
}

fn tkey(t: f64) -> u64 {
    (t * 1e6).round() as i64 as u64
}
fn rand_at(seed: u64, t: f64) -> f32 {
    hash01(seed, tkey(t))
}

/// Group events with the same onset (events must be sorted by start).
fn onset_groups(mut ev: Vec<Event>) -> Vec<Vec<Event>> {
    ev.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.pitch.total_cmp(&b.pitch))
    });
    let mut out: Vec<Vec<Event>> = vec![];
    for e in ev {
        match out.last_mut() {
            Some(g) if (g[0].start - e.start).abs() < 1e-6 => g.push(e),
            _ => out.push(vec![e]),
        }
    }
    out
}

impl Pattern {
    fn lookback(&self) -> f64 {
        self.period().f64().ceil().clamp(2.0, 16.0)
    }

    // ------------------------------------------------------------- time

    pub(crate) fn fast_rat(self, k: Rat) -> Pattern {
        let kf = k.f64();
        let inner = self.clone();
        self.derive(self.period().div(k), move |q| {
            inner
                .q(&q.span(q.from * kf, q.to * kf))
                .into_iter()
                .map(|e| Event {
                    start: e.start / kf,
                    dur: e.dur / kf,
                    ..e
                })
                .collect()
        })
    }
    /// Play `k` times faster (`"a b".fast(2)` = `"a b a b"` per cycle). `k <= 0` gives silence.
    pub fn fast(self, k: impl Val) -> Pattern {
        let k = k.val() as f64;
        if k <= 0.0 {
            Pattern::silence()
        } else {
            self.fast_rat(Rat::from_f64(k))
        }
    }
    /// Play `k` times slower (one cycle stretched over `k` cycles).
    pub fn slow(self, k: impl Val) -> Pattern {
        let k = k.val() as f64;
        if k <= 0.0 {
            Pattern::silence()
        } else {
            self.fast_rat(Rat::ONE.div(Rat::from_f64(k)))
        }
    }
    /// Shift later by `cycles` (a rotation: `late(0.25)` = one beat in 4/4).
    pub fn late(self, cycles: impl Val) -> Pattern {
        let t = cycles.val() as f64;
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            inner
                .q(&q.span(q.from - t, q.to - t))
                .into_iter()
                .map(|e| Event {
                    start: e.start + t,
                    ..e
                })
                .collect()
        })
    }
    /// Shift earlier by `cycles`.
    pub fn early(self, cycles: impl Val) -> Pattern {
        let t = cycles.val();
        self.late(-t)
    }
    /// Reverse every cycle.
    pub fn rev(self) -> Pattern {
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let mut out = vec![];
            for c in q.cycles() {
                let Some(sub) = q.in_cycle(c) else { continue };
                let cf = c as f64;
                for e in inner.q(&q.span(cf, cf + 1.0)) {
                    let mut s = 2.0 * cf + 1.0 - e.end();
                    let mut d = e.dur;
                    if s < cf {
                        d -= cf - s;
                        s = cf;
                    }
                    if d > EPS && sub.has(s) {
                        out.push(Event {
                            start: s,
                            dur: d,
                            ..e
                        });
                    }
                }
            }
            out
        })
    }
    /// Forwards, then backwards (every odd cycle reversed).
    pub fn palindrome(self) -> Pattern {
        let r = self.clone().rev();
        let period = self.period().lcm(Rat::int(2));
        self.cycle_switch(period, r, |c| c.rem_euclid(2) == 1)
    }
    /// Rotate the start point by `1/n` cycle every cycle (`"a b c d".iter(4)`: abcd, bcda, cdab, dabc).
    pub fn iter(self, n: usize) -> Pattern {
        let n = n.max(1) as i64;
        let inner = self.clone();
        self.derive(self.period().lcm(Rat::int(n as u64)), move |q| {
            let mut out = vec![];
            for c in q.cycles() {
                let Some(sub) = q.in_cycle(c) else { continue };
                let sh = c.rem_euclid(n) as f64 / n as f64;
                out.extend(
                    inner
                        .q(&sub.span(sub.from + sh, sub.to + sh))
                        .into_iter()
                        .map(|e| Event {
                            start: e.start - sh,
                            ..e
                        }),
                );
            }
            out
        })
    }
    /// Repeat each event `n` times within its own duration (`"a b".ply(2)` = `"[a a] [b b]"`).
    pub fn ply(self, n: usize) -> Pattern {
        let n = n.max(1);
        let inner = self.clone();
        let lb = self.lookback();
        self.derive(self.period(), move |q| {
            let mut out = vec![];
            for e in inner.q(&q.span(q.from - lb, q.to)) {
                let d = e.dur / n as f64;
                for i in 0..n {
                    let s = e.start + d * i as f64;
                    if q.has(s) {
                        out.push(Event {
                            start: s,
                            dur: d,
                            ..e
                        });
                    }
                }
            }
            out
        })
    }
    /// Loop length `k` times longer (content keeps going: random choices keep varying).
    pub fn repeat(self, k: usize) -> Pattern {
        let p = self.period().mul(Rat::int(k.max(1) as u64));
        self.with_period(p)
    }
    /// Swing: delay the off-beat 8ths (one swing pair per beat). `0` = straight, `1` = full triplet swing.
    pub fn swing(self, amount: impl Val) -> Pattern {
        self.swing_impl(amount.val(), None)
    }
    /// 16th-note swing (two pairs per beat).
    pub fn swing16(self, amount: impl Val) -> Pattern {
        self.swing_impl(amount.val(), Some(2.0))
    }
    /// Swing with `pairs` swing pairs per cycle (`swing` uses one per beat).
    pub fn swing_by(self, amount: impl Val, pairs_per_cycle: impl Val) -> Pattern {
        let n = pairs_per_cycle.val().max(1.0) as f64;
        self.swing_impl(amount.val(), Some(-n))
    }
    /// `per_beat`: Some(+x) = x pairs per beat, Some(-x) = x pairs per cycle, None = 1 per beat.
    fn swing_impl(self, amount: f32, spec: Option<f64>) -> Pattern {
        let a = amount.clamp(-0.9, 2.5) as f64;
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let n = match spec {
                None => q.bpc.round().max(1.0),
                Some(x) if x > 0.0 => (q.bpc * x).round().max(1.0),
                Some(x) => -x,
            };
            let pair = 1.0 / n;
            let half = pair / 2.0;
            let mid = half * (1.0 + a / 3.0);
            let warp = |t: f64| {
                let c = (t / pair + EPS).floor();
                let x = t - c * pair;
                c * pair
                    + if x <= half {
                        x * mid / half
                    } else {
                        mid + (x - half) * (pair - mid) / half
                    }
            };
            inner
                .q(&q.span(q.from - pair, q.to + pair))
                .into_iter()
                .filter_map(|e| {
                    let s = warp(e.start);
                    let same = ((e.end() - EPS) / pair).floor() == (e.start / pair + EPS).floor();
                    let d = if same {
                        warp(e.end()) - s
                    } else {
                        e.dur - (s - e.start)
                    };
                    q.has(s).then_some(Event {
                        start: s,
                        dur: d.max(1e-6),
                        ..e
                    })
                })
                .collect()
        })
    }
    /// Seeded random timing (± `time`, in beats) and velocity (± `vel`) jitter.
    /// `p("hh*8").humanize(0.02, 0.1)`.
    pub fn humanize(self, time: impl Into<Time>, vel: impl Val) -> Pattern {
        let tb = time.into().0.abs();
        let va = vel.val().abs();
        let seed = splitmix(self.seed_value() ^ 0x4855_4D41_4E49_5A45);
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let tc = tb / q.bpc;
            inner
                .q(&q.span(q.from - tc, q.to + tc))
                .into_iter()
                .filter_map(|e| {
                    let k = tkey(e.start) ^ (e.pitch.to_bits() as u64).wrapping_mul(0x9E37_79B9);
                    let s = e.start + hash_bipolar(seed, k) as f64 * tc;
                    let v = (e.vel + hash_bipolar(seed ^ 0xABCD, k) * va).clamp(0.02, 1.0);
                    q.has(s).then_some(Event {
                        start: s,
                        vel: v,
                        ..e
                    })
                })
                .collect()
        })
    }

    // ------------------------------------------------------------- values

    /// Map every event (timing changes must keep onsets inside the queried window — prefer the
    /// dedicated combinators for that).
    pub fn map_events(self, f: impl Fn(Event) -> Event + Send + Sync + 'static) -> Pattern {
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            inner.q(q).into_iter().map(&f).collect()
        })
    }
    /// Keep only events matching a predicate: `.filter(|e| e.pitch < 60.0)`.
    pub fn filter(self, f: impl Fn(&Event) -> bool + Send + Sync + 'static) -> Pattern {
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            inner.q(q).into_iter().filter(|e| f(e)).collect()
        })
    }
    fn flat_map_events(
        self,
        f: impl Fn(Event, &mut Vec<Event>) + Send + Sync + 'static,
    ) -> Pattern {
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let mut out = vec![];
            for e in inner.q(q) {
                f(e, &mut out);
            }
            out
        })
    }
    /// Transpose by semitones.
    pub fn transpose(self, st: impl Val) -> Pattern {
        let st = st.val();
        self.map_events(move |e| Event {
            pitch: e.pitch + st,
            ..e
        })
    }
    /// Transpose up by semitones.
    pub fn up(self, st: impl Val) -> Pattern {
        self.transpose(st)
    }
    /// Transpose down by semitones.
    pub fn down(self, st: impl Val) -> Pattern {
        let st = st.val();
        self.transpose(-st)
    }
    /// Shift by octaves.
    pub fn oct(self, n: i32) -> Pattern {
        self.transpose(12 * n)
    }
    /// Set every velocity.
    pub fn vel(self, v: impl Val) -> Pattern {
        let v = v.val().clamp(0.0, 1.0);
        self.map_events(move |e| Event { vel: v, ..e })
    }
    /// Multiply velocities (accepts dB too: `.gain(-6.db())`).
    pub fn gain(self, k: impl Val) -> Pattern {
        let k = k.val();
        self.map_events(move |e| Event {
            vel: (e.vel * k).clamp(0.0, 1.0),
            ..e
        })
    }
    /// Velocities from a pattern of numbers: `p("hh*8").vels("1 .5 .8 .5")`.
    #[track_caller]
    pub fn vels(self, vels: &str) -> Pattern {
        let vp = Pattern::parse_or_panic(vels);
        let inner = self.clone();
        let lb = vp.lookback();
        let period = self.period().lcm(vp.period());
        self.derive(period, move |q| {
            let vs = vp.q(&q.span(q.from - lb, q.to));
            inner
                .q(q)
                .into_iter()
                .map(|e| {
                    match vs
                        .iter()
                        .rev()
                        .find(|v| v.start <= e.start + EPS && e.start < v.end() - EPS)
                    {
                        Some(v) => Event {
                            vel: v.pitch.clamp(0.0, 1.0),
                            ..e
                        },
                        None => e,
                    }
                })
                .collect()
        })
    }
    /// Scale note lengths (`legato(1.0)` = full step, `2.0` overlaps, `0.5` = half).
    pub fn legato(self, factor: impl Val) -> Pattern {
        let f = factor.val().max(0.0) as f64;
        self.map_events(move |e| Event {
            dur: e.dur * f,
            ..e
        })
    }
    /// Short notes (`legato(0.5)`).
    pub fn staccato(self) -> Pattern {
        self.legato(0.5)
    }
    /// Resolve scale degrees (0-based, as produced by [`n`](super::n)) into pitches of a key:
    /// `n("0 2 4").scale("C4 minor")` or `.scale(Key::new(C4, Scale::Dorian))`. Fractional
    /// degrees keep their fractional part as a semitone offset.
    #[track_caller]
    pub fn scale(self, key: impl Into<Key>) -> Pattern {
        let key = key.into();
        self.map_events(move |e| {
            let d = e.pitch.round();
            Event {
                pitch: key.degree(d as i32).0 + (e.pitch - d),
                ..e
            }
        })
    }
    /// Alias of [`Pattern::scale`].
    #[track_caller]
    pub fn in_key(self, key: impl Into<Key>) -> Pattern {
        self.scale(key)
    }
    /// Snap every pitch to the nearest note of a key.
    #[track_caller]
    pub fn quantize(self, key: impl Into<Key>) -> Pattern {
        let key = key.into();
        self.map_events(move |e| Event {
            pitch: key.quantize(e.pitch).0,
            ..e
        })
    }
    /// Turn every note into a chord of the given quality rooted on it: `p("c3 f3 g3").chord("m7")`.
    #[track_caller]
    pub fn chord(self, quality: &str) -> Pattern {
        let iv: Vec<f32> = match crate::theory::Chord::of(crate::theory::Pitch(0.0), quality) {
            Ok(c) => c.pitches(),
            Err(e) => panic!("{e}"),
        };
        self.flat_map_events(move |e, out| {
            out.extend(iv.iter().map(|i| Event {
                pitch: e.pitch + i,
                ..e
            }))
        })
    }
    /// Turn scale degrees into diatonic triads: `n("0 5 3 4").triads("C3 major")` (I vi IV V).
    #[track_caller]
    pub fn triads(self, key: impl Into<Key>) -> Pattern {
        let key = key.into();
        self.flat_map_events(move |e, out| {
            out.extend(
                key.triad(e.pitch.round() as i32)
                    .notes()
                    .iter()
                    .map(|p| Event { pitch: p.0, ..e }),
            )
        })
    }
    /// Turn scale degrees into diatonic seventh chords.
    #[track_caller]
    pub fn sevenths(self, key: impl Into<Key>) -> Pattern {
        let key = key.into();
        self.flat_map_events(move |e, out| {
            out.extend(
                key.seventh(e.pitch.round() as i32)
                    .notes()
                    .iter()
                    .map(|p| Event { pitch: p.0, ..e }),
            )
        })
    }
    /// Transform every chord (events sharing an onset) with a [`Chord`] function:
    /// `.map_chords(|c| c.drop2())`.
    pub fn map_chords(self, f: impl Fn(Chord) -> Chord + Send + Sync + 'static) -> Pattern {
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let mut out = vec![];
            for g in onset_groups(inner.q(q)) {
                let c = f(Chord::new(g.iter().map(|e| e.pitch)));
                let dur = g.iter().map(|e| e.dur).fold(0.0, f64::max);
                let vel = g.iter().map(|e| e.vel).sum::<f32>() / g.len() as f32;
                out.extend(c.notes().iter().map(|p| Event {
                    start: g[0].start,
                    dur,
                    pitch: p.0,
                    vel,
                }));
            }
            out
        })
    }
    /// Invert every chord `n` times (see [`Chord::inv`]).
    pub fn inv(self, n: i32) -> Pattern {
        self.map_chords(move |c| c.inv(n))
    }
    /// Drop-2 voicing for every chord.
    pub fn drop2(self) -> Pattern {
        self.map_chords(|c| c.drop2())
    }
    /// Open voicing for every chord.
    pub fn open(self) -> Pattern {
        self.map_chords(|c| c.open())
    }
    /// Strum chords: each successive note (low → high) starts `time` (beats) later.
    pub fn strum(self, time: impl Into<Time>) -> Pattern {
        let tb = time.into().0;
        let inner = self.clone();
        self.derive(self.period(), move |q| {
            let tc = tb / q.bpc;
            let back = (tc.abs() * 16.0).max(0.0);
            let mut out = vec![];
            for g in onset_groups(inner.q(&q.span(q.from - back, q.to))) {
                for (i, e) in g.into_iter().enumerate() {
                    let off = tc * i as f64;
                    let s = e.start + off;
                    if q.has(s) {
                        out.push(Event {
                            start: s,
                            dur: (e.dur - off).max(e.dur * 0.25),
                            ..e
                        });
                    }
                }
            }
            out
        })
    }
    /// Arpeggiate chords (events sharing an onset): `rate` notes per cycle, cycling through the
    /// chord in the given order for as long as the chord lasts.
    /// `p("<Cmaj7 Am7 Dm7 G'7>").arp("updown", 8)`.
    #[track_caller]
    pub fn arp(self, mode: impl Into<ArpMode>, rate: impl Val) -> Pattern {
        let mode = mode.into();
        let step = 1.0 / (rate.val().max(1e-3) as f64);
        let seed = splitmix(self.seed_value() ^ 0x4152_5050);
        let inner = self.clone();
        let lb = self.lookback();
        self.derive(self.period(), move |q| {
            let mut out = vec![];
            for g in onset_groups(inner.q(&q.span(q.from - lb, q.to))) {
                let s0 = g[0].start;
                let dur = g.iter().map(|e| e.dur).fold(0.0, f64::max);
                let order = mode.order(g.len());
                let mut i = 0usize;
                loop {
                    let t = s0 + step * i as f64;
                    if t >= s0 + dur - EPS || t >= q.to {
                        break;
                    }
                    if q.has(t) {
                        let idx = if mode == ArpMode::Random {
                            ((rand_at(seed, t) * g.len() as f32) as usize).min(g.len() - 1)
                        } else {
                            order[i % order.len()]
                        };
                        let e = g[idx];
                        out.push(Event {
                            start: t,
                            dur: step.min(s0 + dur - t),
                            ..e
                        });
                    }
                    i += 1;
                }
            }
            out
        })
    }

    // ------------------------------------------------------------- structure

    fn cycle_switch(
        self,
        period: Rat,
        alt: Pattern,
        pred: impl Fn(i64) -> bool + Send + Sync + 'static,
    ) -> Pattern {
        let base = self.clone();
        self.derive(period, move |q| {
            let mut out = vec![];
            for c in q.cycles() {
                let Some(sub) = q.in_cycle(c) else { continue };
                out.extend(if pred(c) { alt.q(&sub) } else { base.q(&sub) });
            }
            out
        })
    }
    /// Apply `f` on every `n`-th cycle, starting with the first (cycles 0, n, 2n...).
    /// `p("c d e f").every(4, |p| p.rev())`.
    pub fn every(self, n: usize, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        let n = n.max(1) as i64;
        let alt = f(self.clone());
        let period = self.period().lcm(alt.period()).lcm(Rat::int(n as u64));
        self.cycle_switch(period, alt, move |c| c.rem_euclid(n) == 0)
    }
    /// Apply `f` on the last cycle of every `n` (great for fills): `.last_of(4, |p| p.fast(2))`.
    pub fn last_of(self, n: usize, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        let n = n.max(1) as i64;
        let alt = f(self.clone());
        let period = self.period().lcm(alt.period()).lcm(Rat::int(n as u64));
        self.cycle_switch(period, alt, move |c| c.rem_euclid(n) == n - 1)
    }
    /// Apply `f` on the cycles where `pred(cycle)` is true. (The loop length can't see the
    /// predicate's period; use [`Pattern::repeat`] if needed.)
    pub fn when_cycle(
        self,
        pred: impl Fn(i64) -> bool + Send + Sync + 'static,
        f: impl FnOnce(Pattern) -> Pattern,
    ) -> Pattern {
        let alt = f(self.clone());
        let period = self.period().lcm(alt.period());
        self.cycle_switch(period, alt, pred)
    }
    /// Set the random seed used by the random combinators applied **after** this call
    /// (`sometimes`, `degrade`, `humanize`, random arps).
    pub fn seed(mut self, seed: u64) -> Pattern {
        self.seed = seed;
        self
    }
    /// Apply `f` to a seeded-random `prob` fraction of the events (decided per onset, so chords
    /// stay together; decisions differ from cycle to cycle but are reproducible).
    pub fn sometimes_by(self, prob: impl Val, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        let prob = prob.val();
        let alt = f(self.clone());
        let base = self.clone();
        let seed = splitmix(self.seed_value() ^ 0x534F_4D45);
        let period = self.period().lcm(alt.period());
        self.derive(period, move |q| {
            let mut out: Vec<Event> = base
                .q(q)
                .into_iter()
                .filter(|e| rand_at(seed, e.start) >= prob)
                .collect();
            out.extend(
                alt.q(q)
                    .into_iter()
                    .filter(|e| rand_at(seed, e.start) < prob),
            );
            out
        })
    }
    /// `f` on 50% of the events.
    pub fn sometimes(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        self.sometimes_by(0.5, f)
    }
    /// `f` on 75% of the events.
    pub fn often(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        self.sometimes_by(0.75, f)
    }
    /// `f` on 25% of the events.
    pub fn rarely(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        self.sometimes_by(0.25, f)
    }
    /// `f` on 10% of the events.
    pub fn almost_never(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        self.sometimes_by(0.1, f)
    }
    /// `f` on 90% of the events.
    pub fn almost_always(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        self.sometimes_by(0.9, f)
    }
    pub(crate) fn degrade_seeded(self, prob: f32, seed: u64) -> Pattern {
        let inner = self.clone();
        let seed = splitmix(seed ^ self.seed_value());
        self.derive(self.period(), move |q| {
            inner
                .q(q)
                .into_iter()
                .filter(|e| rand_at(seed, e.start) >= prob)
                .collect()
        })
    }
    /// Randomly drop a `prob` fraction of the events (seeded, per onset).
    pub fn degrade_by(self, prob: impl Val) -> Pattern {
        self.degrade_seeded(prob.val(), 0xDE6_2ADE)
    }
    /// Randomly drop half of the events.
    pub fn degrade(self) -> Pattern {
        self.degrade_by(0.5)
    }
    /// Layer another pattern on top (also the `&` operator).
    pub fn stack(self, other: impl Into<Pattern>) -> Pattern {
        Pattern::layers([self, other.into()])
    }
    /// Play `other` after this pattern's full loop (also the `+` operator).
    pub fn cat(self, other: impl Into<Pattern>) -> Pattern {
        Pattern::concat([self, other.into()])
    }
    /// Stack `f(self)` on top of `self`: `.superimpose(|p| p.up(12).vel(0.4))`.
    pub fn superimpose(self, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        let o = f(self.clone());
        self.stack(o)
    }
    /// Stack a shifted, transformed copy (Tidal's `off`): `.off(0.125, |p| p.up(7))`.
    pub fn off(self, cycles: impl Val, f: impl FnOnce(Pattern) -> Pattern) -> Pattern {
        let o = f(self.clone().late(cycles));
        self.stack(o)
    }
    /// Re-rhythm with a structure pattern: each hit (`x`/`1`) takes the notes sounding at that moment.
    /// `p("<c3 eb3>").struct_("x ~ x x ~ x x ~")`.
    #[track_caller]
    pub fn struct_(self, structure: &str) -> Pattern {
        let st = mini::parse(structure, Mode::Bool).unwrap_or_else(|e| panic!("{e}"));
        self.struct_pat(st)
    }
    /// [`Pattern::struct_`] with a pattern (every event counts as a hit).
    pub fn struct_pat(self, structure: Pattern) -> Pattern {
        let inner = self.clone();
        let lb = self.lookback();
        let period = self.period().lcm(structure.period());
        self.derive(period, move |q| {
            let hits = structure.q(q);
            if hits.is_empty() {
                return vec![];
            }
            let src = inner.q(&q.span(q.from - lb, q.to));
            let mut out = vec![];
            for h in hits {
                if !q.has(h.start) {
                    continue;
                }
                for e in &src {
                    if e.start <= h.start + EPS && h.start < e.end() - EPS {
                        out.push(Event {
                            start: h.start,
                            dur: h.dur,
                            pitch: e.pitch,
                            vel: e.vel,
                        });
                    }
                }
            }
            out
        })
    }
    /// Keep only events starting where the mask is on: `.mask("1 1 0 1")` / `.mask("<x ~> x")`.
    #[track_caller]
    pub fn mask(self, mask: &str) -> Pattern {
        let m = mini::parse(mask, Mode::Bool).unwrap_or_else(|e| panic!("{e}"));
        let inner = self.clone();
        let lb = m.lookback();
        let period = self.period().lcm(m.period());
        self.derive(period, move |q| {
            let on = m.q(&q.span(q.from - lb, q.to));
            inner
                .q(q)
                .into_iter()
                .filter(|e| {
                    on.iter()
                        .any(|h| h.start <= e.start + EPS && e.start < h.end() - EPS)
                })
                .collect()
        })
    }
    /// Euclidean re-rhythm: `k` hits over `n` steps (see [`Pattern::struct_`]).
    pub fn euclid(self, k: u32, n: u32) -> Pattern {
        self.euclid_rot(k, n, 0)
    }
    /// Euclidean re-rhythm rotated left by `r` steps.
    pub fn euclid_rot(self, k: u32, n: u32, r: i32) -> Pattern {
        let st = Pattern::fastcat_w(
            euclid_bools(k, n.max(1), r)
                .into_iter()
                .map(|b| {
                    (
                        if b {
                            Pattern::atom(1.0, DEFAULT_VEL)
                        } else {
                            Pattern::silence()
                        },
                        1.0,
                    )
                })
                .collect(),
        );
        self.struct_pat(st)
    }
}

impl<T: Into<Pattern>> Add<T> for Pattern {
    type Output = Pattern;
    /// `a + b`: play `b` after `a`.
    fn add(self, o: T) -> Pattern {
        self.cat(o)
    }
}
impl<T: Into<Pattern>> BitAnd<T> for Pattern {
    type Output = Pattern;
    /// `a & b`: play together.
    fn bitand(self, o: T) -> Pattern {
        self.stack(o)
    }
}
impl Mul<usize> for Pattern {
    type Output = Pattern;
    /// `a * 4`: a loop four times as long (repeat).
    fn mul(self, k: usize) -> Pattern {
        self.repeat(k)
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn is<T: Send + Sync>() {}
    is::<Pattern>();
    let _ = |q: Q| q;
}
