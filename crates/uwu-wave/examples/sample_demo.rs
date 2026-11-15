//! Sample demo: real piano, bass guitar, celtic harp and an acoustic drum kit — all from the
//! bundled `assets/` (wavebase samples are fetched once into ~/.cache/uwu-wave on first run).
//!
//! ```text
//! cargo run --release --example sample_demo            # → out/Sample_Demo.wav
//! cargo run --release --example sample_demo -- --play  # listen live
//! cargo run --release --example sample_demo -- --info  # arrangement view
//! ```
use uwu_wave::prelude::*;

/// Am – F – C – G, one chord per bar: (bass root, piano voicing).
const CHORDS: [(i32, [i32; 4]); 4] = [
    (33, [57, 60, 64, 69]), // Am
    (29, [57, 60, 65, 69]), // F
    (36, [55, 60, 64, 67]), // C
    (31, [55, 59, 62, 67]), // G
];

/// Broken-chord piano: low note on the beat, the rest of the voicing in eighths.
fn piano_part(vel: f32) -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, (root, v)) in CHORDS.iter().enumerate() {
        let b = bar as f64 * 4.0;
        c = c.note(b, 3.8, root + 12, vel * 0.85);
        let pattern = [v[0], v[1], v[2], v[3], v[2], v[1], v[2], v[3]];
        for (i, p) in pattern.iter().enumerate() {
            let accent = if i % 2 == 0 { 1.0 } else { 0.8 };
            c = c.note(b + 0.5 * i as f64, 0.9, *p, vel * accent);
        }
    }
    c
}

/// Sustained chords for the intro/outro.
fn piano_pads(vel: f32) -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, (root, v)) in CHORDS.iter().enumerate() {
        let b = bar as f64 * 4.0;
        c = c.note(b, 3.9, root + 12, vel);
        for (i, p) in v.iter().enumerate() {
            // a gentle strum
            c = c.note(b + 0.04 * i as f64, 3.8, *p, vel * 0.8);
        }
    }
    c
}

fn bass_part() -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, (root, _)) in CHORDS.iter().enumerate() {
        let b = bar as f64 * 4.0;
        c = c
            .note(b, 1.4, *root, 0.95)
            .note(b + 1.5, 0.45, *root, 0.7)
            .note(b + 2.0, 1.0, root + 12, 0.8)
            .note(b + 3.0, 0.9, root + 7, 0.75);
    }
    c
}

fn drum_part(fill: bool) -> Clip {
    let mut c = Clip::new(4.beats());
    // kick / snare
    c = c
        .note(0.0, 0.25, 36, 1.0)
        .note(1.0, 0.25, 38, 0.9)
        .note(2.5, 0.25, 36, 0.8)
        .note(3.0, 0.25, 38, 0.95);
    // eighth hats with a small accent pattern, open hat on the last off-beat
    for i in 0..8 {
        let t = i as f64 * 0.5;
        if i == 7 {
            c = c.note(t, 0.5, 46, 0.7);
        } else {
            c = c.note(t, 0.25, 42, if i % 2 == 0 { 0.75 } else { 0.5 });
        }
    }
    if fill {
        c = Clip::new(4.beats())
            .note(0.0, 0.25, 36, 1.0)
            .note(1.0, 0.25, 38, 0.9);
        for (i, p) in [50, 50, 48, 48, 45, 45, 41, 41].iter().enumerate() {
            c = c.note(2.0 + 0.25 * i as f64, 0.25, *p, 0.75 + 0.03 * i as f32);
        }
    }
    c
}

fn harp_part() -> Clip {
    // rising arpeggios in sixteenth-note triplets over each chord
    let mut c = Clip::new(16.beats());
    for (bar, (_, v)) in CHORDS.iter().enumerate() {
        let b = bar as f64 * 4.0 + 2.0;
        let notes = [v[0] + 12, v[1] + 12, v[2] + 12, v[3] + 12];
        for (i, p) in notes.iter().enumerate() {
            c = c.note(b + i as f64 / 3.0, 1.5, *p, 0.55 + 0.08 * i as f32);
        }
    }
    c
}

fn main() {
    let song = Song::new("Sample Demo").bpm(92);
    song.master().gain(-3.db());

    let piano = song
        .track("piano", Sampler::piano().release(0.6).gain(-6.db()))
        .pan(-0.1);
    let bass = song.track("bass", Sampler::bass_guitar().gain(-3.db()));
    let harp = song.track("harp", Sampler::harp().gain(-6.db())).pan(0.35);
    let drums = song.track("drums", SampleKit::acoustic().gain(-8.db()));

    song.section("intro", 4.bars(), |s| {
        s.play(&piano, piano_pads(0.55));
    });
    song.section("groove", 8.bars(), |s| {
        s.play(&piano, piano_part(0.65));
        s.play(&bass, bass_part());
        s.during(bar(0)..bar(7), &drums, drum_part(false));
        s.at(bar(7), &drums, drum_part(true));
        s.during(bar(4)..bar(8), &harp, harp_part());
    });
    song.section("lift", 8.bars(), |s| {
        s.play(&piano, piano_part(0.8));
        s.play(&bass, bass_part());
        s.at(bar(0), &drums, Clip::new(1.beats()).note(0, 1, 49, 1.0));
        s.during(bar(0)..bar(7), &drums, drum_part(false));
        s.at(bar(7), &drums, drum_part(true));
        s.play(&harp, harp_part().vel(1.2));
    });
    song.section("outro", 4.bars(), |s| {
        s.play(&piano, piano_pads(0.45));
        s.at(
            bar(0),
            &drums,
            Clip::new(1.beats()).note(0, 1, 49, 0.9).note(0, 1, 36, 1.0),
        );
        s.at(bar(0), &bass, Clip::new(4.beats()).note(0, 4, 33, 0.9));
        s.at(
            bar(3),
            &harp,
            Clip::new(4.beats())
                .note(0, 4, 69, 0.6)
                .note(0.2, 4, 72, 0.6)
                .note(0.4, 4, 76, 0.7)
                .note(0.6, 4, 81, 0.75),
        );
    });
    song.run();
}
