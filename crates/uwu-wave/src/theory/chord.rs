//! Chords: note sets with inversions, voicings, slash basses and chord-symbol parsing.

use super::{Pitch, TheoryError, err, midi_of, parse_note_prefix};
use crate::units::Val;
use std::fmt;

/// A chord: a set of pitches (kept sorted low → high) plus the root it was built on.
///
/// ```
/// use uwu_wave::prelude::*;
/// let c = C4.maj7();                               // [C4 E4 G4 B4]
/// assert_eq!(c.clone().inv(1).bass(), E4);         // first inversion
/// let open = C4.maj().open();                      // [C4 G4 E5]
/// let d2 = C4.maj7().drop2();                      // [G3 C4 E4 B4]
/// let slash = C4.maj().over(E4);                   // [E3 C4 G4]
/// let parsed = Chord::parse("F#m7b5").unwrap();    // [F#4 A4 C5 E5]
/// let bb_d = Chord::parse("Bb/D").unwrap();        // [D4 Bb4 F5]
/// # let _ = (open, d2, slash, parsed, bb_d);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    root: Pitch,
    notes: Vec<Pitch>,
}

/// Chord-quality table: symbol aliases → intervals.
const QUALITIES: &[(&[&str], &[f32])] = &[
    (&["", "M", "maj", "major"], &[0.0, 4.0, 7.0]),
    (&["m", "min", "minor", "-"], &[0.0, 3.0, 7.0]),
    (&["dim", "o", "°"], &[0.0, 3.0, 6.0]),
    (&["aug", "+"], &[0.0, 4.0, 8.0]),
    (&["sus2", "2"], &[0.0, 2.0, 7.0]),
    (&["sus4", "sus", "4"], &[0.0, 5.0, 7.0]),
    (&["5", "power"], &[0.0, 7.0]),
    (&["6", "maj6", "M6"], &[0.0, 4.0, 7.0, 9.0]),
    (&["m6", "min6"], &[0.0, 3.0, 7.0, 9.0]),
    (&["69", "6/9", "6add9"], &[0.0, 4.0, 7.0, 9.0, 14.0]),
    (&["m69", "m6/9"], &[0.0, 3.0, 7.0, 9.0, 14.0]),
    (&["7", "dom7", "dom"], &[0.0, 4.0, 7.0, 10.0]),
    (
        &["maj7", "M7", "Δ", "Δ7", "ma7", "j7"],
        &[0.0, 4.0, 7.0, 11.0],
    ),
    (&["m7", "min7", "-7", "mi7"], &[0.0, 3.0, 7.0, 10.0]),
    (
        &["mmaj7", "mM7", "minmaj7", "m(maj7)", "-Δ7"],
        &[0.0, 3.0, 7.0, 11.0],
    ),
    (&["dim7", "o7", "°7"], &[0.0, 3.0, 6.0, 9.0]),
    (
        &["m7b5", "ø", "ø7", "min7b5", "-7b5", "h7"],
        &[0.0, 3.0, 6.0, 10.0],
    ),
    (
        &["aug7", "+7", "7#5", "7+5", "7aug"],
        &[0.0, 4.0, 8.0, 10.0],
    ),
    (
        &["maj7#5", "augmaj7", "+M7", "+maj7"],
        &[0.0, 4.0, 8.0, 11.0],
    ),
    (&["7b5"], &[0.0, 4.0, 6.0, 10.0]),
    (&["7sus4", "7sus"], &[0.0, 5.0, 7.0, 10.0]),
    (&["7sus2"], &[0.0, 2.0, 7.0, 10.0]),
    (&["add9", "add2"], &[0.0, 4.0, 7.0, 14.0]),
    (&["madd9", "madd2", "m(add9)"], &[0.0, 3.0, 7.0, 14.0]),
    (&["add11", "add4"], &[0.0, 4.0, 7.0, 17.0]),
    (&["9", "dom9"], &[0.0, 4.0, 7.0, 10.0, 14.0]),
    (&["maj9", "M9", "Δ9"], &[0.0, 4.0, 7.0, 11.0, 14.0]),
    (&["m9", "min9", "-9"], &[0.0, 3.0, 7.0, 10.0, 14.0]),
    (&["mmaj9", "mM9"], &[0.0, 3.0, 7.0, 11.0, 14.0]),
    (&["7b9"], &[0.0, 4.0, 7.0, 10.0, 13.0]),
    (&["7#9"], &[0.0, 4.0, 7.0, 10.0, 15.0]),
    (&["7#11"], &[0.0, 4.0, 7.0, 10.0, 18.0]),
    (&["maj7#11", "M7#11"], &[0.0, 4.0, 7.0, 11.0, 18.0]),
    (&["9sus4", "9sus"], &[0.0, 5.0, 7.0, 10.0, 14.0]),
    (&["11", "dom11"], &[0.0, 4.0, 7.0, 10.0, 14.0, 17.0]),
    (&["m11", "min11", "-11"], &[0.0, 3.0, 7.0, 10.0, 14.0, 17.0]),
    (&["maj11", "M11"], &[0.0, 4.0, 7.0, 11.0, 14.0, 17.0]),
    (&["13", "dom13"], &[0.0, 4.0, 7.0, 10.0, 14.0, 21.0]),
    (&["maj13", "M13"], &[0.0, 4.0, 7.0, 11.0, 14.0, 21.0]),
    (&["m13", "min13", "-13"], &[0.0, 3.0, 7.0, 10.0, 14.0, 21.0]),
];

/// Intervals for a chord-quality symbol (`"m7"`, `"maj9"`, `"sus4"`...).
pub(crate) fn quality_intervals(q: &str) -> Option<&'static [f32]> {
    QUALITIES
        .iter()
        .find(|(names, _)| names.contains(&q))
        .map(|(_, iv)| *iv)
}

impl Chord {
    /// A chord from explicit pitches; the root is the lowest note.
    pub fn new<P: Into<Pitch>>(notes: impl IntoIterator<Item = P>) -> Chord {
        let mut notes: Vec<Pitch> = notes.into_iter().map(Into::into).collect();
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let root = notes.first().copied().unwrap_or_default();
        Chord { root, notes }
    }
    /// A chord from semitone intervals above a root: `Chord::from_intervals(C4, &[0.0, 4.0, 7.0])`.
    pub fn from_intervals(root: impl Into<Pitch>, intervals: &[f32]) -> Chord {
        let root = root.into();
        let mut c = Chord::new(intervals.iter().map(|&i| root.up(i)));
        c.root = root;
        c
    }
    /// A chord of a named quality on a root: `Chord::of(D3, "m7")`.
    ///
    /// Qualities: `""`/`maj`/`M`, `m`/`min`/`-`, `dim`/`°`, `aug`/`+`, `sus2`, `sus4`/`sus`, `5`, `6`, `m6`,
    /// `69`, `7`, `maj7`/`M7`/`Δ`, `m7`, `mmaj7`, `dim7`/`°7`, `m7b5`/`ø`, `aug7`/`7#5`, `maj7#5`,
    /// `7b5`, `7sus4`, `7sus2`, `add9`, `madd9`, `add11`, `9`, `maj9`, `m9`, `mmaj9`, `7b9`, `7#9`,
    /// `7#11`, `maj7#11`, `9sus4`, `11`, `m11`, `maj11`, `13`, `maj13`, `m13`.
    pub fn of(root: impl Into<Pitch>, quality: &str) -> Result<Chord, TheoryError> {
        match quality_intervals(quality) {
            Some(iv) => Ok(Chord::from_intervals(root, iv)),
            None => err(format!(
                "unknown chord quality {quality:?} (try maj, m, 7, maj7, m7, dim, sus4, m7b5, 9...)"
            )),
        }
    }
    /// Parse a chord symbol: `"C"`, `"Cmaj7"`, `"F#m7b5"`, `"Bbsus4"`, `"G7"`, `"Bb/D"` (slash bass).
    ///
    /// Roots without an octave sit in octave 4 (`"Am"` = A4 C5 E5); digits right after the root are
    /// part of the quality (`"G7"` is G dominant seventh). To give an explicit octave, separate the
    /// quality with an apostrophe: `"g2'7"`, `"c3'maj7"`, `"e3'"` (major).
    pub fn parse(sym: &str) -> Result<Chord, TheoryError> {
        let s = sym.trim();
        let bad = || {
            TheoryError(format!(
                "not a chord symbol: {sym:?} (expected e.g. C, Am, F#m7b5, Bbmaj7, G7, C/E, c3'maj7)"
            ))
        };
        // slash bass (but "6/9" is a quality)
        let (main, bass) = match s.rfind('/') {
            Some(i) if !s[..i].ends_with('6') || !s[i + 1..].starts_with('9') => {
                (&s[..i], Some(&s[i + 1..]))
            }
            _ => (s, None),
        };
        let (root, quality) = if let Some(i) = main.find('\'') {
            let (semi, oct, rest) = parse_note_prefix(&main[..i], true).ok_or_else(bad)?;
            if !rest.is_empty() {
                return Err(bad());
            }
            (Pitch(midi_of(semi, oct.unwrap_or(4))), &main[i + 1..])
        } else {
            let (semi, _, rest) = parse_note_prefix(main, false).ok_or_else(bad)?;
            (Pitch(midi_of(semi, 4)), rest)
        };
        let mut chord =
            Chord::of(root, quality).map_err(|e| TheoryError(format!("{e} in chord {sym:?}")))?;
        if let Some(b) = bass {
            let (semi, oct, rest) = parse_note_prefix(b, true).ok_or_else(bad)?;
            if !rest.is_empty() {
                return Err(bad());
            }
            chord = chord.over(Pitch(midi_of(semi, oct.unwrap_or(4))));
        }
        Ok(chord)
    }

    /// The pitches, low → high.
    pub fn notes(&self) -> &[Pitch] {
        &self.notes
    }
    /// The pitches as MIDI numbers.
    pub fn pitches(&self) -> Vec<f32> {
        self.notes.iter().map(|p| p.0).collect()
    }
    /// The root the chord was built on (not necessarily the lowest note after inversions).
    pub fn root(&self) -> Pitch {
        self.root
    }
    /// The lowest note.
    pub fn bass(&self) -> Pitch {
        self.notes.first().copied().unwrap_or(self.root)
    }
    /// The highest note.
    pub fn top(&self) -> Pitch {
        self.notes.last().copied().unwrap_or(self.root)
    }
    pub fn len(&self) -> usize {
        self.notes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
    fn sort(mut self) -> Chord {
        self.notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        self
    }
    /// `n`-th inversion: the lowest note moves up an octave `n` times (negative `n`: the highest
    /// note moves down).
    pub fn inv(mut self, n: i32) -> Chord {
        if self.notes.is_empty() {
            return self;
        }
        for _ in 0..n.unsigned_abs() {
            if n > 0 {
                self.notes[0] = self.notes[0].oct(1);
            } else {
                let l = self.notes.len() - 1;
                self.notes[l] = self.notes[l].oct(-1);
            }
            self = self.sort();
        }
        self
    }
    /// Open voicing: every second voice (2nd, 4th, ...) moves up an octave (`C E G` → `C G E'`).
    pub fn open(mut self) -> Chord {
        if self.notes.len() >= 3 {
            for i in (1..self.notes.len()).step_by(2) {
                self.notes[i] = self.notes[i].oct(1);
            }
        }
        self.sort()
    }
    /// Drop-2 voicing: the second-highest voice drops an octave.
    pub fn drop2(self) -> Chord {
        self.drop_n(2)
    }
    /// Drop-3 voicing: the third-highest voice drops an octave.
    pub fn drop3(self) -> Chord {
        self.drop_n(3)
    }
    fn drop_n(mut self, k: usize) -> Chord {
        let l = self.notes.len();
        if l >= k && l >= 3 {
            self.notes[l - k] = self.notes[l - k].oct(-1);
        }
        self.sort()
    }
    /// Slash chord: put `bass`'s pitch class directly below the chord (removing that pitch class
    /// from the upper voices). `C4.maj().over(E4)` → `[E3 C4 G4]`. Use [`Chord::with_bass`] for an
    /// exact bass pitch.
    pub fn over(mut self, bass: impl Into<Pitch>) -> Chord {
        let bass = bass.into();
        let pc = bass.pitch_class();
        let same = |p: &Pitch| (p.pitch_class() - pc).abs() < 1e-3;
        if self.notes.iter().filter(|p| !same(p)).count() >= 2 {
            self.notes.retain(|p| !same(p));
        }
        let lowest = self.bass();
        let mut diff = (lowest.0 - pc).rem_euclid(12.0);
        if diff < 1e-3 {
            diff = 12.0;
        }
        self.notes.insert(0, Pitch(lowest.0 - diff));
        self.sort()
    }
    /// Add an exact bass note (any octave), keeping all upper voices.
    pub fn with_bass(mut self, bass: impl Into<Pitch>) -> Chord {
        self.notes.push(bass.into());
        self.sort()
    }
    /// Add a note.
    pub fn add_note(mut self, p: impl Into<Pitch>) -> Chord {
        self.notes.push(p.into());
        self.sort()
    }
    /// Remove every note of a pitch class (e.g. omit the fifth: `c.omit(G4)`).
    pub fn omit(mut self, p: impl Into<Pitch>) -> Chord {
        let pc = p.into().pitch_class();
        self.notes.retain(|n| (n.pitch_class() - pc).abs() > 1e-3);
        self
    }
    /// Transpose by semitones.
    pub fn up(mut self, st: impl Val) -> Chord {
        let st = st.val();
        self.root = self.root.up(st);
        self.notes.iter_mut().for_each(|p| *p = p.up(st));
        self
    }
    /// Transpose down by semitones.
    pub fn down(self, st: impl Val) -> Chord {
        let st = st.val();
        self.up(-st)
    }
    /// Alias of [`Chord::up`].
    pub fn transpose(self, st: impl Val) -> Chord {
        self.up(st)
    }
    /// Shift by octaves.
    pub fn oct(self, n: i32) -> Chord {
        self.up(12 * n)
    }
    /// Double the root an octave below.
    pub fn with_sub(self) -> Chord {
        let r = self.root.oct(-1);
        let r = if r.0 >= self.bass().0 { r.oct(-1) } else { r };
        self.add_note(r)
    }
    /// The inversion/octave placement of this chord closest to `other` (smooth voice leading).
    pub fn near(self, other: &Chord) -> Chord {
        if self.notes.is_empty() || other.notes.is_empty() {
            return self;
        }
        let target: Vec<f32> = other.pitches();
        let cost = |c: &Chord| -> f32 {
            let p = c.pitches();
            if p.len() == target.len() {
                p.iter().zip(&target).map(|(a, b)| (a - b).abs()).sum()
            } else {
                // every note to its nearest neighbour, both ways
                let near = |x: f32, set: &[f32]| {
                    set.iter().map(|y| (x - y).abs()).fold(f32::MAX, f32::min)
                };
                p.iter().map(|&x| near(x, &target)).sum::<f32>()
                    + target.iter().map(|&x| near(x, &p)).sum::<f32>()
            }
        };
        let n = self.notes.len() as i32;
        let mut best = self.clone();
        let mut best_cost = cost(&best);
        // move the chord into the target's register first
        let shift = ((other.notes[0].0 - self.notes[0].0) / 12.0).round() as i32;
        let base = self.oct(shift);
        for k in -2 * n..=2 * n {
            let c = base.clone().inv(k);
            let cc = cost(&c);
            if cc < best_cost - 1e-4 {
                best = c;
                best_cost = cc;
            }
        }
        best
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self.notes.iter().map(|p| p.name()).collect();
        write!(f, "[{}]", names.join(" "))
    }
}

impl std::str::FromStr for Chord {
    type Err = TheoryError;
    fn from_str(s: &str) -> Result<Chord, TheoryError> {
        Chord::parse(s)
    }
}

impl From<&str> for Chord {
    /// Parses a chord symbol; **panics** with a helpful message on bad input.
    #[track_caller]
    fn from(s: &str) -> Chord {
        Chord::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }
}

impl From<Vec<Pitch>> for Chord {
    fn from(v: Vec<Pitch>) -> Chord {
        Chord::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::super::notes::*;
    use super::*;

    fn p(c: &Chord) -> Vec<f32> {
        c.pitches()
    }

    #[test]
    fn builders_and_voicings() {
        assert_eq!(p(&C4.maj()), vec![60.0, 64.0, 67.0]);
        assert_eq!(p(&C4.maj().inv(1)), vec![64.0, 67.0, 72.0]);
        assert_eq!(p(&C4.maj().inv(2)), vec![67.0, 72.0, 76.0]);
        assert_eq!(p(&C4.maj().inv(-1)), vec![55.0, 60.0, 64.0]);
        assert_eq!(p(&C4.maj().open()), vec![60.0, 67.0, 76.0]);
        assert_eq!(p(&C4.maj7().drop2()), vec![55.0, 60.0, 64.0, 71.0]);
        assert_eq!(p(&C4.maj().over(E4)), vec![52.0, 60.0, 67.0]);
        assert_eq!(p(&C4.maj().over(Bb2)), vec![58.0, 60.0, 64.0, 67.0]);
        assert_eq!(C4.maj7().inv(1).root(), C4);
    }

    #[test]
    fn parse_symbols() {
        assert_eq!(p(&Chord::parse("C").unwrap()), vec![60.0, 64.0, 67.0]);
        assert_eq!(
            p(&Chord::parse("Cmaj7").unwrap()),
            vec![60.0, 64.0, 67.0, 71.0]
        );
        assert_eq!(
            p(&Chord::parse("F#m7b5").unwrap()),
            vec![66.0, 69.0, 72.0, 76.0]
        );
        assert_eq!(
            p(&Chord::parse("G7").unwrap()),
            vec![67.0, 71.0, 74.0, 77.0]
        );
        assert_eq!(p(&Chord::parse("Bb/D").unwrap()), vec![62.0, 70.0, 77.0]);
        assert_eq!(
            p(&Chord::parse("g2'7").unwrap()),
            vec![43.0, 47.0, 50.0, 53.0]
        );
        assert_eq!(p(&Chord::parse("Am").unwrap()), vec![69.0, 72.0, 76.0]);
        assert_eq!(p(&Chord::parse("Csus4").unwrap()), vec![60.0, 65.0, 67.0]);
        assert_eq!(Chord::parse("C6/9").unwrap().len(), 5);
        assert_eq!(Chord::parse("Ebm9").unwrap().root(), Eb4);
        assert!(Chord::parse("Cblah").is_err());
        assert!(Chord::parse("X7").is_err());
    }

    #[test]
    fn voice_leading() {
        let g = G4.dom7().near(&C4.maj7());
        // G7 near Cmaj7 should stay in the same register
        assert!((g.bass().0 - 60.0).abs() <= 7.0, "{g}");
    }
}
