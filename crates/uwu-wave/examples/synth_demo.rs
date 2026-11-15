//! synth_demo — a short tour of the uwu-wave synthesizers.
//!
//! `cargo run --example synth_demo` → writes `out/synth_demo.wav`.
//!
//! Everything here is synthesized: a TR-909 kit, a 303-style acid line with an automated filter,
//! a supersaw/pad bed, FM e-piano and bells, a Karplus–Strong harp, a drawbar organ and a lead.

use uwu_wave::prelude::*;

/// Notes as (start_beat, length_beats, midi, velocity).
fn clip(len_beats: f64, notes: &[(f64, f64, i32, f32)]) -> Clip {
    notes
        .iter()
        .fold(Clip::new(beat(len_beats)), |c, &(s, d, p, v)| {
            c.note(beat(s), beat(d), p, v)
        })
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

    // ---- material (A minor: i – VI – III – VII = Am F C G, voice-led)
    let key = Key::minor(A3);
    let chords = voice_lead(key.prog("i VI III VII"));
    let pads = chords.pat().legato(0.98).vel(0.7);
    let stabs = chords.pat().oct(1).struct_("~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ x ~ ~").legato(0.8).vel(0.75);
    // the 303 line stays a hand-written Clip: overlapping notes slide (legato), velocity 1.0 = accent
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
    let arp = chords.pat().oct(1).arp("updown", 8).vels("0.9 0.65 0.65 0.65").legato(0.9);
    // e-piano comping: drop-2 voicings on a two-bar rhythm (x@3 = a dotted quarter)
    let ep_comp = chords.pat().drop2().struct_("<[x@3 ~@5] [~ x@2 ~@5]>");
    let bells = p("<[e5^0.8 ~] [~ c5^0.6] [g5^0.7 ~] [~ d5^0.6]>");
    let melody = p("<[e5@3 d5 c5@2 a4@2] [c5@3 d5 e5@4] [g5@2 e5@2 d5@4] [b4@2 c5@2 d5@4]>")
        .legato(0.95)
        .humanize(0.01, 0.08);
    let subs = p("<[a1@3 ~] [f1@3 ~] [c2@3 ~] [g1 g1^0.64]>").legato(0.9);

    let hats = drums! {
        hh: "..x. ..x. ..x. ..xo",
        ph: "o.o. o.o. o.o. o.o.",
    };
    let groove = &hats
        & drums! {
            bd: "X... x... x... x...",
            cp: ".... x... .... x...",
            oh: ".... .... .... x...",
        };
    let ride = grid("rd", "x.x. x.x. x.x. x.x.");
    let fill = drums! {
        sd: "x.x. x.x. xxxx XXXX",
        ht: ".... .... x.x. x.x.",
    };

    // ---- arrangement
    song.section("intro", 4.bars(), |s| {
        s.play(&pad, &pads);
        s.play(&ep, &ep_comp);
        s.play(&drums, &hats);
        s.at(bar(3), &drums, &fill);
    });
    song.section("groove", 8.bars(), |s| {
        s.play(&drums, &groove);
        s.play(&acid, &acid_line);
        s.ramp_exp(&cutoff, 220.0, 2600.0);
        s.play(&pad, &pads);
        s.play(&harp, &arp);
        s.play(&organ, &stabs);
    });
    song.section("break", 4.bars(), |s| {
        s.play(&saws, &pads);
        s.play(&bell, &bells);
        s.play(&harp, &arp);
        s.play(&drums, &ride);
        s.at(bar(3), &drums, &fill);
    });
    song.section("drop", 8.bars(), |s| {
        s.play(&drums, &groove & "<cr ~ ~ ~>");
        s.play(&acid, &acid_line);
        s.set(&cutoff, 900.0);
        s.play(&sub, &subs);
        s.play(&saws, &stabs);
        s.play(&ep, &ep_comp);
        s.play(&lead, &melody);
        s.play(&bell, &bells);
    });
    song.section("outro", 2.bars(), |s| {
        s.once(&pad, Chord::new([A3, E4, A4, C5]).pat().slow(2).vel(0.7));
        s.once(&bell, "a5^0.7 ~ ~ ~");
        s.once(&drums, "[cr, bd]^1 ~ ~ ~");
    });

    song.run();
}
