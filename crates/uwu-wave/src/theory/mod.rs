//! Music theory: pitches, note names, scales, keys, chords and roman-numeral progressions.
//!
//! ```
//! use uwu_wave::prelude::*;
//! let key = Key::new(C4, Scale::Minor);          // or Key::parse("C minor"), or "C minor".into()
//! assert_eq!(key.degree(2), Eb4);                // degrees are 0-based: 0 = root
//! assert_eq!(key.degree(-1), Bb3);               // negative / large degrees wrap octaves
//! let ch = A3.min7().inv(1);                     // chord builders on any pitch
//! let ii_v_i = prog("ii7 V7 Imaj7", Key::major(C3));
//! let slash = Chord::parse("Bb/D").unwrap();
//! assert_eq!(C4.up(7), G4);
//! assert_eq!(Cs4, Db4);
//! # let _ = (ch, ii_v_i, slash);
//! ```
//!
//! A [`Pitch`] is a fractional MIDI note number, so microtonal pitches (`C4.up(0.5)`) are fine.

mod chord;
mod key;
pub mod notes;
mod scale;

pub use chord::Chord;
pub use key::{Key, prog, voice_lead};
pub use scale::Scale;

use crate::units::Val;
use std::fmt;
use std::ops::{Add, Sub};

/// Something went wrong parsing a note name, chord symbol, scale, key or roman numeral.
#[derive(Clone, Debug, PartialEq)]
pub struct TheoryError(pub String);

impl fmt::Display for TheoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for TheoryError {}
impl From<TheoryError> for crate::error::Error {
    fn from(e: TheoryError) -> crate::error::Error {
        crate::error::Error::Invalid(e.0)
    }
}

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T, TheoryError> {
    Err(TheoryError(msg.into()))
}

/// A pitch as a (fractional) MIDI note number: `C4` = 60, `A4` = 69 = 440 Hz.
///
/// ```
/// use uwu_wave::prelude::*;
/// assert_eq!(C4.up(4), E4);
/// assert_eq!(A4.oct(-1), A3);
/// assert!((A4.hz() - 440.0).abs() < 1e-3);
/// assert_eq!(Fs3.name(), "F#3");
/// let c = C4.maj7();                       // Chord [C4 E4 G4 B4]
/// let p: Pitch = "Bb2".into();            // parse (panics on bad input; see Pitch::parse)
/// # let _ = (c, p);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct Pitch(pub f32);

const SHARP_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

impl Pitch {
    /// From a MIDI note number.
    pub const fn new(midi: f32) -> Pitch {
        Pitch(midi)
    }
    /// The MIDI note number.
    pub fn midi(self) -> f32 {
        self.0
    }
    /// Up by `st` semitones (fractional allowed).
    pub fn up(self, st: impl Val) -> Pitch {
        Pitch(self.0 + st.val())
    }
    /// Down by `st` semitones.
    pub fn down(self, st: impl Val) -> Pitch {
        Pitch(self.0 - st.val())
    }
    /// Shift by `n` octaves (negative = down).
    pub fn oct(self, n: i32) -> Pitch {
        Pitch(self.0 + 12.0 * n as f32)
    }
    /// Frequency in Hz (A4 = 440).
    pub fn hz(self) -> f32 {
        crate::dsp::midi_to_hz(self.0)
    }
    /// The pitch closest to a frequency.
    pub fn from_hz(hz: f32) -> Pitch {
        Pitch(crate::dsp::hz_to_midi(hz))
    }
    /// Nearest whole semitone.
    pub fn round(self) -> Pitch {
        Pitch(self.0.round())
    }
    /// Pitch class 0..12 (C = 0).
    pub fn pitch_class(self) -> f32 {
        self.0.rem_euclid(12.0)
    }
    /// Octave number in scientific notation (`C4` → 4).
    pub fn octave(self) -> i32 {
        (self.0.round() / 12.0).floor() as i32 - 1
    }
    /// Note name with sharps, e.g. `"C#4"`. Microtonal pitches get a cents suffix (`"C4+25c"`).
    pub fn name(self) -> String {
        self.name_with(&SHARP_NAMES)
    }
    /// Note name with flats, e.g. `"Db4"`.
    pub fn name_flat(self) -> String {
        self.name_with(&FLAT_NAMES)
    }
    fn name_with(self, names: &[&str; 12]) -> String {
        let r = self.0.round();
        let pc = (r as i32).rem_euclid(12) as usize;
        let oct = (r / 12.0).floor() as i32 - 1;
        let cents = ((self.0 - r) * 100.0).round() as i32;
        if cents == 0 {
            format!("{}{}", names[pc], oct)
        } else {
            format!("{}{}{:+}c", names[pc], oct, cents)
        }
    }
    /// Parse a note name (`"C4"`, `"f#2"`, `"Eb3"`, `"cs4"`, `"a"` = A4), a MIDI number (`"60"`)
    /// or a General MIDI drum name (`"bd"`, `"sd"`, `"hh"`...). A missing octave means octave 4.
    pub fn parse(s: &str) -> Result<Pitch, TheoryError> {
        let t = s.trim();
        if let Some(p) = gm_drum(t) {
            return Ok(p);
        }
        if let Ok(v) = t.parse::<f32>() {
            return Ok(Pitch(v));
        }
        match parse_note_prefix(t, true) {
            Some((semi, oct, "")) => Ok(Pitch(midi_of(semi, oct.unwrap_or(4)))),
            _ => err(format!(
                "not a note name: {s:?} (expected e.g. C4, f#2, Eb3, cs4, 60 or a drum name like bd)"
            )),
        }
    }
    /// A chord of the given quality rooted on this pitch, e.g. `C4.chord("m7b5")`.
    /// Panics on an unknown quality — see [`Chord::of`] for the fallible version and the list.
    #[track_caller]
    pub fn chord(self, quality: &str) -> Chord {
        Chord::of(self, quality).unwrap_or_else(|e| panic!("{e}"))
    }
    fn iv(self, iv: &[f32]) -> Chord {
        Chord::from_intervals(self, iv)
    }
    /// Major triad.
    pub fn maj(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0])
    }
    /// Minor triad.
    pub fn min(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0])
    }
    /// Diminished triad.
    pub fn dim(self) -> Chord {
        self.iv(&[0.0, 3.0, 6.0])
    }
    /// Augmented triad.
    pub fn aug(self) -> Chord {
        self.iv(&[0.0, 4.0, 8.0])
    }
    /// Suspended 2nd.
    pub fn sus2(self) -> Chord {
        self.iv(&[0.0, 2.0, 7.0])
    }
    /// Suspended 4th.
    pub fn sus4(self) -> Chord {
        self.iv(&[0.0, 5.0, 7.0])
    }
    /// Power chord (root + fifth).
    pub fn power(self) -> Chord {
        self.iv(&[0.0, 7.0])
    }
    /// Major sixth.
    pub fn six(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 9.0])
    }
    /// Minor sixth.
    pub fn min6(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 9.0])
    }
    /// Major seventh.
    pub fn maj7(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 11.0])
    }
    /// Minor seventh.
    pub fn min7(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 10.0])
    }
    /// Dominant seventh.
    pub fn dom7(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 10.0])
    }
    /// Dominant seventh (alias of [`Pitch::dom7`]).
    pub fn seven(self) -> Chord {
        self.dom7()
    }
    /// Diminished seventh.
    pub fn dim7(self) -> Chord {
        self.iv(&[0.0, 3.0, 6.0, 9.0])
    }
    /// Half-diminished seventh (m7♭5).
    pub fn m7b5(self) -> Chord {
        self.iv(&[0.0, 3.0, 6.0, 10.0])
    }
    /// Minor-major seventh.
    pub fn min_maj7(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 11.0])
    }
    /// Augmented seventh.
    pub fn aug7(self) -> Chord {
        self.iv(&[0.0, 4.0, 8.0, 10.0])
    }
    /// Dominant seventh, suspended 4th.
    pub fn seven_sus4(self) -> Chord {
        self.iv(&[0.0, 5.0, 7.0, 10.0])
    }
    /// Major add 9.
    pub fn add9(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 14.0])
    }
    /// Minor add 9.
    pub fn madd9(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 14.0])
    }
    /// Dominant ninth.
    pub fn nine(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 10.0, 14.0])
    }
    /// Minor ninth.
    pub fn min9(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 10.0, 14.0])
    }
    /// Major ninth.
    pub fn maj9(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 11.0, 14.0])
    }
    /// Dominant eleventh.
    pub fn eleven(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 10.0, 14.0, 17.0])
    }
    /// Minor eleventh.
    pub fn min11(self) -> Chord {
        self.iv(&[0.0, 3.0, 7.0, 10.0, 14.0, 17.0])
    }
    /// Dominant thirteenth.
    pub fn thirteen(self) -> Chord {
        self.iv(&[0.0, 4.0, 7.0, 10.0, 14.0, 21.0])
    }
}

impl fmt::Display for Pitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

macro_rules! pitch_from_num { ($($t:ty),*) => { $(impl From<$t> for Pitch { fn from(v: $t) -> Pitch { Pitch(v as f32) } })* } }
pitch_from_num!(f32, f64, i32, i64, u8, u32, usize);

impl From<&str> for Pitch {
    /// Parses a note/drum name; **panics** with a helpful message on bad input
    /// (use [`Pitch::parse`] to handle errors).
    #[track_caller]
    fn from(s: &str) -> Pitch {
        Pitch::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }
}
impl From<Pitch> for f32 {
    fn from(p: Pitch) -> f32 {
        p.0
    }
}
impl From<Pitch> for crate::clip::f32Pitch {
    fn from(p: Pitch) -> crate::clip::f32Pitch {
        crate::clip::f32Pitch(p.0)
    }
}

impl<T: Val> Add<T> for Pitch {
    type Output = Pitch;
    /// `C4 + 7` = `G4`.
    fn add(self, st: T) -> Pitch {
        self.up(st)
    }
}
impl Sub<Pitch> for Pitch {
    type Output = f32;
    /// Interval in semitones: `G4 - C4` = 7.
    fn sub(self, o: Pitch) -> f32 {
        self.0 - o.0
    }
}
impl Sub<i32> for Pitch {
    type Output = Pitch;
    fn sub(self, st: i32) -> Pitch {
        self.down(st)
    }
}
impl Sub<f32> for Pitch {
    type Output = Pitch;
    fn sub(self, st: f32) -> Pitch {
        self.down(st)
    }
}

/// General MIDI drum note for a short drum name: `bd`/`kick` 36, `rim`/`rs` 37, `sd`/`snare` 38,
/// `cp`/`clap` 39, `lt` 41, `hh`/`ch` 42, `ph` 44, `mt` 45, `oh` 46, `ht` 48, `cr`/`crash` 49,
/// `rd`/`ride` 51, `tb`/`tamb` 54, `cb`/`cowbell` 56, `sh`/`shaker` 70.
pub fn gm_drum(name: &str) -> Option<Pitch> {
    let m = match name {
        "bd" | "kick" => 36,
        "rim" | "rs" => 37,
        "sd" | "snare" => 38,
        "cp" | "clap" => 39,
        "lt" => 41,
        "hh" | "ch" | "hat" => 42,
        "ph" => 44,
        "mt" => 45,
        "oh" => 46,
        "ht" => 48,
        "cr" | "crash" => 49,
        "rd" | "ride" => 51,
        "tb" | "tamb" => 54,
        "cb" | "cowbell" => 56,
        "sh" | "shaker" => 70,
        _ => return None,
    };
    Some(Pitch(m as f32))
}

pub(crate) fn midi_of(semi: i32, oct: i32) -> f32 {
    ((oct + 1) * 12 + semi) as f32
}

/// Parse a leading note name: letter, accidentals (`#`, `♯`, `b`, `♭`, or `s` for sharp when
/// followed by a digit/end/`'`/`/`), then (if `allow_octave`) octave digits.
/// Returns (semitones from C incl. accidentals, octave, rest).
pub(crate) fn parse_note_prefix(s: &str, allow_octave: bool) -> Option<(i32, Option<i32>, &str)> {
    let mut chars = s.chars();
    let c0 = chars.next()?;
    let pc = match c0.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut idx = c0.len_utf8();
    let mut acc = 0i32;
    loop {
        let r = &s[idx..];
        let mut it = r.chars();
        let Some(c) = it.next() else { break };
        let next = it.next();
        match c {
            '#' | '♯' if (0..2).contains(&acc) => acc += 1,
            'b' | '♭' if acc <= 0 && acc > -2 => acc -= 1,
            's' if (0..2).contains(&acc)
                && matches!(next, None | Some('0'..='9') | Some('\'') | Some('/')) =>
            {
                acc += 1
            }
            _ => break,
        }
        idx += c.len_utf8();
    }
    let mut oct = None;
    if allow_octave {
        let r = &s[idx..];
        let n = r.bytes().take_while(|b| b.is_ascii_digit()).count();
        if n > 0 && n <= 2 {
            oct = r[..n].parse().ok();
            idx += n;
        }
    }
    Some((pc + acc, oct, &s[idx..]))
}

pub mod prelude {
    pub use super::notes::*;
    pub use super::{Chord, Key, Pitch, Scale, prog, voice_lead};
}

#[cfg(test)]
mod tests {
    use super::notes::*;
    use super::*;

    #[test]
    fn pitch_basics() {
        assert_eq!(C4.0, 60.0);
        assert_eq!(A4.0, 69.0);
        assert_eq!(C0.0, 12.0);
        assert_eq!(B8.0, 119.0);
        assert_eq!(Cs4, Db4);
        assert_eq!(C4.up(7), G4);
        assert_eq!(C4.down(1), B3);
        assert_eq!(C4.oct(-2), C2);
        assert_eq!(C4 + 4, E4);
        assert_eq!(G4 - C4, 7.0);
        assert!((A4.hz() - 440.0).abs() < 1e-3);
        assert_eq!(Fs3.name(), "F#3");
        assert_eq!(Fs3.name_flat(), "Gb3");
        assert_eq!(C4.up(0.25).name(), "C4+25c");
        assert_eq!(B3.octave(), 3);
        let f: f32 = C4.into();
        assert_eq!(f, 60.0);
    }

    #[test]
    fn pitch_parse() {
        for (s, m) in [
            ("C4", 60.0),
            ("c#4", 61.0),
            ("Db4", 61.0),
            ("cs4", 61.0),
            ("eb3", 51.0),
            ("bb2", 46.0),
            ("B2", 47.0),
            ("a", 69.0),
            ("60", 60.0),
            ("bd", 36.0),
            ("Cb4", 59.0),
            ("F##3", 55.0),
        ] {
            assert_eq!(Pitch::parse(s).unwrap().0, m, "{s}");
        }
        assert!(Pitch::parse("h4").is_err());
        assert!(Pitch::parse("c4x").is_err());
    }

    #[test]
    fn clip_accepts_pitch() {
        let c = crate::clip::Clip::new(4)
            .note(0, 1, C4, 0.8)
            .note(1, 1, 64, 0.8);
        assert_eq!(c.notes[0].pitch, 60.0);
    }
}
