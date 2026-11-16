//! [`Orchestra`]: presets for every instrument of the sound library, with stage seats,
//! graceful fallbacks and automatic gain staging.
//!
//! Each preset lists library candidates, best first. The first one usually lives in an
//! *import* (e.g. the Philharmonia Orchestra samples, installed into the git-ignored
//! `assets/local` overlay by `uwu library import philharmonia`); the later ones are in the
//! redistributable base library (`assets/library`: VPO sections, piano, cluesurf drums). A
//! preset therefore always plays, and silently upgrades when the import is installed. When a
//! fallback is used a single line is printed once per process.
//!
//! Levels are calibrated when a preset loads: a held *mf* note in the middle of the
//! instrument's range is rendered and its gain adjusted to the preset's target (RMS for
//! sustained instruments, peak for one-shots), so every candidate patch lands at the same level.

use super::engine::Articulated;
use super::patch::{instrument_location, library_root};
use crate::clip::Artic;
use crate::ctx::Ctx;
use crate::dsp::Frame;
use crate::instrument::{Instrument, NoteOn};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// How a preset behaves.
#[derive(Clone, Copy)]
struct Def {
    /// Library instrument ids to try, best first.
    ids: &'static [&'static str],
    name: &'static str,
    /// Calibration target in dB: RMS of a held mf note (sustained instruments) or peak of a
    /// mf hit (one-shots). Sections ≈ −22, solos/winds ≈ −24, ensembles ≈ −21.
    level: f32,
    pan: f32,
    depth: f32,
    legato: bool,
    oneshot: bool,
    /// The first candidate comes from an import (print a hint when falling back).
    import: bool,
}

const fn d(
    ids: &'static [&'static str],
    name: &'static str,
    level: f32,
    pan: f32,
    depth: f32,
) -> Def {
    Def {
        ids,
        name,
        level,
        pan,
        depth,
        legato: false,
        oneshot: false,
        import: false,
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
/// First candidate needs the Philharmonia import.
const fn phil(mut x: Def) -> Def {
    x.import = true;
    x
}

/// Every preset: (method name, definition). Seats follow a classic American seating:
/// violins I far left, violins II left, violas centre-right, cellos right, basses far right
/// behind; woodwinds centre, brass behind, percussion at the back.
const PRESETS: &[(&str, Def)] = &[
    // ---- strings (VPO sections)
    (
        "violins_1",
        leg(d(&["strings/violins-1"], "violins 1", -22.0, -0.6, 0.25)),
    ),
    (
        "violins_2",
        leg(d(
            &["strings/violins-2", "strings/violins-1"],
            "violins 2",
            -22.5,
            -0.3,
            0.3,
        )),
    ),
    (
        "violas",
        leg(d(&["strings/violas"], "violas", -22.5, 0.2, 0.3)),
    ),
    (
        "cellos",
        leg(d(&["strings/cellos"], "cellos", -22.0, 0.45, 0.3)),
    ),
    (
        "basses",
        leg(d(&["strings/basses"], "basses", -22.5, 0.65, 0.45)),
    ),
    (
        "strings",
        d(&["strings/strings-ensemble"], "strings", -21.0, 0.0, 0.3),
    ),
    // ---- solo strings: Philharmonia (full articulations, pp..ff) → VPO
    (
        "violin_solo",
        phil(leg(d(
            &["strings/violin", "strings/violin-solo", "strings/violins-1"],
            "violin solo",
            -24.0,
            -0.35,
            0.15,
        ))),
    ),
    (
        "viola_solo",
        phil(leg(d(
            &["strings/viola", "strings/viola-solo", "strings/violas"],
            "viola solo",
            -24.0,
            0.1,
            0.15,
        ))),
    ),
    (
        "cello_solo",
        phil(leg(d(
            &["strings/cello-solo", "strings/cellos"],
            "cello solo",
            -24.0,
            0.3,
            0.15,
        ))),
    ),
    (
        "bass_solo",
        phil(leg(d(
            &["strings/bass-solo", "strings/basses"],
            "bass solo",
            -24.0,
            0.5,
            0.2,
        ))),
    ),
    // ---- woodwinds: Philharmonia solo → VPO section split → VPO ensemble
    (
        "piccolo",
        leg(d(&["woodwinds/piccolo"], "piccolo", -25.0, -0.1, 0.5)),
    ),
    (
        "flute",
        phil(leg(d(
            &[
                "woodwinds/flute",
                "woodwinds/flutes",
                "woodwinds/woodwinds-ensemble",
            ],
            "flute",
            -24.0,
            -0.15,
            0.5,
        ))),
    ),
    (
        "oboe",
        phil(leg(d(
            &[
                "woodwinds/oboe",
                "woodwinds/oboes",
                "woodwinds/woodwinds-ensemble",
            ],
            "oboe",
            -24.0,
            0.1,
            0.5,
        ))),
    ),
    (
        "cor_anglais",
        phil(leg(d(
            &[
                "woodwinds/cor-anglais",
                "woodwinds/oboes",
                "woodwinds/woodwinds-ensemble",
            ],
            "cor anglais",
            -24.0,
            0.15,
            0.52,
        ))),
    ),
    (
        "clarinet",
        phil(leg(d(
            &[
                "woodwinds/clarinet",
                "woodwinds/clarinets",
                "woodwinds/woodwinds-ensemble",
            ],
            "clarinet",
            -24.0,
            -0.1,
            0.55,
        ))),
    ),
    (
        "bass_clarinet",
        phil(leg(d(
            &[
                "woodwinds/bass-clarinet",
                "woodwinds/clarinets",
                "woodwinds/woodwinds-ensemble",
            ],
            "bass clarinet",
            -24.0,
            -0.2,
            0.57,
        ))),
    ),
    (
        "bassoon",
        phil(leg(d(
            &[
                "woodwinds/bassoon",
                "woodwinds/bassoons",
                "woodwinds/woodwinds-ensemble",
            ],
            "bassoon",
            -24.0,
            0.15,
            0.57,
        ))),
    ),
    (
        "contrabassoon",
        phil(leg(d(
            &[
                "woodwinds/contrabassoon",
                "woodwinds/bassoons",
                "woodwinds/woodwinds-ensemble",
            ],
            "contrabassoon",
            -24.0,
            0.25,
            0.6,
        ))),
    ),
    (
        "saxophone",
        phil(leg(d(
            &[
                "woodwinds/saxophone",
                "woodwinds/clarinets",
                "woodwinds/woodwinds-ensemble",
            ],
            "saxophone",
            -24.0,
            0.0,
            0.5,
        ))),
    ),
    (
        "flutes",
        leg(d(
            &[
                "woodwinds/flutes",
                "woodwinds/flute",
                "woodwinds/woodwinds-ensemble",
            ],
            "flutes",
            -22.5,
            -0.15,
            0.5,
        )),
    ),
    (
        "oboes",
        leg(d(
            &[
                "woodwinds/oboes",
                "woodwinds/oboe",
                "woodwinds/woodwinds-ensemble",
            ],
            "oboes",
            -22.5,
            0.1,
            0.5,
        )),
    ),
    (
        "clarinets",
        leg(d(
            &[
                "woodwinds/clarinets",
                "woodwinds/clarinet",
                "woodwinds/woodwinds-ensemble",
            ],
            "clarinets",
            -22.5,
            -0.1,
            0.55,
        )),
    ),
    (
        "bassoons",
        leg(d(
            &[
                "woodwinds/bassoons",
                "woodwinds/bassoon",
                "woodwinds/woodwinds-ensemble",
            ],
            "bassoons",
            -22.5,
            0.15,
            0.57,
        )),
    ),
    (
        "woodwinds",
        d(
            &["woodwinds/woodwinds-ensemble"],
            "woodwinds",
            -21.0,
            0.0,
            0.5,
        ),
    ),
    // ---- brass (brass/trumpet + brass/tuba: the Philharmonia import shadows VPO)
    (
        "horns",
        leg(d(
            &["brass/horns", "brass/horn"],
            "horns",
            -21.0,
            -0.35,
            0.7,
        )),
    ),
    (
        "horn_solo",
        phil(leg(d(
            &["brass/horn", "brass/horns"],
            "horn solo",
            -24.0,
            -0.3,
            0.68,
        ))),
    ),
    (
        "trumpet",
        leg(d(
            &["brass/trumpet", "brass/brass-ensemble"],
            "trumpet",
            -22.0,
            0.1,
            0.75,
        )),
    ),
    (
        "trombones",
        leg(d(
            &["brass/trombones", "brass/trombone"],
            "trombones",
            -21.5,
            0.3,
            0.75,
        )),
    ),
    (
        "trombone_solo",
        phil(leg(d(
            &["brass/trombone", "brass/trombones"],
            "trombone solo",
            -24.0,
            0.3,
            0.72,
        ))),
    ),
    (
        "tuba",
        leg(d(
            &["brass/tuba", "brass/brass-ensemble"],
            "tuba",
            -22.0,
            0.45,
            0.78,
        )),
    ),
    (
        "brass",
        d(&["brass/brass-ensemble"], "brass", -21.0, 0.1, 0.72),
    ),
    // ---- pitched percussion (VPO)
    (
        "timpani",
        shot(d(&["percussion/timpani"], "timpani", -6.0, -0.2, 0.9)),
    ),
    (
        "glockenspiel",
        shot(d(
            &["percussion/glockenspiel"],
            "glockenspiel",
            -12.0,
            0.25,
            0.85,
        )),
    ),
    (
        "xylophone",
        shot(d(&["percussion/xylophone"], "xylophone", -10.0, 0.2, 0.85)),
    ),
    (
        "vibraphone",
        shot(d(
            &["percussion/vibraphone"],
            "vibraphone",
            -12.0,
            0.3,
            0.85,
        )),
    ),
    (
        "celesta",
        shot(d(
            &["percussion/celesta", "keys/celesta"],
            "celesta",
            -12.0,
            -0.4,
            0.6,
        )),
    ),
    (
        "tubular_bells",
        shot(d(
            &["percussion/tubular-bells"],
            "tubular bells",
            -10.0,
            0.35,
            0.9,
        )),
    ),
    // ---- unpitched percussion: Philharmonia → cluesurf one-shots (drums/*)
    (
        "bass_drum",
        phil(shot(d(
            &[
                "percussion/bass-drum",
                "drums/kick-deep",
                "drums/kick-heavy",
                "synth:bass-drum",
            ],
            "bass drum",
            -4.0,
            0.1,
            0.92,
        ))),
    ),
    (
        "snare_drum",
        phil(shot(d(
            &[
                "percussion/snare-drum",
                "drums/snare-layered",
                "drums/snare-body",
                "synth:snare-drum",
            ],
            "snare drum",
            -8.0,
            0.15,
            0.9,
        ))),
    ),
    (
        "cymbals",
        phil(shot(d(
            &[
                "percussion/clash-cymbals",
                "drums/cymbal-crash",
                "synth:cymbals",
            ],
            "cymbals",
            -10.0,
            0.2,
            0.92,
        ))),
    ),
    (
        "suspended_cymbal",
        phil(shot(d(
            &[
                "percussion/suspended-cymbal",
                "drums/cymbal-ride",
                "drums/cymbal-crash",
                "synth:suspended-cymbal",
            ],
            "suspended cymbal",
            -12.0,
            0.25,
            0.92,
        ))),
    ),
    (
        "tam_tam",
        phil(shot(d(
            &[
                "percussion/tam-tam",
                "drums/cymbal-effected",
                "drums/cymbal-crash",
                "synth:tam-tam",
            ],
            "tam-tam",
            -10.0,
            0.3,
            0.95,
        ))),
    ),
    (
        "gong",
        phil(shot(d(
            &[
                "percussion/thai-gong",
                "drums/cymbal-effected",
                "drums/bell",
                "synth:gong",
            ],
            "gong",
            -10.0,
            0.3,
            0.95,
        ))),
    ),
    (
        "triangle",
        phil(shot(d(
            &["percussion/triangle", "drums/bell", "synth:triangle"],
            "triangle",
            -18.0,
            0.3,
            0.9,
        ))),
    ),
    (
        "tambourine",
        phil(shot(d(
            &["percussion/tambourine", "drums/shaker", "synth:tambourine"],
            "tambourine",
            -14.0,
            0.25,
            0.9,
        ))),
    ),
    (
        "woodblock",
        phil(shot(d(
            &[
                "percussion/woodblock",
                "drums/click",
                "drums/rimshot",
                "synth:woodblock",
            ],
            "woodblock",
            -12.0,
            0.2,
            0.9,
        ))),
    ),
    (
        "castanets",
        phil(shot(d(
            &["percussion/castanets", "drums/clap-snap", "synth:castanets"],
            "castanets",
            -12.0,
            0.2,
            0.9,
        ))),
    ),
    (
        "sleigh_bells",
        phil(shot(d(
            &[
                "percussion/sleigh-bells",
                "drums/shaker",
                "synth:sleigh-bells",
            ],
            "sleigh bells",
            -12.0,
            0.25,
            0.9,
        ))),
    ),
    (
        "wind_chimes",
        phil(shot(d(
            &["percussion/wind-chimes", "drums/bell", "synth:wind-chimes"],
            "wind chimes",
            -16.0,
            0.3,
            0.9,
        ))),
    ),
    (
        "tom_toms",
        phil(shot(d(
            &[
                "percussion/tom-toms",
                "drums/tom-classic",
                "drums/tom-heavy",
                "synth:tom-toms",
            ],
            "tom-toms",
            -6.0,
            -0.1,
            0.92,
        ))),
    ),
    // ---- keys, plucked, choir
    (
        "harp",
        shot(d(
            &["plucked/harp", "plucked/celtic-harp"],
            "harp",
            -8.0,
            -0.5,
            0.5,
        )),
    ),
    (
        "celtic_harp",
        shot(d(
            &["plucked/celtic-harp", "plucked/harp"],
            "celtic harp",
            -8.0,
            -0.5,
            0.5,
        )),
    ),
    ("piano", shot(d(&["keys/piano"], "piano", -8.0, -0.2, 0.4))),
    (
        "guitar",
        phil(shot(d(
            &[
                "plucked/guitar",
                "plucked/electric-guitar",
                "plucked/zither",
            ],
            "guitar",
            -8.0,
            -0.3,
            0.4,
        ))),
    ),
    (
        "mandolin",
        phil(shot(d(
            &["plucked/mandolin", "plucked/zither"],
            "mandolin",
            -10.0,
            0.3,
            0.4,
        ))),
    ),
    (
        "banjo",
        phil(shot(d(
            &["plucked/banjo", "plucked/zither"],
            "banjo",
            -10.0,
            0.2,
            0.4,
        ))),
    ),
    ("choir", d(&["choir/mixed"], "choir", -23.0, 0.0, 0.95)),
    (
        "choir_female",
        d(
            &["choir/female", "choir/mixed"],
            "choir (female)",
            -23.0,
            -0.2,
            0.95,
        ),
    ),
    (
        "choir_male",
        d(
            &["choir/male", "choir/mixed"],
            "choir (male)",
            -23.0,
            0.2,
            0.95,
        ),
    ),
];

/// Every preset name (method names of [`Orchestra`]) that can play with the installed samples
/// (directly or through a fallback).
pub fn available() -> Vec<String> {
    PRESETS
        .iter()
        .filter(|(_, d)| d.ids.iter().any(|id| candidate_exists(id)))
        .map(|(n, _)| n.to_string())
        .collect()
}

/// Is a candidate usable right now? (`synth:*` candidates always are.)
fn candidate_exists(id: &str) -> bool {
    id.starts_with("synth:") || instrument_location(id).is_some()
}

/// Render a synthesized one-shot into a sample.
fn render_hit(inst: &mut dyn Instrument, vel: f32, secs: f32) -> crate::sample::Sample {
    let sr = 48_000.0;
    inst.prepare(sr);
    let ctx = Ctx::offline(sr);
    inst.note_on(&NoteOn::new(1, 60.0, vel, 0.1), &ctx);
    let mut out = vec![Frame::ZERO; (secs * sr) as usize];
    for c in out.chunks_mut(64) {
        inst.render(c, &ctx);
    }
    inst.note_off(1, &ctx);
    let last = out
        .iter()
        .rposition(|f| f.peak() > 1e-4)
        .map(|i| i + 1)
        .unwrap_or(out.len());
    out.truncate(last.max(64));
    crate::sample::Sample::from_frames(out, sr)
}

/// Last-resort fallbacks: the built-in drum/FM synths rendered into a 3-layer one-shot patch,
/// so percussion presets play even without any percussion samples installed.
fn synth_patch(name: &str) -> Option<Articulated> {
    use super::patch::{ArticPatch, ArticZone};
    use crate::instrument::Poly;
    use crate::synth::*;
    let mk = |vel: f32| -> Option<crate::sample::Sample> {
        let (mut inst, secs): (Box<dyn Instrument>, f32) = match name {
            "bass-drum" => (
                Box::new(Poly::new(
                    Kick::new()
                        .tune(42.0)
                        .sweep(5.0)
                        .pitch_decay(0.08)
                        .decay(1.6)
                        .click(0.04)
                        .drive(0.05),
                )),
                2.0,
            ),
            "snare-drum" => (
                Box::new(Poly::new(
                    Snare::tr909()
                        .tune(210.0)
                        .snappy(0.95)
                        .decay(0.28)
                        .tone(0.4),
                )),
                0.6,
            ),
            "cymbals" => (
                Box::new(Poly::new(Cymbal::crash().decay(3.0).noise(0.6))),
                3.5,
            ),
            "suspended-cymbal" => (
                Box::new(Poly::new(Cymbal::ride().bell(0.15).decay(4.0))),
                4.5,
            ),
            "tam-tam" => (
                Box::new(Poly::new(
                    Cymbal::china().tune(0.45).tone(2500.0).decay(6.0),
                )),
                6.5,
            ),
            "gong" => (Box::new(Fm::metallic().index(0.6)), 4.0),
            "triangle" => (Box::new(Fm::bell().index(0.4)), 3.0),
            "tambourine" => (Box::new(Poly::new(Shaker::tambourine())), 0.6),
            "woodblock" => (Box::new(Fm::marimba().index(0.6)), 0.4),
            "castanets" => (
                Box::new(Poly::new(Rimshot::new().tune(1400.0).decay(0.02))),
                0.2,
            ),
            "sleigh-bells" => (
                Box::new(Poly::new(Shaker::tambourine().decay(0.45).tone(7000.0))),
                0.7,
            ),
            "wind-chimes" => (Box::new(Fm::bell()), 3.0),
            "tom-toms" => (Box::new(Poly::new(Tom::mid())), 1.0),
            _ => return None,
        };
        Some(render_hit(inst.as_mut(), vel, secs))
    };
    let mut patch = ArticPatch::new(format!("synth:{name}"))
        .attack(0.0005)
        .release(0.3);
    for (vel, lo, hi) in [(0.35, 0.0, 0.45), (0.7, 0.45, 0.8), (1.0, 0.8, 1.0)] {
        let s = mk(vel)?;
        patch = patch.zone(ArticZone::new(s, 60.0).keys(0.0, 127.0).layer(lo, hi));
    }
    patch.oneshot = true;
    Some(Articulated::new(name).artic(Artic::Normal, patch.finish()))
}

/// Print a one-line note once per process.
fn note_once(key: &str, msg: impl FnOnce() -> String) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(key.to_string()) {
        eprintln!("{}", msg());
    }
}

/// Render a held mf note in the middle of the range; return (rms dB, peak dB).
fn measure(a: &Articulated) -> (f32, f32) {
    let sr = 48_000.0;
    let mut m = a.clone().humanize(0.0, 0.0);
    m.prepare(sr);
    let (lo, hi) = m.range();
    let mid = ((lo + hi) * 0.5).round();
    let ctx = Ctx::offline(sr);
    m.note_on(&NoteOn::new(1, mid, 0.65, 1.0), &ctx);
    let mut out = vec![Frame::ZERO; 48_000];
    for c in out.chunks_mut(64) {
        m.render(c, &ctx);
    }
    let pk = out.iter().fold(0.0f32, |x, f| x.max(f.peak()));
    // RMS over the steady part (skip the first 150 ms of attack)
    let st = &out[7_200..];
    let rms =
        (st.iter().map(|f| f.l * f.l + f.r * f.r).sum::<f32>() / (2.0 * st.len() as f32)).sqrt();
    let db = |x: f32| 20.0 * x.max(1e-9).log10();
    (db(rms), db(pk))
}

fn build(def: &Def, preset: &str) -> Result<Articulated, String> {
    let mut last = String::new();
    for (i, id) in def.ids.iter().enumerate() {
        let loaded = match id.strip_prefix("synth:") {
            Some(s) => synth_patch(s).ok_or_else(|| format!("no synth fallback {s}")),
            None => Articulated::load(id),
        };
        match loaded {
            Ok(a) => {
                if i > 0 && def.import {
                    note_once(preset, || {
                        format!(
                            "uwu-wave: using fallback {id} for Orchestra::{preset}() (run `uwu library import philharmonia` for the full instrument)"
                        )
                    });
                }
                let mut a = a
                    .named(def.name)
                    .legato(def.legato)
                    .one_shot(def.oneshot)
                    .auto_short(def.legato)
                    .place(def.pan, def.depth);
                if def.oneshot {
                    a = a.default_artic(Artic::Normal);
                }
                let (rms, pk) = measure(&a.clone().gain(1.0));
                let have = if def.oneshot { pk } else { rms };
                let trim = if have > -100.0 {
                    (def.level - have).clamp(-30.0, 30.0)
                } else {
                    0.0
                };
                return Ok(a.gain(crate::units::db_to_gain(trim)));
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
    build(&def, name).unwrap_or_else(|e| {
        panic!(
            "uwu-wave: Orchestra::{name}() needs one of {:?} in the sound library ({}).\n{e}\nAvailable presets: {}",
            def.ids,
            library_root().display(),
            available().join(", ")
        )
    })
}

/// The orchestra: one constructor per library instrument, each an [`Articulated`] with sensible
/// defaults (legato engine for sections and solo winds/brass, one-shots for percussion,
/// calibrated levels so a tutti is balanced) and a stage seat ([`Articulated::seat`] → (pan, depth)).
///
/// Presets work with just the base library; installing the Philharmonia import
/// (`uwu library import philharmonia` → `assets/local`) upgrades the solo strings, woodwinds,
/// solo brass, trumpet, tuba, unpitched percussion and guitar family to full instruments with
/// pp..ff layers and many articulations. Fallbacks without the import:
///
/// | preset | fallback |
/// |---|---|
/// | `violin_solo` / `viola_solo` | VPO solo sustain (`strings/violin-solo`, `viola-solo`) |
/// | `cello_solo` / `bass_solo` | VPO sections (`strings/cellos`, `basses`) |
/// | `flute` `oboe` `clarinet` `bassoon` | VPO section split (`woodwinds/flutes` …) |
/// | `cor_anglais` | `woodwinds/oboes` (played lower) |
/// | `bass_clarinet` / `saxophone` | `woodwinds/clarinets` |
/// | `contrabassoon` | `woodwinds/bassoons` |
/// | `horn_solo` / `trombone_solo` | VPO sections |
/// | `trumpet` / `tuba` | VPO sections (same id; the import shadows them) |
/// | unpitched percussion | cluesurf one-shots (`drums/kick-deep`, `snare-layered`, `cymbal-crash` …) |
/// | `guitar` `mandolin` `banjo` | `plucked/electric-guitar` / `zither` |
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
/// Constructors panic only when no candidate at all is installed; use [`Orchestra::get`] for a
/// fallible lookup by name.
pub struct Orchestra;

macro_rules! presets {
    ($($(#[$m:meta])* $f:ident),* $(,)?) => {
        impl Orchestra {
            $( $(#[$m])* pub fn $f() -> Articulated { preset(stringify!($f)) } )*
        }
    };
}

presets! {
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
    /// Solo violin (Philharmonia import: sustain, staccato, spiccato, pizzicato, tremolo,
    /// harmonics… pp..ff; else VPO solo sustain).
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
    /// Flute section (VPO).
    flutes,
    /// Oboe section (VPO).
    oboes,
    /// Clarinet section (VPO).
    clarinets,
    /// Bassoon section (VPO).
    bassoons,
    /// Woodwind ensemble.
    woodwinds,
    /// French horn section.
    horns,
    /// Solo French horn (Philharmonia: pp..ff layers, staccato, muted, trills).
    horn_solo,
    trumpet,
    trombones,
    /// Solo trombone (Philharmonia: pp..ff layers).
    trombone_solo,
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
    /// Thai gong.
    gong,
    triangle,
    tambourine,
    woodblock,
    castanets,
    sleigh_bells,
    wind_chimes,
    tom_toms,
    harp,
    celtic_harp,
    piano,
    guitar,
    mandolin,
    banjo,
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
            return build(def, name);
        }
        Articulated::load(name)
    }
    /// Names of every preset that can play with the installed samples.
    pub fn available() -> Vec<String> {
        available()
    }
    /// Which library instrument a preset would load right now (first installed candidate), and
    /// whether that is the preferred one.
    pub fn source_of(name: &str) -> Option<(String, bool)> {
        let def = PRESETS.iter().find(|p| p.0 == name)?.1;
        def.ids
            .iter()
            .enumerate()
            .find(|(_, id)| candidate_exists(id))
            .map(|(i, id)| (id.to_string(), i == 0))
    }
    /// Is a library instrument installed (in any root)?
    pub fn installed(instrument: &str) -> bool {
        instrument_location(instrument).is_some()
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
        // no duplicate names
        let mut names: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len());
    }

    #[test]
    fn synth_fallbacks_always_load() {
        for n in [
            "bass-drum",
            "snare-drum",
            "cymbals",
            "suspended-cymbal",
            "tam-tam",
            "gong",
            "triangle",
            "tambourine",
            "woodblock",
            "castanets",
            "sleigh-bells",
            "wind-chimes",
            "tom-toms",
        ] {
            let a = synth_patch(n).unwrap_or_else(|| panic!("{n}"));
            let (_, pk) = measure(&a.one_shot(true));
            assert!(pk > -40.0 && pk < 6.0, "{n}: {pk}");
        }
        assert!(synth_patch("nope").is_none());
    }

    /// Smoke-test every installed preset: renders finite audio at the calibrated level.
    #[test]
    fn installed_presets_render() {
        for n in available() {
            let Ok(a) = Orchestra::get(&n) else { continue };
            let src = Orchestra::source_of(&n).map(|s| s.0).unwrap_or_default();
            let (rms, pk) = measure(&a.clone().gain(1.0));
            let (rms2, pk2) = measure(&a);
            let def = PRESETS.iter().find(|p| p.0 == n).unwrap().1;
            let got = if def.oneshot { pk2 } else { rms2 };
            println!(
                "{n:<18} {src:<34} raw rms {rms:>6.1} pk {pk:>6.1}  → {got:>6.1} dB (target {:.1})",
                def.level
            );
            assert!(pk2 < 6.0, "{n}: peak {pk2}");
            assert!(got.is_finite());
        }
    }
}
