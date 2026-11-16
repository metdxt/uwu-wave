//! violin — a short solo-violin showcase for the articulated engine.
//!
//! `cargo run --release --example violin`
//!
//! Uses the Philharmonia solo violin (`strings/violin`, installed into `assets/local` by
//! `uwu library import philharmonia`): a slurred legato phrase with portamento, a spiccato run,
//! pizzicato, tremolo, harmonics, CC1 crescendo/diminuendo and CC21 vibrato, over a quiet
//! string pad. Without the import it explains how to get it and plays the VPO fallback.

use uwu_wave::prelude::*;

/// (beat, beats, midi, vel, artic)
fn phrase(len: f64, notes: &[(f64, f64, i32, f32, Artic)]) -> Clip {
    let mut c = Clip::new(beat(len));
    for &(s, d, p, v, a) in notes {
        c.push(Note::new(beat(s), beat(d), p as f32, v).artic(a));
    }
    c
}

fn chords(prog: &[[i32; 3]], vel: f32) -> Clip {
    let mut c = Clip::new(beat(prog.len() as f64 * 4.0));
    for (i, ch) in prog.iter().enumerate() {
        for &p in ch {
            c.push(Note::new(beat(i as f64 * 4.0), beat(4.02), p as f32, vel));
        }
    }
    c
}

fn main() {
    use Artic::*;
    if !Orchestra::installed("strings/violin") {
        eprintln!("violin: the full solo violin (strings/violin) is not installed.");
        eprintln!(
            "  Download the Philharmonia samples (https://philharmonia.co.uk/resources/sound-samples/)"
        );
        eprintln!(
            "  next to the repo and run `uwu library import philharmonia`. Playing the VPO fallback."
        );
    }
    let song = Song::new("violin").bpm(76);
    let hall = song.bus("hall").fx(Reverb::hall().mix(1.0).decay(2.4));
    song.master().fx(Limiter::master());

    let solo = Orchestra::violin_solo().legato_speed(0.12).portamento(0.08);
    let (pan, _) = solo.seat();
    let vln = song
        .track("violin", solo)
        .pan(pan * 0.5)
        .send(&hall, (-9).db());
    let pad = song
        .track("strings", Orchestra::strings())
        .gain((-9).db())
        .send(&hall, (-6).db());

    // A minor. Notes overlap their successor slightly -> slurred, with portamento.
    let legato = phrase(
        16.0,
        &[
            (0.0, 1.55, 69, 0.7, Normal),
            (1.5, 0.55, 72, 0.7, Normal),
            (2.0, 1.05, 76, 0.75, Normal),
            (3.0, 1.05, 74, 0.7, Normal),
            (4.0, 2.05, 81, 0.8, Legato), // big leap: audible portamento
            (6.0, 1.05, 79, 0.75, Normal),
            (7.0, 1.0, 76, 0.7, Normal),
            (8.0, 3.05, 77, 0.75, Normal),
            (11.0, 1.05, 76, 0.7, Normal),
            (12.0, 4.0, 72, 0.7, Normal),
        ],
    );
    let spiccato = {
        let run = [
            69, 71, 72, 74, 76, 77, 79, 81, 83, 81, 79, 77, 76, 74, 72, 71,
        ];
        let mut c = Clip::new(beat(4.0));
        for (i, &p) in run.iter().enumerate() {
            let v = if i % 4 == 0 { 0.85 } else { 0.6 };
            c.push(Note::new(beat(i as f64 * 0.25), beat(0.2), p as f32, v).artic(Spiccato));
        }
        c
    };
    let pizz = phrase(
        4.0,
        &[
            (0.0, 0.5, 57, 0.8, Pizzicato),
            (1.0, 0.5, 64, 0.7, Pizzicato),
            (2.0, 0.5, 69, 0.75, Pizzicato),
            (2.5, 0.5, 72, 0.7, Pizzicato),
            (3.0, 1.0, 76, 0.8, Pizzicato),
        ],
    );
    let trem = phrase(
        4.0,
        &[(0.0, 2.0, 76, 0.7, Tremolo), (2.0, 2.0, 77, 0.75, Tremolo)],
    );
    let harm = phrase(
        4.0,
        &[(0.0, 2.0, 88, 0.6, Harmonic), (2.0, 2.0, 93, 0.6, Harmonic)],
    );

    song.section("cantabile", 4.bars(), |s| {
        s.play(&vln, legato.clone());
        s.hairpin(&vln, bars(0..2), Dyn::P, Dyn::F);
        s.hairpin(&vln, bars(2..4), Dyn::F, Dyn::PP);
        s.cc_ramp(&vln, cc::VIBRATO, 0.2, 0.8);
        s.play(
            &pad,
            chords(
                &[[57, 60, 64], [53, 57, 60], [55, 59, 62], [52, 57, 60]],
                0.5,
            ),
        );
        s.dynamics(&pad, Dyn::PP);
    });
    song.section("spiccato", 2.bars(), |s| {
        s.play(&vln, spiccato.clone());
        s.dynamics(&vln, Dyn::MF);
        s.play(&pad, chords(&[[57, 60, 64], [52, 56, 59]], 0.5));
    });
    song.section("pizzicato", 1.bars(), |s| {
        s.once(&vln, pizz.clone());
        s.dynamics(&vln, Dyn::F);
    });
    song.section("tremolo", 1.bars(), |s| {
        s.once(&vln, trem.clone());
        s.cresc(&vln, Dyn::PP, Dyn::FF);
        s.once(&pad, chords(&[[53, 57, 60]], 0.5));
    });
    song.section("harmonics", 1.bars(), |s| {
        s.once(&vln, harm.clone());
        s.dynamics(&vln, Dyn::P);
        s.once(&pad, chords(&[[57, 64, 69]], 0.4));
        s.dynamics(&pad, Dyn::PPP);
    });
    song.run();
}
