//! Scales: interval sets relative to a root.

use super::{TheoryError, err};

/// A scale as a set of semitone offsets within one octave (always starting at 0).
///
/// Use the associated constants like enum variants (`Scale::Minor`, `Scale::Dorian`,
/// `Scale::MinorPentatonic`...), [`Scale::parse`] (`"harmonic minor"`), or [`Scale::custom`].
///
/// ```
/// use uwu_wave::prelude::*;
/// assert_eq!(Scale::Minor.intervals(), &[0, 2, 3, 5, 7, 8, 10]);
/// assert_eq!(Scale::parse("F# dorian".split_once(' ').unwrap().1).unwrap(), Scale::Dorian);
/// assert_eq!(Scale::Major.offset(7), 12);  // degree 7 = the octave
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    name: &'static str,
    steps: [u8; 12],
    len: u8,
}

impl PartialEq for Scale {
    fn eq(&self, o: &Scale) -> bool {
        self.intervals() == o.intervals()
    }
}

const fn def(name: &'static str, s: &[u8]) -> Scale {
    let mut steps = [0u8; 12];
    let mut i = 0;
    while i < s.len() && i < 12 {
        steps[i] = s[i];
        i += 1;
    }
    Scale {
        name,
        steps,
        len: i as u8,
    }
}

#[allow(non_upper_case_globals)]
impl Scale {
    pub const Major: Scale = def("major", &[0, 2, 4, 5, 7, 9, 11]);
    pub const Ionian: Scale = def("ionian", &[0, 2, 4, 5, 7, 9, 11]);
    pub const Dorian: Scale = def("dorian", &[0, 2, 3, 5, 7, 9, 10]);
    pub const Phrygian: Scale = def("phrygian", &[0, 1, 3, 5, 7, 8, 10]);
    pub const Lydian: Scale = def("lydian", &[0, 2, 4, 6, 7, 9, 11]);
    pub const Mixolydian: Scale = def("mixolydian", &[0, 2, 4, 5, 7, 9, 10]);
    /// Natural minor.
    pub const Minor: Scale = def("minor", &[0, 2, 3, 5, 7, 8, 10]);
    pub const Aeolian: Scale = def("aeolian", &[0, 2, 3, 5, 7, 8, 10]);
    pub const Locrian: Scale = def("locrian", &[0, 1, 3, 5, 6, 8, 10]);
    pub const HarmonicMinor: Scale = def("harmonic minor", &[0, 2, 3, 5, 7, 8, 11]);
    /// Melodic minor (ascending / jazz minor).
    pub const MelodicMinor: Scale = def("melodic minor", &[0, 2, 3, 5, 7, 9, 11]);
    pub const HarmonicMajor: Scale = def("harmonic major", &[0, 2, 4, 5, 7, 8, 11]);
    pub const MajorPentatonic: Scale = def("major pentatonic", &[0, 2, 4, 7, 9]);
    pub const MinorPentatonic: Scale = def("minor pentatonic", &[0, 3, 5, 7, 10]);
    /// Minor blues (hexatonic: minor pentatonic + ♭5).
    pub const Blues: Scale = def("blues", &[0, 3, 5, 6, 7, 10]);
    pub const MajorBlues: Scale = def("major blues", &[0, 2, 3, 4, 7, 9]);
    pub const WholeTone: Scale = def("whole tone", &[0, 2, 4, 6, 8, 10]);
    pub const Chromatic: Scale = def("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    /// 5th mode of harmonic minor (Spanish / Freygish).
    pub const PhrygianDominant: Scale = def("phrygian dominant", &[0, 1, 4, 5, 7, 8, 10]);
    pub const LydianDominant: Scale = def("lydian dominant", &[0, 2, 4, 6, 7, 9, 10]);
    /// Super-locrian (7th mode of melodic minor).
    pub const Altered: Scale = def("altered", &[0, 1, 3, 4, 6, 8, 10]);
    pub const DoubleHarmonic: Scale = def("double harmonic", &[0, 1, 4, 5, 7, 8, 11]);
    pub const HungarianMinor: Scale = def("hungarian minor", &[0, 2, 3, 6, 7, 8, 11]);
    /// Half-whole diminished (octatonic).
    pub const Diminished: Scale = def("diminished", &[0, 1, 3, 4, 6, 7, 9, 10]);
    /// Whole-half diminished.
    pub const WholeHalf: Scale = def("whole half", &[0, 2, 3, 5, 6, 8, 9, 11]);
    pub const Hirajoshi: Scale = def("hirajoshi", &[0, 2, 3, 7, 8]);
    pub const InSen: Scale = def("in sen", &[0, 1, 5, 7, 10]);
    pub const Egyptian: Scale = def("egyptian", &[0, 2, 5, 7, 10]);
    pub const BebopDominant: Scale = def("bebop dominant", &[0, 2, 4, 5, 7, 9, 10, 11]);
    pub const BebopMajor: Scale = def("bebop major", &[0, 2, 4, 5, 7, 8, 9, 11]);

    /// All built-in scales.
    pub const ALL: &'static [Scale] = &[
        Scale::Major,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::Minor,
        Scale::Locrian,
        Scale::HarmonicMinor,
        Scale::MelodicMinor,
        Scale::HarmonicMajor,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
        Scale::MajorBlues,
        Scale::WholeTone,
        Scale::Chromatic,
        Scale::PhrygianDominant,
        Scale::LydianDominant,
        Scale::Altered,
        Scale::DoubleHarmonic,
        Scale::HungarianMinor,
        Scale::Diminished,
        Scale::WholeHalf,
        Scale::Hirajoshi,
        Scale::InSen,
        Scale::Egyptian,
        Scale::BebopDominant,
        Scale::BebopMajor,
    ];

    /// A custom scale from semitone offsets (sorted, deduplicated, reduced into one octave; 0 is added).
    pub fn custom(intervals: &[u8]) -> Scale {
        let mut v: Vec<u8> = intervals.iter().map(|i| i % 12).collect();
        v.push(0);
        v.sort_unstable();
        v.dedup();
        def("custom", &v)
    }
    /// Scale name (lowercase, e.g. `"harmonic minor"`).
    pub fn name(&self) -> &'static str {
        self.name
    }
    /// Number of notes per octave.
    pub fn len(&self) -> usize {
        self.len as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Semitone offsets within one octave.
    pub fn intervals(&self) -> &[u8] {
        &self.steps[..self.len as usize]
    }
    /// Semitones above the root of a 0-based scale degree; wraps octaves in both directions
    /// (`Major.offset(7) == 12`, `Major.offset(-1) == -1`).
    pub fn offset(&self, degree: i32) -> i32 {
        let n = self.len.max(1) as i32;
        let oct = degree.div_euclid(n);
        let i = degree.rem_euclid(n) as usize;
        oct * 12 + self.steps[i] as i32
    }
    /// Does the scale contain this pitch class (semitones above the root, any octave)?
    pub fn contains(&self, semis: i32) -> bool {
        let pc = semis.rem_euclid(12) as u8;
        self.intervals().contains(&pc)
    }
    /// The `n`-th mode (rotation) of this scale, e.g. `Scale::Major.mode(1)` = Dorian.
    pub fn mode(&self, n: i32) -> Scale {
        let len = self.len() as i32;
        if len == 0 {
            return *self;
        }
        let base = self.offset(n);
        let v: Vec<u8> = (0..len)
            .map(|i| (self.offset(n + i) - base) as u8)
            .collect();
        let mut s = def("custom", &v);
        if let Some(k) = Scale::ALL.iter().find(|k| **k == s) {
            s.name = k.name;
        }
        s
    }
    /// Parse a scale name: `major`/`maj`/`ionian`, `minor`/`min`/`m`/`aeolian`, `dorian`, `phrygian`,
    /// `lydian`, `mixolydian`, `locrian`, `harmonic minor`, `melodic minor`, `harmonic major`,
    /// `pentatonic`/`major pentatonic`, `minor pentatonic`, `blues`, `major blues`, `whole tone`,
    /// `chromatic`, `phrygian dominant`, `lydian dominant`, `altered`, `double harmonic`,
    /// `hungarian minor`, `diminished`, `whole half`, `hirajoshi`, `in sen`, `egyptian`,
    /// `bebop dominant`, `bebop major`. Case, spaces, `-` and `_` are ignored.
    pub fn parse(s: &str) -> Result<Scale, TheoryError> {
        let k: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .collect::<String>()
            .to_lowercase();
        let sc = match k.as_str() {
            "" | "major" | "maj" | "ionian" => Scale::Major,
            "minor" | "min" | "m" | "aeolian" | "naturalminor" => Scale::Minor,
            "dorian" => Scale::Dorian,
            "phrygian" => Scale::Phrygian,
            "lydian" => Scale::Lydian,
            "mixolydian" | "mixo" => Scale::Mixolydian,
            "locrian" => Scale::Locrian,
            "harmonicminor" | "harmonic" => Scale::HarmonicMinor,
            "melodicminor" | "melodic" | "jazzminor" => Scale::MelodicMinor,
            "harmonicmajor" => Scale::HarmonicMajor,
            "pentatonic" | "majorpentatonic" | "majpent" | "pent" => Scale::MajorPentatonic,
            "minorpentatonic" | "minpent" | "mpent" => Scale::MinorPentatonic,
            "blues" | "minorblues" => Scale::Blues,
            "majorblues" => Scale::MajorBlues,
            "wholetone" | "whole" => Scale::WholeTone,
            "chromatic" => Scale::Chromatic,
            "phrygiandominant" | "spanish" | "freygish" => Scale::PhrygianDominant,
            "lydiandominant" | "overtone" | "acoustic" => Scale::LydianDominant,
            "altered" | "superlocrian" => Scale::Altered,
            "doubleharmonic" | "byzantine" => Scale::DoubleHarmonic,
            "hungarianminor" | "gypsy" => Scale::HungarianMinor,
            "diminished" | "halfwhole" | "octatonic" => Scale::Diminished,
            "wholehalf" => Scale::WholeHalf,
            "hirajoshi" => Scale::Hirajoshi,
            "insen" => Scale::InSen,
            "egyptian" => Scale::Egyptian,
            "bebopdominant" | "bebop" => Scale::BebopDominant,
            "bebopmajor" => Scale::BebopMajor,
            _ => {
                return err(format!(
                    "unknown scale {s:?} (try major, minor, dorian, harmonic minor, minor pentatonic, blues...)"
                ));
            }
        };
        Ok(sc)
    }
}

impl std::str::FromStr for Scale {
    type Err = TheoryError;
    fn from_str(s: &str) -> Result<Scale, TheoryError> {
        Scale::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offsets_and_modes() {
        assert_eq!(Scale::Major.offset(0), 0);
        assert_eq!(Scale::Major.offset(2), 4);
        assert_eq!(Scale::Major.offset(7), 12);
        assert_eq!(Scale::Major.offset(9), 16);
        assert_eq!(Scale::Major.offset(-1), -1);
        assert_eq!(Scale::Major.offset(-7), -12);
        assert_eq!(Scale::MinorPentatonic.offset(5), 12);
        assert_eq!(Scale::Major.mode(1), Scale::Dorian);
        assert_eq!(Scale::Major.mode(5).name(), "minor");
        assert_eq!(
            Scale::parse("Harmonic Minor").unwrap(),
            Scale::HarmonicMinor
        );
        assert!(Scale::parse("klingon").is_err());
        assert_eq!(Scale::custom(&[7, 4, 12]).intervals(), &[0, 4, 7]);
    }
}
