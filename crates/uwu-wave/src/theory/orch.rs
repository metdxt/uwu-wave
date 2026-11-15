//! Orchestration helpers: instrument ranges and section voicing.

use super::notes::*;
use super::{Chord, Pitch};

/// A playable pitch range (inclusive), e.g. `Range::VIOLIN` = G3..=A7.
///
/// ```
/// use uwu_wave::prelude::*;
/// assert_eq!(Range::CELLO.fit(C7), C6);   // octave-shifted into range
/// assert!(Range::FLUTE.contains(A5));
/// let low = Range::new(C2, C4);
/// # let _ = low;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub lo: Pitch,
    pub hi: Pitch,
}

#[allow(non_upper_case_globals)]
impl Range {
    pub const fn new(lo: Pitch, hi: Pitch) -> Range {
        Range { lo, hi }
    }
    // strings
    pub const VIOLIN: Range = Range::new(G3, A7);
    pub const VIOLA: Range = Range::new(C3, E6);
    pub const CELLO: Range = Range::new(C2, C6);
    pub const BASS: Range = Range::new(E1, G4);
    pub const HARP: Range = Range::new(B0, Gs7);
    // woodwinds
    pub const PICCOLO: Range = Range::new(D5, C8);
    pub const FLUTE: Range = Range::new(C4, C7);
    pub const OBOE: Range = Range::new(Bb3, A6);
    pub const COR_ANGLAIS: Range = Range::new(E3, C6);
    pub const CLARINET: Range = Range::new(E3, C7);
    pub const BASS_CLARINET: Range = Range::new(Bb1, Bb5);
    pub const BASSOON: Range = Range::new(Bb1, E5);
    pub const CONTRABASSOON: Range = Range::new(Bb0, C4);
    pub const ALTO_SAX: Range = Range::new(Db3, A5);
    pub const TENOR_SAX: Range = Range::new(Ab2, E5);
    // brass
    pub const HORN: Range = Range::new(B1, F5);
    pub const TRUMPET: Range = Range::new(Fs3, D6);
    pub const TROMBONE: Range = Range::new(E2, F5);
    pub const BASS_TROMBONE: Range = Range::new(Bb1, Bb4);
    pub const TUBA: Range = Range::new(D1, F4);
    // percussion & keys
    pub const TIMPANI: Range = Range::new(D2, A3);
    pub const GLOCKENSPIEL: Range = Range::new(G5, C8);
    pub const XYLOPHONE: Range = Range::new(F4, C8);
    pub const VIBRAPHONE: Range = Range::new(F3, F6);
    pub const MARIMBA: Range = Range::new(C2, C7);
    pub const CELESTA: Range = Range::new(C4, C8);
    pub const TUBULAR_BELLS: Range = Range::new(C4, F5);
    pub const PIANO: Range = Range::new(A0, C8);
    pub const GUITAR: Range = Range::new(E2, B5);
    // voices
    pub const SOPRANO: Range = Range::new(C4, A5);
    pub const ALTO: Range = Range::new(F3, D5);
    pub const TENOR: Range = Range::new(C3, A4);
    pub const BARITONE: Range = Range::new(A2, F4);
    pub const BASS_VOICE: Range = Range::new(E2, E4);

    /// Is the pitch inside the range?
    pub fn contains(&self, p: impl Into<Pitch>) -> bool {
        let p = p.into().0;
        p >= self.lo.0 - 1e-3 && p <= self.hi.0 + 1e-3
    }
    /// The middle of the range.
    pub fn center(&self) -> Pitch {
        Pitch((self.lo.0 + self.hi.0) * 0.5)
    }
    /// Shift by whole octaves into the range (if the range is narrower than an octave, the
    /// nearest octave placement to the range is used).
    pub fn fit(&self, p: impl Into<Pitch>) -> Pitch {
        let mut p = p.into().0;
        while p < self.lo.0 - 1e-3 {
            p += 12.0;
        }
        while p > self.hi.0 + 1e-3 {
            p -= 12.0;
        }
        if p < self.lo.0 - 1e-3 {
            // range narrower than an octave and the note falls in the gap
            if (self.lo.0 - p) > (p + 12.0 - self.hi.0) {
                p += 12.0;
            }
        }
        Pitch(p)
    }
    /// The comfortable middle part of the range (`margin` semitones trimmed at both ends).
    pub fn inner(&self, margin: f32) -> Range {
        let m = margin.min((self.hi.0 - self.lo.0) / 2.0 - 1.0).max(0.0);
        Range::new(Pitch(self.lo.0 + m), Pitch(self.hi.0 - m))
    }
}

impl Chord {
    /// Spread the chord over several instruments, top note first: note `i` counted from the top
    /// goes to `ranges[i]` (octave-shifted into it). Missing notes repeat the root.
    /// `C4.maj().spread_over(&[Range::VIOLIN, Range::VIOLA, Range::CELLO])`.
    pub fn spread_over(&self, ranges: &[Range]) -> Vec<Pitch> {
        let mut notes: Vec<Pitch> = self.notes().to_vec();
        notes.reverse();
        ranges
            .iter()
            .enumerate()
            .map(|(i, r)| r.fit(notes.get(i).copied().unwrap_or(self.root())))
            .collect()
    }
    /// Octave-shift every note into a range.
    pub fn fit(&self, range: Range) -> Chord {
        Chord::new(self.notes().iter().map(|&p| range.fit(p)))
    }
}

/// Default ranges (top → bottom) used by [`voice_satb`]: violins I, violins II, violas, cellos
/// (comfortable registers, not the extremes).
pub const STRING_SECTION: [Range; 4] = [
    Range::new(G4, A5),
    Range::new(C4, E5),
    Range::new(F3, A4),
    Range::new(C2, D3),
];

/// Four-part voicing of a progression with smooth voice leading: returns `[soprano, alto, tenor, bass]`
/// lines (one pitch per chord). The bass takes the chord's lowest note (so slash chords and
/// inversions are respected); the three upper voices share the remaining chord tones (for seventh
/// chords the root is dropped from the top voices), move as little as possible, never cross,
/// and stay inside the given ranges (top → bottom).
///
/// ```
/// use uwu_wave::prelude::*;
/// let [s, a, t, b] = voice_satb(&prog("I vi IV V", "C major"), &[Range::SOPRANO, Range::ALTO, Range::TENOR, Range::BASS_VOICE]);
/// assert_eq!(b[0].pitch_class(), 0.0);
/// assert!(s.iter().zip(&a).all(|(s, a)| s.0 > a.0));
/// # let _ = t;
/// ```
pub fn voice_satb(chords: &[Chord], ranges: &[Range; 4]) -> [Vec<Pitch>; 4] {
    let mut out: [Vec<Pitch>; 4] = Default::default();
    let mut prev: [f32; 4] = [
        ranges[0].center().0,
        ranges[1].center().0,
        ranges[2].center().0,
        ranges[3].center().0,
    ];
    for ch in chords {
        if ch.is_empty() {
            for (i, v) in out.iter_mut().enumerate() {
                v.push(Pitch(prev[i]));
            }
            continue;
        }
        // bass: lowest chord note's pitch class, nearest to the previous bass, in range
        let bass_pc = ch.bass().pitch_class();
        let bass = nearest_pc(bass_pc, prev[3], &ranges[3]);
        // upper pitch classes
        let mut pcs: Vec<f32> = vec![];
        for p in ch.notes() {
            let pc = p.pitch_class();
            if !pcs.iter().any(|x| (x - pc).abs() < 1e-3) {
                pcs.push(pc);
            }
        }
        let root_pc = ch.root().pitch_class();
        let mut upper: Vec<f32> = pcs.clone();
        if upper.len() > 3 {
            // drop the root (the bass or another voice covers it), then the fifth if still too many
            upper.retain(|&x| (x - root_pc).abs() > 1e-3);
            if upper.len() > 3 {
                let fifth = (root_pc + 7.0).rem_euclid(12.0);
                if let Some(i) = upper.iter().position(|&x| (x - fifth).abs() < 1e-3) {
                    upper.remove(i);
                }
            }
            upper.truncate(3);
        }
        while upper.len() < 3 {
            // double the root (or the first tone)
            upper.push(if pcs.iter().any(|x| (x - root_pc).abs() < 1e-3) {
                root_pc
            } else {
                pcs[0]
            });
        }
        // try every assignment of pitch classes to voices; pick the smoothest valid one
        let perms = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        let mut best: Option<([f32; 3], f32)> = None;
        for perm in perms {
            let mut v = [0.0f32; 3];
            for k in 0..3 {
                v[k] = nearest_pc(upper[perm[k]], prev[k], &ranges[k]).0;
            }
            // no crossing; keep above the bass
            let mut cost: f32 = (0..3).map(|k| (v[k] - prev[k]).abs()).sum();
            if !(v[0] > v[1] && v[1] > v[2]) {
                cost += 50.0;
            }
            if v[2] <= bass.0 {
                cost += 50.0;
            }
            // avoid more than an octave between adjacent upper voices
            if v[0] - v[1] > 12.0 || v[1] - v[2] > 12.0 {
                cost += 6.0;
            }
            if best.is_none_or(|(_, c)| cost < c) {
                best = Some((v, cost));
            }
        }
        let (v, _) = best.unwrap();
        let line = [v[0], v[1], v[2], bass.0];
        for (i, p) in line.iter().enumerate() {
            out[i].push(Pitch(*p));
            prev[i] = *p;
        }
    }
    out
}

/// The pitch with pitch class `pc` nearest to `target`, inside `range` when possible.
fn nearest_pc(pc: f32, target: f32, range: &Range) -> Pitch {
    let base = target - (target - pc).rem_euclid(12.0);
    let mut cands = [base - 12.0, base, base + 12.0, base + 24.0];
    cands.sort_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()));
    cands
        .iter()
        .copied()
        .find(|&c| range.contains(Pitch(c)))
        .map(Pitch)
        .unwrap_or_else(|| range.fit(Pitch(cands[0])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::Key;

    #[test]
    fn ranges_fit() {
        assert_eq!(Range::CELLO.fit(C7), C5.oct(1));
        assert_eq!(Range::VIOLIN.fit(C2), C4);
        assert!(Range::VIOLIN.contains(G3));
        assert!(!Range::VIOLIN.contains(Fs3));
        let narrow = Range::new(C4, E4);
        assert_eq!(narrow.fit(D5), D4);
        let s = C4
            .maj()
            .spread_over(&[Range::VIOLIN, Range::VIOLA, Range::CELLO]);
        assert_eq!(s, vec![G4, E4, C4]);
    }

    #[test]
    fn satb_voice_leading() {
        let chords = Key::major(C4).prog("I vi IV V7 I");
        let [s, a, t, b] = voice_satb(&chords, &STRING_SECTION);
        for i in 0..chords.len() {
            assert!(
                s[i].0 > a[i].0 && a[i].0 > t[i].0 && t[i].0 > b[i].0,
                "{i}: {} {} {} {}",
                s[i],
                a[i],
                t[i],
                b[i]
            );
            for (v, r) in [&s, &a, &t, &b].iter().zip(STRING_SECTION.iter()) {
                assert!(r.contains(v[i]) || (v[i].0 - r.center().0).abs() < 18.0);
            }
            // every voice is a chord tone
            for v in [&s, &a, &t, &b] {
                assert!(
                    chords[i]
                        .notes()
                        .iter()
                        .any(|n| (n.pitch_class() - v[i].pitch_class()).abs() < 1e-3)
                );
            }
        }
        // bass follows the roots
        assert_eq!(
            b.iter().map(|p| p.pitch_class() as i32).collect::<Vec<_>>(),
            vec![0, 9, 5, 7, 0]
        );
        // smooth: upper voices move by at most a fourth
        for v in [&s, &a, &t] {
            for w in v.windows(2) {
                assert!((w[1].0 - w[0].0).abs() <= 5.0, "{} -> {}", w[0], w[1]);
            }
        }
    }
}
