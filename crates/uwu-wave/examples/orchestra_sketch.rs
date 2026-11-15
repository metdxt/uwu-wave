//! orchestra_sketch — a short test piece for the articulated orchestra engine.
//!
//! `cargo run --release --example orchestra_sketch` (needs `assets/library`; missing
//! instruments are skipped).
//!
//! Shows: slurred legato lines with portamento, CC1 crescendo/diminuendo across sustained
//! notes, staccato/pizzicato articulations, accents, and percussion one-shots.

use uwu_wave::prelude::*;

/// (beat, beats, midi, vel, artic)
fn line(len: f64, notes: &[(f64, f64, i32, f32, Artic)]) -> Clip {
    let mut c = Clip::new(beat(len));
    for &(s, d, p, v, a) in notes {
        c.push(Note::new(beat(s), beat(d), p as f32, v).artic(a));
    }
    c
}

fn main() {
    use Artic::*;
    let song = Song::new("orchestra_sketch").bpm(72);
    let hall = song.bus("hall").fx(Reverb::hall().mix(1.0).decay(2.6));
    song.master().fx(Limiter::master());

    // add a track if the instrument is installed; seat → pan
    let add = |name: &str, preset: &str, send_db: f32| -> Option<Track> {
        match Orchestra::get(preset) {
            Ok(inst) => {
                let (pan, depth) = inst.seat();
                Some(
                    song.track(name, inst)
                        .pan(pan * 0.8)
                        .send(&hall, (send_db - 6.0 * (1.0 - depth)).db()),
                )
            }
            Err(e) => {
                eprintln!("skipping {preset}: {}", e.lines().next().unwrap_or(""));
                None
            }
        }
    };
    let vln1 = add("violins 1", "violins_1", -10.0);
    let vln2 = add("violins 2", "violins_2", -10.0);
    let vla = add("violas", "violas", -10.0);
    let vc = add("cellos", "cellos", -11.0);
    let cb = add("basses", "basses", -12.0);
    let fl = add("flute", "flute", -9.0);
    let hn = add("horns", "horns", -8.0);
    let timp = add("timpani", "timpani", -10.0);
    let harp = add("harp", "harp", -8.0);

    // D minor: i – VI – iv – V
    let melody = line(
        16.0,
        &[
            (0.0, 1.5, 74, 0.7, Normal),
            (1.5, 0.6, 76, 0.7, Normal), // overlaps → slurred
            (2.0, 1.0, 77, 0.75, Normal),
            (3.0, 1.0, 76, 0.7, Normal),
            (4.0, 2.0, 74, 0.7, Normal),
            (6.0, 2.0, 70, 0.7, Legato),
            (8.0, 1.0, 72, 0.75, Normal),
            (9.0, 1.0, 74, 0.8, Normal),
            (10.0, 2.0, 77, 0.85, Accent),
            (12.0, 1.5, 76, 0.8, Normal),
            (13.5, 0.6, 74, 0.7, Normal),
            (14.0, 2.0, 73, 0.75, Normal),
        ],
    );
    let pad = |notes: [[i32; 4]; 4], vel: f32| {
        let mut c = Clip::new(beat(16.0));
        for (i, ch) in notes.iter().enumerate() {
            for &p in ch {
                c.push(Note::new(beat(i as f64 * 4.0), beat(4.02), p as f32, vel));
            }
        }
        c
    };
    let chords = [
        [62, 65, 69, 74],
        [58, 62, 65, 70],
        [55, 58, 62, 67],
        [57, 61, 64, 69],
    ];
    let bass_line = line(
        16.0,
        &[
            (0.0, 4.0, 38, 0.7, Sustain),
            (4.0, 4.0, 34, 0.7, Sustain),
            (8.0, 4.0, 31, 0.7, Sustain),
            (12.0, 4.0, 33, 0.7, Sustain),
        ],
    );
    let pizz = {
        let mut c = Clip::new(beat(16.0));
        for (i, ch) in chords.iter().enumerate() {
            for k in 0..4 {
                c.push(
                    Note::new(
                        beat(i as f64 * 4.0 + k as f64),
                        beat(0.4),
                        (ch[k % 4] - 12) as f32,
                        0.7,
                    )
                    .artic(Pizzicato),
                );
            }
        }
        c
    };
    let stacc = {
        let mut c = Clip::new(beat(16.0));
        for (i, ch) in chords.iter().enumerate() {
            for k in 0..8 {
                c.push(
                    Note::new(
                        beat(i as f64 * 4.0 + k as f64 * 0.5),
                        beat(0.25),
                        ch[1 + k % 3] as f32,
                        if k % 2 == 0 { 0.85 } else { 0.6 },
                    )
                    .artic(Staccato),
                );
            }
        }
        c
    };
    let arp = {
        let mut c = Clip::new(beat(16.0));
        for (i, ch) in chords.iter().enumerate() {
            for k in 0..8 {
                c.push(Note::new(
                    beat(i as f64 * 4.0 + k as f64 * 0.5),
                    beat(1.0),
                    (ch[k % 4] + if k >= 4 { 12 } else { 0 }) as f32,
                    0.6,
                ));
            }
        }
        c
    };
    let timp_roll = line(
        16.0,
        &[
            (0.0, 1.0, 38, 0.9, Normal),
            (12.0, 1.0, 33, 0.8, Normal),
            (15.0, 0.5, 33, 0.6, Normal),
            (15.5, 0.5, 33, 0.8, Normal),
        ],
    );

    song.section("intro", 4.bars(), |s| {
        if let Some(t) = &vln2 {
            s.play(
                t,
                pad(
                    [
                        [62, 65, 69, 74],
                        [62, 65, 70, 74],
                        [62, 67, 70, 74],
                        [61, 64, 69, 73],
                    ],
                    0.6,
                ),
            );
            s.cresc(t, Dyn::PP, Dyn::MF);
        }
        if let Some(t) = &vla {
            s.play(t, pad(chords.map(|c| [c[0], c[1], c[1], c[0]]), 0.6));
            s.cresc(t, Dyn::PP, Dyn::MF);
        }
        if let Some(t) = &vc {
            s.play(t, bass_line.clone().transpose(12.0));
            s.cresc(t, Dyn::P, Dyn::MF);
        }
        if let Some(t) = &harp {
            s.play(t, arp.clone());
        }
        if let Some(t) = &fl {
            s.play(t, melody.clone());
            s.dynamics(t, Dyn::MP);
        }
    });
    song.section("tutti", 4.bars(), |s| {
        if let Some(t) = &vln1 {
            s.play(t, melody.clone());
            s.cresc(t, Dyn::MF, Dyn::FF);
        }
        if let Some(t) = &vln2 {
            s.play(t, stacc.clone());
            s.dynamics(t, Dyn::F);
        }
        if let Some(t) = &vla {
            s.play(t, stacc.clone().transpose(-12.0));
            s.dynamics(t, Dyn::F);
        }
        if let Some(t) = &vc {
            s.play(t, pizz.clone());
            s.dynamics(t, Dyn::F);
        }
        if let Some(t) = &cb {
            s.play(t, bass_line.clone());
            s.cresc(t, Dyn::MF, Dyn::F);
        }
        if let Some(t) = &hn {
            s.play(
                t,
                pad(
                    chords.map(|c| [c[0] - 12, c[2] - 12, c[0] - 12, c[2] - 12]),
                    0.7,
                ),
            );
            s.cresc(t, Dyn::P, Dyn::F);
        }
        if let Some(t) = &timp {
            s.play(t, timp_roll.clone());
        }
    });
    song.section("coda", 2.bars(), |s| {
        let end = Clip::new(beat(8.0));
        if let Some(t) = &vln1 {
            s.once(t, end.clone().note(0, 7.5, 74, 0.6));
            s.cresc(t, Dyn::MF, Dyn::PPP);
        }
        if let Some(t) = &vla {
            s.once(t, end.clone().note(0, 7.5, 65, 0.6).note(0, 7.5, 69, 0.6));
            s.cresc(t, Dyn::MF, Dyn::PPP);
        }
        if let Some(t) = &vc {
            s.once(t, end.clone().note(0, 7.5, 50, 0.6));
            s.cresc(t, Dyn::MF, Dyn::PPP);
        }
        if let Some(t) = &cb {
            s.once(t, end.clone().note(0, 7.5, 38, 0.6));
            s.cresc(t, Dyn::MF, Dyn::PPP);
        }
        if let Some(t) = &timp {
            s.once(t, end.clone().note(0, 1.0, 38, 0.5));
        }
    });
    song.run();
}
