//! synth_demo — a short tour of the uwu-wave synthesizers.
//!
//! `cargo run --example synth_demo` → writes `out/synth_demo.wav`.
//!
//! Everything here is synthesized: a TR-909 kit, a 303-style acid line with an automated filter,
//! a supersaw/pad bed, FM e-piano and bells, a Karplus–Strong harp, a drawbar organ and a lead.

use uwu_wave::prelude::*;

/// Step sequencer helper: one char per 16th note. `x` = hit, `X` = accent, `o` = ghost, anything
/// else = rest.
fn steps(pattern: &str, pitch: u8) -> Clip {
    let n = pattern.chars().count();
    let mut c = Clip::new(beat(n as f64 * 0.25));
    for (i, ch) in pattern.chars().enumerate() {
        let vel = match ch {
            'X' => 1.0,
            'x' => 0.8,
            'o' => 0.45,
            _ => continue,
        };
        c = c.note(beat(i as f64 * 0.25), sixteenth(), pitch, vel);
    }
    c
}

/// Notes as (start_beat, length_beats, midi, velocity).
fn clip(len_beats: f64, notes: &[(f64, f64, i32, f32)]) -> Clip {
    notes
        .iter()
        .fold(Clip::new(beat(len_beats)), |c, &(s, d, p, v)| {
            c.note(beat(s), beat(d), p, v)
        })
}

/// Block chords: each chord lasts `len` beats.
fn chords(len: f64, prog: &[&[i32]], vel: f32) -> Clip {
    let mut c = Clip::new(beat(len * prog.len() as f64));
    for (i, ch) in prog.iter().enumerate() {
        for &p in ch.iter() {
            c = c.note(beat(i as f64 * len), beat(len * 0.98), p, vel);
        }
    }
    c
}

fn main() {
    let song = Song::new("synth_demo").bpm(124);

    // ---- buses
    let verb = song.bus("verb").fx(Reverb::hall().mix(1.0));
    song.master().fx(Limiter::master());

    // ---- instruments
    let cutoff = Lane::new(300.0);
    let drums = song.track("drums", DrumMachine::tr909()).gain(-5.db());
    let acid = song
        .track("acid", Synth::acid().cutoff(&cutoff).res(0.84))
        .gain(-4.db())
        .send(&verb, -18.db());
    let pad = song
        .track(
            "pad",
            Synth::pad().cutoff(lfo::sine(8.bars()).range(1200.0, 3200.0)),
        )
        .gain(-6.db())
        .send(&verb, -8.db());
    let saws = song
        .track("supersaw", Synth::supersaw())
        .gain(-9.db())
        .send(&verb, -10.db());
    let ep = song
        .track("epiano", Fm::epiano())
        .gain(-5.db())
        .pan(-0.2)
        .send(&verb, -12.db());
    let bell = song
        .track("bell", Fm::bell())
        .gain(-12.db())
        .pan(0.3)
        .send(&verb, -4.db());
    let harp = song
        .track("harp", Pluck::harp())
        .gain(-6.db())
        .pan(0.25)
        .send(&verb, -8.db());
    let organ = song
        .track("organ", Organ::jazz())
        .gain(-10.db())
        .pan(-0.3)
        .send(&verb, -14.db());
    let lead = song
        .track("lead", Synth::lead())
        .gain(-6.db())
        .send(&verb, -10.db());
    let sub = song
        .track(
            "808",
            DrumMachine::only(Kick::tr808().pitched().decay(1.4).sweep(7.0)),
        )
        .gain(-8.db());

    // ---- material (A minor: Am – F – C – G)
    let prog: [&[i32]; 4] = [&[57, 60, 64], &[53, 57, 60], &[55, 60, 64], &[55, 59, 62]];
    let pad_chords = chords(4.0, &prog, 0.7);
    let stabs = {
        let mut c = Clip::new(16.beats());
        for (i, ch) in prog.iter().enumerate() {
            for off in [0.5, 1.5, 2.5, 3.25] {
                for &p in ch.iter() {
                    c = c.note(beat(i as f64 * 4.0 + off), beat(0.2), p + 12, 0.75);
                }
            }
        }
        c
    };
    // 303 line: overlapping notes slide (legato), velocity 1.0 = accent
    let acid_line = clip(
        4.0,
        &[
            (0.0, 0.25, 33, 1.0),
            (0.25, 0.25, 33, 0.6),
            (0.5, 0.3, 45, 0.7),
            (0.75, 0.25, 33, 0.6),
            (1.0, 0.5, 36, 1.0),
            (1.5, 0.3, 33, 0.6),
            (1.75, 0.25, 43, 0.7),
            (2.0, 0.25, 33, 1.0),
            (2.25, 0.25, 45, 0.6),
            (2.5, 0.25, 33, 0.7),
            (2.75, 0.5, 40, 0.8), // slides into the next note
            (3.0, 0.25, 41, 1.0),
            (3.25, 0.25, 33, 0.6),
            (3.5, 0.25, 45, 0.7),
            (3.75, 0.25, 31, 0.9),
        ],
    );
    let arp = {
        let mut c = Clip::new(16.beats());
        for (i, ch) in prog.iter().enumerate() {
            let tones = [ch[0], ch[1], ch[2], ch[0] + 12, ch[2], ch[1]];
            for k in 0..8 {
                c = c.note(
                    beat(i as f64 * 4.0 + k as f64 * 0.5),
                    beat(0.45),
                    tones[k % tones.len()] + 12,
                    if k % 4 == 0 { 0.9 } else { 0.65 },
                );
            }
        }
        c
    };
    let ep_comp = clip(
        16.0,
        &[
            (0.0, 1.5, 57, 0.8),
            (0.0, 1.5, 64, 0.7),
            (0.0, 1.5, 72, 0.75),
            (4.5, 1.0, 57, 0.7),
            (4.5, 1.0, 65, 0.65),
            (4.5, 1.0, 72, 0.7),
            (8.0, 1.5, 55, 0.75),
            (8.0, 1.5, 64, 0.7),
            (8.0, 1.5, 72, 0.7),
            (12.5, 1.0, 55, 0.7),
            (12.5, 1.0, 62, 0.65),
            (12.5, 1.0, 71, 0.75),
        ],
    );
    let bells = clip(
        16.0,
        &[
            (0.0, 2.0, 76, 0.8),
            (6.0, 2.0, 72, 0.6),
            (8.0, 2.0, 79, 0.7),
            (14.0, 2.0, 74, 0.6),
        ],
    );
    let melody = clip(
        16.0,
        &[
            (0.0, 1.5, 76, 0.9),
            (1.5, 0.5, 74, 0.7),
            (2.0, 1.0, 72, 0.8),
            (3.0, 1.0, 69, 0.7),
            (4.0, 1.5, 72, 0.9),
            (5.5, 0.6, 74, 0.7),
            (6.0, 2.0, 76, 0.85),
            (8.0, 1.0, 79, 1.0),
            (9.0, 1.0, 76, 0.8),
            (10.0, 2.0, 74, 0.85),
            (12.0, 1.0, 71, 0.8),
            (13.0, 1.0, 72, 0.8),
            (14.0, 2.0, 74, 0.9),
        ],
    );
    let subs = clip(
        16.0,
        &[
            (0.0, 3.0, 33, 1.0),
            (4.0, 3.0, 29, 1.0),
            (8.0, 3.0, 36, 1.0),
            (12.0, 2.0, 31, 1.0),
            (14.0, 1.5, 31, 0.8),
        ],
    );

    let kick = steps("X---x---x---x---", 36);
    let clap = steps("----x-------x---", 39);
    let hats = steps("--x---x---x---xo", 42) & steps("o-o-o-o-o-o-o-o-", 44);
    let open = steps("------------x---", 46);
    let ride = steps("x-x-x-x-x-x-x-x-", 51);
    let fill = steps("x-x-x-x-xxxxXXXX", 38) & steps("--------x-x-x-x-", 48);

    // ---- arrangement
    song.section("intro", 4.bars(), |s| {
        s.play(&pad, pad_chords.clone());
        s.play(&ep, ep_comp.clone());
        s.play(&drums, hats.clone());
        s.at(bar(3), &drums, fill.clone());
    });
    song.section("groove", 8.bars(), |s| {
        s.play(
            &drums,
            kick.clone() & clap.clone() & hats.clone() & open.clone(),
        );
        s.play(&acid, acid_line.clone());
        s.ramp_exp(&cutoff, 220.0, 2600.0);
        s.play(&pad, pad_chords.clone());
        s.play(&harp, arp.clone());
        s.play(&organ, stabs.clone());
    });
    song.section("break", 4.bars(), |s| {
        s.play(&saws, pad_chords.clone());
        s.play(&bell, bells.clone());
        s.play(&harp, arp.clone());
        s.play(&drums, ride.clone());
        s.at(bar(3), &drums, fill.clone());
    });
    song.section("drop", 8.bars(), |s| {
        s.play(
            &drums,
            kick.clone()
                & clap.clone()
                & hats.clone()
                & open.clone()
                & steps("x---------------", 49).times(1),
        );
        s.play(&acid, acid_line.clone());
        s.set(&cutoff, 900.0);
        s.play(&sub, subs.clone());
        s.play(&saws, stabs.clone());
        s.play(&ep, ep_comp.clone());
        s.play(&lead, melody.clone());
        s.play(&bell, bells.clone());
    });
    song.section("outro", 2.bars(), |s| {
        s.once(&pad, chords(8.0, &[&[57, 64, 69, 72]], 0.7));
        s.once(&bell, clip(1.0, &[(0.0, 1.0, 81, 0.7)]));
        s.once(&drums, steps("X", 49) & steps("X", 36));
    });

    song.run();
}
