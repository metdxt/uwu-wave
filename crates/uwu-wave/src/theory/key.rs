//! Keys (root + scale), scale degrees, quantization and roman-numeral progressions.

use super::{Chord, Pitch, Scale, TheoryError, err, midi_of, parse_note_prefix};
use crate::units::Val;
use std::fmt;

/// A key: a root pitch plus a [`Scale`]. The root's octave matters — degrees are counted from it.
///
/// ```
/// use uwu_wave::prelude::*;
/// let k = Key::new(C4, Scale::Minor);
/// assert_eq!(k.degree(0), C4);
/// assert_eq!(k.degree(4), G4);
/// assert_eq!(k.degree(7), C5);      // wraps up an octave
/// assert_eq!(k.degree(-2), Ab3);    // and down
/// assert_eq!(k.quantize(E4), Eb4);  // snap to the scale
/// let k2 = Key::parse("F# dorian").unwrap();
/// let chords = Key::major(C3).prog("I V vi IV");
/// # let _ = (k2, chords);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub root: Pitch,
    pub scale: Scale,
}

impl Key {
    pub fn new(root: impl Into<Pitch>, scale: Scale) -> Key {
        Key {
            root: root.into(),
            scale,
        }
    }
    /// Major key.
    pub fn major(root: impl Into<Pitch>) -> Key {
        Key::new(root, Scale::Major)
    }
    /// Natural minor key.
    pub fn minor(root: impl Into<Pitch>) -> Key {
        Key::new(root, Scale::Minor)
    }
    /// Parse `"C minor"`, `"F# dorian"`, `"Bb3 major"`, `"eb harmonic minor"`, `"Am"`, `"C"`.
    /// Roots without an octave are in octave 4.
    pub fn parse(s: &str) -> Result<Key, TheoryError> {
        let t = s.trim();
        let Some((semi, oct, rest)) = parse_note_prefix(t, true) else {
            return err(format!(
                "not a key: {s:?} (expected e.g. \"C minor\", \"F# dorian\", \"Am\")"
            ));
        };
        let rest = rest.trim().trim_start_matches(':').trim();
        let scale = Scale::parse(rest).map_err(|e| TheoryError(format!("{e} in key {s:?}")))?;
        Ok(Key::new(Pitch(midi_of(semi, oct.unwrap_or(4))), scale))
    }
    /// Same key with a different scale (same root).
    pub fn with_scale(self, scale: Scale) -> Key {
        Key { scale, ..self }
    }
    /// Transpose the key.
    pub fn up(self, st: impl Val) -> Key {
        Key {
            root: self.root.up(st),
            ..self
        }
    }
    /// Shift the key's root octave.
    pub fn oct(self, n: i32) -> Key {
        Key {
            root: self.root.oct(n),
            ..self
        }
    }
    /// The pitch of a **0-based** scale degree (0 = root, 2 = third in a heptatonic scale). Negative
    /// degrees and degrees past the scale length wrap octaves.
    pub fn degree(&self, d: i32) -> Pitch {
        self.root.up(self.scale.offset(d))
    }
    /// Several degrees at once.
    pub fn degrees(&self, ds: &[i32]) -> Vec<Pitch> {
        ds.iter().map(|&d| self.degree(d)).collect()
    }
    /// One octave of scale notes starting at the root.
    pub fn notes(&self) -> Vec<Pitch> {
        (0..self.scale.len() as i32)
            .map(|d| self.degree(d))
            .collect()
    }
    /// Is this pitch (any octave) in the key?
    pub fn contains(&self, p: impl Into<Pitch>) -> bool {
        let semis = (p.into().0 - self.root.0).round() as i32;
        self.scale.contains(semis)
    }
    /// Snap a pitch to the nearest scale tone (ties go down).
    pub fn quantize(&self, p: impl Into<Pitch>) -> Pitch {
        let p = p.into();
        let rel = p.0 - self.root.0;
        let oct = (rel / 12.0).floor();
        let mut best = p;
        let mut best_d = f32::MAX;
        for o in [oct - 1.0, oct, oct + 1.0] {
            for &iv in self.scale.intervals() {
                let cand = self.root.0 + o * 12.0 + iv as f32;
                let d = (cand - p.0).abs();
                if d < best_d - 1e-4 || ((d - best_d).abs() < 1e-4 && cand < best.0) {
                    best = Pitch(cand);
                    best_d = d;
                }
            }
        }
        best
    }
    /// The 0-based scale degree index of a pitch, if it is in the key (`Key::major(C4).degree_of(G5) == Some(11)`).
    pub fn degree_of(&self, p: impl Into<Pitch>) -> Option<i32> {
        let p = p.into();
        let n = self.scale.len() as i32;
        let rel = (p.0 - self.root.0).round() as i32;
        let oct = rel.div_euclid(12);
        let pc = rel.rem_euclid(12) as u8;
        self.scale
            .intervals()
            .iter()
            .position(|&i| i == pc)
            .map(|i| oct * n + i as i32)
    }
    /// A diatonic chord of `size` notes stacked in thirds on a 0-based degree.
    pub fn chord_on(&self, degree: i32, size: usize) -> Chord {
        let mut c = Chord::new((0..size as i32).map(|i| self.degree(degree + 2 * i)));
        c = Chord::from_intervals(
            self.degree(degree),
            &c.pitches()
                .iter()
                .map(|p| p - self.degree(degree).0)
                .collect::<Vec<_>>(),
        );
        c
    }
    /// Diatonic triad on a 0-based degree (`Key::major(C4).triad(1)` = Dm).
    pub fn triad(&self, degree: i32) -> Chord {
        self.chord_on(degree, 3)
    }
    /// Diatonic seventh chord on a 0-based degree (`Key::major(C4).seventh(4)` = G7).
    pub fn seventh(&self, degree: i32) -> Chord {
        self.chord_on(degree, 4)
    }
    /// One roman-numeral chord, e.g. `"V7"`, `"ii"`, `"bVII"`, `"viio"`, `"iiø7"`, `"IVmaj7"`, `"V/V"`.
    ///
    /// The numeral picks the scale degree (`b`/`#` prefixes alter it); its case picks the third:
    /// UPPER = major, lower = minor. Suffixes are chord qualities: `7` (dominant on upper, m7 on
    /// lower), `maj7`, `9`, `sus4`, `o`/`°` (dim), `o7` (dim7), `ø`/`ø7` (half-dim), `+` (aug)...
    /// `X/Y` is a secondary chord: numeral X in the key of Y.
    pub fn roman(&self, numeral: &str) -> Result<Chord, TheoryError> {
        let s = numeral.trim();
        if let Some((a, b)) = s.split_once('/')
            && !(a.ends_with('6') && b.starts_with('9'))
        {
            let (root, upper, _) = self.roman_root(b)?;
            let k = Key::new(root, if upper { Scale::Major } else { Scale::Minor });
            return k.roman(a);
        }
        let (root, upper, rest) = self.roman_root(s)?;
        let q = if upper || rest.starts_with(['o', '°', 'ø', 'm']) || rest.starts_with("dim") {
            rest.to_string()
        } else {
            format!("m{rest}")
        };
        Chord::of(root, &q).map_err(|e| TheoryError(format!("{e} in roman numeral {numeral:?}")))
    }
    fn roman_root<'a>(&self, s: &'a str) -> Result<(Pitch, bool, &'a str), TheoryError> {
        let mut acc = 0.0f32;
        let mut i = 0;
        for c in s.chars() {
            match c {
                'b' | '♭' => acc -= 1.0,
                '#' | '♯' => acc += 1.0,
                _ => break,
            }
            i += c.len_utf8();
        }
        let body = &s[i..];
        const NUMS: [(&str, i32); 7] = [
            ("VII", 6),
            ("III", 2),
            ("VI", 5),
            ("IV", 3),
            ("II", 1),
            ("V", 4),
            ("I", 0),
        ];
        for (n, d) in NUMS {
            if let Some(head) = body.get(..n.len()) {
                let upper = head == n;
                if upper || head == n.to_lowercase() {
                    return Ok((self.degree(d).up(acc), upper, &body[n.len()..]));
                }
            }
        }
        err(format!(
            "not a roman numeral: {s:?} (expected I..VII / i..vii, e.g. \"ii7 V7 Imaj7\")"
        ))
    }
    /// A chord progression from roman numerals separated by spaces, `-`, `|` or `,`:
    /// `key.prog("ii7 V7 Imaj7")`. Panics with a helpful message on bad input (see [`Key::try_prog`]).
    #[track_caller]
    pub fn prog(&self, numerals: &str) -> Vec<Chord> {
        self.try_prog(numerals).unwrap_or_else(|e| panic!("{e}"))
    }
    /// Fallible [`Key::prog`].
    pub fn try_prog(&self, numerals: &str) -> Result<Vec<Chord>, TheoryError> {
        numerals
            .split(|c: char| c.is_whitespace() || matches!(c, '-' | '|' | ','))
            .filter(|t| !t.is_empty())
            .map(|t| self.roman(t))
            .collect()
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.root.name(), self.scale.name())
    }
}

impl std::str::FromStr for Key {
    type Err = TheoryError;
    fn from_str(s: &str) -> Result<Key, TheoryError> {
        Key::parse(s)
    }
}

impl From<&str> for Key {
    /// Parses `"C minor"` etc.; **panics** with a helpful message on bad input.
    #[track_caller]
    fn from(s: &str) -> Key {
        Key::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }
}

/// Roman-numeral progression in a key: `prog("ii V I", Key::major(C3))` or `prog("i bVI bIII bVII", "A3 minor")`.
/// Panics with a helpful message on bad input. Turn it into a pattern (one chord per cycle) with
/// `Pattern::from(chords)`, or play it directly: `s.play(&pad, prog("I V vi IV", key))`.
#[track_caller]
pub fn prog(numerals: &str, key: impl Into<Key>) -> Vec<Chord> {
    key.into().prog(numerals)
}

/// Re-voice a progression for smooth voice leading: each chord takes the inversion closest to the
/// previous one.
pub fn voice_lead(chords: Vec<Chord>) -> Vec<Chord> {
    let mut out: Vec<Chord> = Vec::with_capacity(chords.len());
    for c in chords {
        let c = match out.last() {
            Some(prev) => c.near(prev),
            None => c,
        };
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::notes::*;
    use super::*;

    #[test]
    fn degrees_and_quantize() {
        let k = Key::new(C4, Scale::Minor);
        assert_eq!(k.degree(0), C4);
        assert_eq!(k.degree(2), Eb4);
        assert_eq!(k.degree(7), C5);
        assert_eq!(k.degree(9), Eb5);
        assert_eq!(k.degree(-1), Bb3);
        assert_eq!(k.degree(-7), C3);
        assert_eq!(k.quantize(E4), Eb4);
        assert_eq!(k.quantize(Fs4), F4);
        assert_eq!(k.quantize(B3), Bb3);
        assert!(k.contains(G2));
        assert!(!k.contains(A4));
        assert_eq!(k.degree_of(G5), Some(11));
        let k = Key::parse("F# dorian").unwrap();
        assert_eq!(k.root, Fs4);
        assert_eq!(k.scale, Scale::Dorian);
        assert_eq!(Key::parse("Am").unwrap(), Key::minor(A4));
        assert_eq!(Key::parse("Bb3 major").unwrap(), Key::major(Bb3));
        assert_eq!(
            Key::parse("eb harmonic minor").unwrap().scale,
            Scale::HarmonicMinor
        );
        assert!(Key::parse("H minor").is_err());
    }

    #[test]
    fn diatonic_chords() {
        let k = Key::major(C4);
        assert_eq!(k.triad(1).pitches(), vec![62.0, 65.0, 69.0]);
        assert_eq!(k.seventh(4).pitches(), vec![67.0, 71.0, 74.0, 77.0]);
        assert_eq!(k.triad(1).root(), D4);
    }

    #[test]
    fn roman_numerals() {
        let k = Key::major(C4);
        let ch = k.prog("ii V I");
        assert_eq!(ch[0].pitches(), vec![62.0, 65.0, 69.0]);
        assert_eq!(ch[1].pitches(), vec![67.0, 71.0, 74.0]);
        assert_eq!(ch[2].pitches(), vec![60.0, 64.0, 67.0]);
        let ch = k.prog("ii7 V7 Imaj7 viio iiø7 bVII V/V vi");
        assert_eq!(ch[0], D4.min7());
        assert_eq!(ch[1], G4.dom7());
        assert_eq!(ch[2], C4.maj7());
        assert_eq!(ch[3], B4.dim());
        assert_eq!(ch[4], D4.m7b5());
        assert_eq!(ch[5], Bb4.maj());
        assert_eq!(ch[6], D5.maj());
        assert_eq!(ch[7], A4.min());
        let m = prog("i iv V7", "A3 minor");
        assert_eq!(m[2], E4.dom7());
        assert!(k.try_prog("ii X").is_err());
        let vl = voice_lead(k.prog("I IV V I"));
        assert_eq!(vl.len(), 4);
    }
}
