//! [`Orchestra`]: presets for every instrument of the sound library, with stage seats and
//! gain staging.

use super::engine::Articulated;
use super::patch::{instrument_dirs, library_root};
use crate::clip::Artic;

/// How a preset behaves.
#[derive(Clone, Copy)]
struct Def {
    /// Library ids to try, in order.
    ids: &'static [&'static str],
    name: &'static str,
    /// Trim after loudness normalization (dB) — balances a tutti. Measured: sustained
    /// instruments land at ≈ −22 dB RMS (sections) / −24 dB (solos) for a held mf note in the
    /// middle of their range; percussion at ≈ −6…−12 dB peak.
    trim_db: f32,
    pan: f32,
    depth: f32,
    legato: bool,
    oneshot: bool,
    voices: usize,
}

const fn d(
    ids: &'static [&'static str],
    name: &'static str,
    trim_db: f32,
    pan: f32,
    depth: f32,
) -> Def {
    Def {
        ids,
        name,
        trim_db,
        pan,
        depth,
        legato: false,
        oneshot: false,
        voices: 24,
    }
}
const fn leg(mut x: Def) -> Def {
    x.legato = true;
    x
}
const fn shot(mut x: Def) -> Def {
    x.oneshot = true;
    x
}

/// Every preset: (method name, definition). Seats follow a classic American seating:
/// violins I far left, violins II left, violas centre-right, cellos right, basses far right
/// behind; woodwinds centre, brass behind, percussion at the back.
const PRESETS: &[(&str, Def)] = &[
    (
        "horn_solo",
        leg(d(&["brass/horn"], "horn solo", -1.7, -0.3, 0.68)),
    ),
    (
        "trombone_solo",
        leg(d(&["brass/trombone"], "trombone solo", -2.2, 0.3, 0.72)),
    ),
    (
        "tom_toms",
        shot(d(&["percussion/tom-toms"], "tom-toms", 9.9, -0.1, 0.92)),
    ),
    (
        "woodblock",
        shot(d(&["percussion/woodblock"], "woodblock", 2.5, 0.2, 0.9)),
    ),
    (
        "castanets",
        shot(d(&["percussion/castanets"], "castanets", 0.4, 0.2, 0.9)),
    ),
    (
        "sleigh_bells",
        shot(d(
            &["percussion/sleigh-bells"],
            "sleigh bells",
            1.6,
            0.25,
            0.9,
        )),
    ),
    (
        "wind_chimes",
        shot(d(&["percussion/wind-chimes"], "wind chimes", 1.2, 0.3, 0.9)),
    ),
    (
        "gong",
        shot(d(&["percussion/thai-gong"], "gong", 3.0, 0.3, 0.95)),
    ),
    (
        "celtic_harp",
        shot(d(&["plucked/celtic-harp"], "celtic harp", 8.0, -0.5, 0.5)),
    ),
    (
        "guitar",
        shot(d(&["plucked/guitar"], "guitar", 9.5, -0.3, 0.4)),
    ),
    (
        "mandolin",
        shot(d(&["plucked/mandolin"], "mandolin", 1.2, 0.3, 0.4)),
    ),
    ("banjo", shot(d(&["plucked/banjo"], "banjo", 2.8, 0.2, 0.4))),
    (
        "violins_1",
        leg(d(&["strings/violins-1"], "violins 1", 5.8, -0.6, 0.25)),
    ),
    (
        "violins_2",
        leg(d(
            &["strings/violins-2", "strings/violins-1"],
            "violins 2",
            5.4,
            -0.3,
            0.3,
        )),
    ),
    (
        "violas",
        leg(d(&["strings/violas"], "violas", 4.7, 0.2, 0.3)),
    ),
    (
        "cellos",
        leg(d(&["strings/cellos"], "cellos", 4.7, 0.45, 0.3)),
    ),
    (
        "basses",
        leg(d(&["strings/basses"], "basses", 8.0, 0.65, 0.45)),
    ),
    (
        "strings",
        d(&["strings/strings-ensemble"], "strings", 4.8, 0.0, 0.3),
    ),
    (
        "violin_solo",
        leg(d(
            &["strings/violin-solo", "strings/strings-solo"],
            "violin solo",
            0.0,
            -0.35,
            0.15,
        )),
    ),
    (
        "viola_solo",
        leg(d(
            &["strings/viola-solo", "strings/strings-solo"],
            "viola solo",
            0.2,
            0.1,
            0.15,
        )),
    ),
    (
        "cello_solo",
        leg(d(
            &["strings/cello-solo", "strings/strings-solo"],
            "cello solo",
            -1.9,
            0.3,
            0.15,
        )),
    ),
    (
        "bass_solo",
        leg(d(
            &["strings/bass-solo", "strings/strings-solo"],
            "bass solo",
            1.3,
            0.5,
            0.2,
        )),
    ),
    (
        "piccolo",
        leg(d(&["woodwinds/piccolo"], "piccolo", 3.2, -0.1, 0.5)),
    ),
    (
        "flute",
        leg(d(&["woodwinds/flute"], "flute", 4.3, -0.15, 0.5)),
    ),
    ("oboe", leg(d(&["woodwinds/oboe"], "oboe", -8.9, 0.1, 0.5))),
    (
        "cor_anglais",
        leg(d(
            &["woodwinds/cor-anglais"],
            "cor anglais",
            -3.5,
            0.15,
            0.52,
        )),
    ),
    (
        "clarinet",
        leg(d(&["woodwinds/clarinet"], "clarinet", -3.3, -0.1, 0.55)),
    ),
    (
        "bass_clarinet",
        leg(d(
            &["woodwinds/bass-clarinet"],
            "bass clarinet",
            -5.2,
            -0.2,
            0.57,
        )),
    ),
    (
        "bassoon",
        leg(d(&["woodwinds/bassoon"], "bassoon", -7.1, 0.15, 0.57)),
    ),
    (
        "contrabassoon",
        leg(d(
            &["woodwinds/contrabassoon"],
            "contrabassoon",
            -5.6,
            0.25,
            0.6,
        )),
    ),
    (
        "saxophone",
        leg(d(&["woodwinds/saxophone"], "saxophone", 2.4, 0.0, 0.5)),
    ),
    (
        "woodwinds",
        d(
            &["woodwinds/woodwinds-ensemble"],
            "woodwinds",
            8.4,
            0.0,
            0.5,
        ),
    ),
    (
        "horns",
        leg(d(&["brass/horns", "brass/horn"], "horns", 3.8, -0.35, 0.7)),
    ),
    (
        "trumpet",
        leg(d(
            &["brass/trumpet", "brass/trumpets"],
            "trumpet",
            3.9,
            0.1,
            0.75,
        )),
    ),
    (
        "trombones",
        leg(d(
            &["brass/trombones", "brass/trombone"],
            "trombones",
            4.2,
            0.3,
            0.75,
        )),
    ),
    ("tuba", leg(d(&["brass/tuba"], "tuba", 0.4, 0.45, 0.78))),
    (
        "brass",
        d(&["brass/brass-ensemble"], "brass", 2.7, 0.1, 0.72),
    ),
    (
        "timpani",
        shot(d(&["percussion/timpani"], "timpani", 10.1, -0.2, 0.9)),
    ),
    (
        "glockenspiel",
        shot(d(
            &["percussion/glockenspiel"],
            "glockenspiel",
            3.2,
            0.25,
            0.85,
        )),
    ),
    (
        "xylophone",
        shot(d(&["percussion/xylophone"], "xylophone", 10.4, 0.2, 0.85)),
    ),
    (
        "vibraphone",
        shot(d(&["percussion/vibraphone"], "vibraphone", 7.2, 0.3, 0.85)),
    ),
    (
        "celesta",
        shot(d(
            &["percussion/celesta", "keys/celesta"],
            "celesta",
            4.5,
            -0.4,
            0.6,
        )),
    ),
    (
        "tubular_bells",
        shot(d(
            &["percussion/tubular-bells"],
            "tubular bells",
            0.3,
            0.35,
            0.9,
        )),
    ),
    (
        "bass_drum",
        shot(d(&["percussion/bass-drum"], "bass drum", 3.9, 0.1, 0.92)),
    ),
    (
        "snare_drum",
        shot(d(&["percussion/snare-drum"], "snare drum", 0.3, 0.15, 0.9)),
    ),
    (
        "cymbals",
        shot(d(
            &["percussion/clash-cymbals", "percussion/cymbals"],
            "cymbals",
            2.9,
            0.2,
            0.92,
        )),
    ),
    (
        "suspended_cymbal",
        shot(d(
            &["percussion/suspended-cymbal"],
            "suspended cymbal",
            3.0,
            0.25,
            0.92,
        )),
    ),
    (
        "tam_tam",
        shot(d(&["percussion/tam-tam"], "tam-tam", 6.9, 0.3, 0.95)),
    ),
    (
        "triangle",
        shot(d(&["percussion/triangle"], "triangle", 1.7, 0.3, 0.9)),
    ),
    (
        "tambourine",
        shot(d(&["percussion/tambourine"], "tambourine", -2.6, 0.25, 0.9)),
    ),
    ("harp", shot(d(&["plucked/harp"], "harp", 8.1, -0.5, 0.5))),
    ("piano", shot(d(&["keys/piano"], "piano", 8.0, -0.2, 0.4))),
    ("choir", d(&["choir/mixed"], "choir", 6.7, 0.0, 0.95)),
    (
        "choir_female",
        d(
            &["choir/female", "choir/mixed"],
            "choir (female)",
            5.7,
            -0.2,
            0.95,
        ),
    ),
    (
        "choir_male",
        d(
            &["choir/male", "choir/mixed"],
            "choir (male)",
            8.1,
            0.2,
            0.95,
        ),
    ),
];

/// Every preset name (method names of [`Orchestra`]) whose samples are present.
pub fn available() -> Vec<String> {
    PRESETS
        .iter()
        .filter(|(_, d)| d.ids.iter().any(|id| !instrument_dirs(id).is_empty()))
        .map(|(n, _)| n.to_string())
        .collect()
}

fn build(def: &Def) -> Result<Articulated, String> {
    let mut last = String::new();
    for id in def.ids {
        match Articulated::load(id) {
            Ok(a) => {
                let mut a = a
                    .named(def.name)
                    .legato(def.legato)
                    .one_shot(def.oneshot)
                    .auto_short(def.legato)
                    .voices(def.voices)
                    .gain(crate::units::db_to_gain(def.trim_db))
                    .place(def.pan, def.depth);
                if def.oneshot {
                    a = a.default_artic(Artic::Normal);
                }
                return Ok(a);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn preset(name: &str) -> Articulated {
    let def = PRESETS
        .iter()
        .find(|p| p.0 == name)
        .map(|p| p.1)
        .expect("unknown preset");
    build(&def).unwrap_or_else(|e| {
        panic!(
            "uwu-wave: Orchestra::{name}() needs {:?} in the sound library ({}).\n{e}\nAvailable presets: {}",
            def.ids,
            library_root().display(),
            available().join(", ")
        )
    })
}

/// The orchestra: one constructor per library instrument, each an [`Articulated`] with sensible
/// defaults (legato engine for sections and solo winds/brass, one-shots for percussion,
/// loudness-normalized and trimmed so a tutti is balanced) and a stage seat
/// ([`Articulated::seat`] → (pan, depth)).
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let song = Song::new("overture").bpm(92);
/// let vln = Orchestra::violins_1();
/// let (pan, _depth) = vln.seat();
/// let v1 = song.track("vln1", vln).pan(pan).dynamics(Dyn::MF);
/// let hn = song.track("horns", Orchestra::horns()).dynamics(auto![0 => Dyn::P, bar(8) => Dyn::FF]);
/// let tp = song.track("timp", Orchestra::timpani());
/// ```
///
/// Constructors panic with a helpful message when the samples are missing; use
/// [`Orchestra::get`] for a fallible lookup by name.
pub struct Orchestra;

macro_rules! presets {
    ($($(#[$m:meta])* $f:ident),* $(,)?) => {
        impl Orchestra {
            $( $(#[$m])* pub fn $f() -> Articulated { preset(stringify!($f)) } )*
        }
    };
}

presets! {
    /// Solo French horn (Philharmonia: pp..ff layers, staccato, muted, trills).
    horn_solo,
    /// Solo trombone (Philharmonia: pp..ff layers).
    trombone_solo,
    tom_toms,
    woodblock,
    castanets,
    sleigh_bells,
    wind_chimes,
    /// Thai gong.
    gong,
    celtic_harp,
    guitar,
    mandolin,
    banjo,
    /// 1st violins section (sustain, staccato, pizzicato, tremolo, accent…).
    violins_1,
    /// 2nd violins section.
    violins_2,
    violas,
    cellos,
    /// Double basses section.
    basses,
    /// Full string ensemble (one patch, chords).
    strings,
    violin_solo,
    viola_solo,
    cello_solo,
    bass_solo,
    piccolo,
    flute,
    oboe,
    cor_anglais,
    clarinet,
    bass_clarinet,
    bassoon,
    contrabassoon,
    saxophone,
    /// Woodwind ensemble.
    woodwinds,
    /// French horn section.
    horns,
    trumpet,
    trombones,
    tuba,
    /// Brass ensemble.
    brass,
    timpani,
    glockenspiel,
    xylophone,
    vibraphone,
    celesta,
    tubular_bells,
    bass_drum,
    snare_drum,
    /// Clash cymbals.
    cymbals,
    suspended_cymbal,
    tam_tam,
    triangle,
    tambourine,
    harp,
    piano,
    /// Mixed choir ("aah").
    choir,
    choir_female,
    choir_male,
}

impl Orchestra {
    /// Fallible lookup by preset name (`"violins_1"`, `"horns"` …) or library id
    /// (`"woodwinds/flute"`).
    pub fn get(name: &str) -> Result<Articulated, String> {
        if let Some((_, def)) = PRESETS.iter().find(|p| p.0 == name) {
            return build(def);
        }
        Articulated::load(name)
    }
    /// Names of every preset whose samples are installed.
    pub fn available() -> Vec<String> {
        available()
    }
    /// Stage seat (pan, depth) of a preset without loading samples.
    pub fn seat_of(name: &str) -> Option<(f32, f32)> {
        PRESETS
            .iter()
            .find(|p| p.0 == name)
            .map(|p| (p.1.pan, p.1.depth))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Ctx;
    use crate::dsp::Frame;
    use crate::instrument::{Instrument, NoteOn};

    #[test]
    fn seats_are_sane() {
        for (n, d) in PRESETS {
            assert!(
                (-1.0..=1.0).contains(&d.pan) && (0.0..=1.0).contains(&d.depth),
                "{n}"
            );
        }
        assert!(
            Orchestra::seat_of("violins_1").unwrap().0 < Orchestra::seat_of("cellos").unwrap().0
        );
        assert!(Orchestra::get("nope").is_err());
    }

    /// Smoke-test every installed preset: renders finite audio at a sane level.
    #[test]
    fn installed_presets_render() {
        let names = available();
        let sr = 48_000.0;
        for n in names {
            let mut a = match Orchestra::get(&n) {
                Ok(a) => a,
                Err(_) => continue,
            };
            a.prepare(sr);
            let ctx = Ctx::offline(sr);
            let (lo, hi) = a.range();
            let mid = ((lo + hi) * 0.5).round();
            a.note_on(&NoteOn::new(1, mid, 0.65, 0.8), &ctx);
            let mut out = vec![Frame::ZERO; 48_000];
            for c in out.chunks_mut(64) {
                a.render(c, &ctx);
            }
            let pk = out.iter().fold(0.0f32, |m, f| m.max(f.peak()));
            assert!(
                out.iter().all(|f| f.l.is_finite() && f.r.is_finite()),
                "{n}"
            );
            assert!(pk < 4.0, "{n}: peak {pk}");
            let rms = (out.iter().map(|f| f.l * f.l + f.r * f.r).sum::<f32>()
                / (2.0 * out.len() as f32))
                .sqrt();
            println!(
                "{n:<18} key {mid:>3} peak {:>6.1} dB  rms {:>6.1} dB",
                20.0 * pk.max(1e-9).log10(),
                20.0 * rms.max(1e-9).log10()
            );
        }
    }
}
