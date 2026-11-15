//! # Neon Rain — the uwu-wave flagship
//!
//! A complete synthwave / house track written as a Rust program. It touches almost every
//! feature of the DAW: buses & sends, sidechain ducking, automation lanes written by sections,
//! LFO-modulated knobs, a master chain, roman-numeral harmony with voice leading,
//! mini-notation, drum grids, a sampled piano and tempo automation.
//!
//! ```text
//! cargo run --release --example neon_rain              # render → out/Neon_Rain.wav
//! cargo run --release --example neon_rain -- --play    # listen
//! cargo run --release --example neon_rain -- --info    # arrangement view
//! cargo run --release --example neon_rain -- --play --from drop --solo bass,kick
//! cargo run --release --example neon_rain -- --stems --midi out/neon_rain.mid
//! ```

use uwu_wave::prelude::*;

fn main() {
    // ───────────────────────────── song ─────────────────────────────
    let song = Song::new("Neon Rain")
        .meter(4, 4)
        // a gentle push into the drop, settle back for the outro
        .bpm(auto![0 => 118, bar(24) => 118, bar(32) => 122 => smooth, bar(48) => 122, bar(52) => 114 => smooth]);
    let key = Key::new(A3, Scale::Minor);

    // ───────────────────── automation lanes ─────────────────────
    // created up-front, handed to knobs, written later by the arrangement
    let sweep = Lane::new(20_000.0); // low-pass on the whole music bus (risers / breakdowns)
    let lead_cut = Lane::new(2_400.0); // supersaw brightness
    let wheel = Lane::new(0.0); // lead mod wheel (vibrato + filter swell)
    let scoop = Lane::new(0.0); // lead pitch bend in semitones
    let crush = Lane::new(0.0); // bitcrush wet amount on the drums

    // ──────────────────────────── buses ────────────────────────────
    let verb = song.bus("verb").fx(Reverb::hall().decay(3.2).predelay(25.ms()).wet()).gain(-4.db());
    let echo = song
        .bus("echo")
        .fx(Delay::new(dotted(eighth())).feedback(0.42).ping_pong().high_cut(5_000.0).wet())
        .send(&verb, -10.db())
        .gain(-6.db());
    let music = song
        .bus("music")
        .fx(LowPass::new(&sweep).q(0.8).slope(24))
        .fx(Ducker::new("kick").amount(0.55).release(160.ms()));
    song.master()
        .fx(Eq::new().low_cut(28.0).high_shelf(9_000.0, 1.5.db()))
        .fx(Compressor::glue())
        .fx(Limiter::master().ceiling(-1.db()));

    // ──────────────────────────── tracks ────────────────────────────
    let kick = song.track("kick", DrumMachine::tr909()).gain(-3.db());
    let drums = song
        .track("drums", DrumMachine::tr808())
        .fx(Bitcrush::new().bits(6.0).rate(9_000).mix(&crush))
        .gain(-8.db())
        .send(&verb, -20.db());
    let bass = song
        .track("bass", Synth::acid().cutoff(lfo::sine(2.bars()).range(350.0, 2_200.0)).res(0.65))
        .fx(Distortion::tube(1.8).mix(0.5))
        .gain(-9.db())
        .to(&music);
    let sub = song.track("808", Poly::new(Kick::tr808().pitched()).mono()).gain(-10.db()).to(&music);
    let pad = song
        .track("pad", Synth::pad())
        .fx(Chorus::juno())
        .gain(-10.db())
        .pan(lfo::sine(8.bars()).range(-0.25, 0.25))
        .send(&verb, -4.db())
        .to(&music);
    let piano = song
        .track("piano", Sampler::piano())
        .gain(-9.db())
        .pan(-0.25)
        .send(&verb, -8.db())
        .to(&music);
    let lead = song
        .track("lead", Synth::supersaw().cutoff(&lead_cut).mod_cutoff(1.0))
        .bend(&scoop)
        .modwheel(&wheel)
        .gain(-8.db())
        .send(&echo, -8.db())
        .send(&verb, -12.db())
        .to(&music);
    let bells = song
        .track("bells", Fm::bell())
        .gain(-11.db())
        .pan(0.4)
        .send(&echo, -6.db())
        .send(&verb, -6.db());
    let harp = song
        .track("harp", Pluck::harp())
        .fx(AutoPan::new(2.bars()).depth(0.6))
        .gain(-12.db())
        .send(&verb, -6.db())
        .to(&music);

    // ─────────────────────────── material ───────────────────────────
    // i – VI – III – VII  (Am F C G), voice-led around A4
    let chords = voice_lead(key.oct(1).prog("i VI III VII"));
    let pads = chords.pat().legato(1.0).vel(0.55);
    let stabs = chords.pat().struct_("~ x ~ ~ ~ x ~ x").legato(0.4).vel(0.7);
    let arps = chords.pat().oct(1).arp("updown", 16).vel(0.5).humanize(0.005, 0.1);

    let four = p("bd*4");
    let groove = drums! {
        cp: ".... x... .... x...",
        hh: "x.x. x.xx x.x. x.x.",
        oh: "..x. ..x. ..x. ..x.",
    }
    .swing16(0.2);
    let fill = p("~ ~ ~ [sd sd [sd sd] [ht mt lt lt]]^0.75");
    let beat = groove.last_of(4, |p| p & &fill);

    // acid line in scale degrees, accents on the off-beats
    let acid = n("<[0 0 7 0 3 0 10 5] [0 ~ 0 12 ~ 3 5 7] [-2 -2 5 -2 3 ~ 2 3] [-3 4 -3 ~ 2 -3 6 ~]>")
        .scale(key.oct(-1))
        .fast(2)
        .vels("1 .6 .8 .6")
        .legato(0.85);
    let sub_line = n("<0 -2 2 -3>").scale(key.oct(-1)).struct_("x ~ ~ x ~ ~ x ~").legato(0.9);

    let melody = n("<[4 ~ 3 4 ~ 6 ~ 4] [3 ~ 2 ~ 0@2 ~ ~] [4 ~ 3 4 ~ 7 ~ 9] [8@3 7 6@2 4 ~]>")
        .scale(key.oct(1))
        .legato(0.92)
        .off(eighth().0 / 4.0, |p| p.up(12).gain(0.3));
    let piano_part = chords.pat().strum(0.04).vel(0.6) & melody.down(12).gain(0.5);
    let sparkle = p("e6 ~ b5 ~ ~ a5 ~ ~, ~ ~ ~ c6?0.5").slow(2).vel(0.6);

    // ───────────────────────── arrangement ─────────────────────────
    song.section("intro", 8.bars(), |s| {
        s.play(&pad, &pads);
        s.play(&piano, chords.pat().strum(0.06).vel(0.5));
        s.during(4.bars()..8.bars(), &drums, p("hh*8").vels("0.5 0.25"));
        s.ramp_exp(&sweep, 400.0, 6_000.0);
    });
    song.section("verse", 16.bars(), |s| {
        s.set(&sweep, 20_000.0);
        s.play(&kick, &four);
        s.play(&drums, &beat);
        s.play(&bass, &acid);
        s.play(&pad, &pads);
        s.play(&harp, &arps);
        s.during(8.bars()..16.bars(), &piano, &piano_part);
    });
    song.section("build", 8.bars(), |s| {
        s.play(&pad, &stabs);
        s.play(&harp, arps.fast(2));
        s.play(&drums, p("<[sd*4] [sd*8] [sd*16] [sd*16]>").vels("0.4 0.6 0.8 1"));
        s.play(&lead, melody.gain(0.6));
        s.ramp_exp(&sweep, 900.0, 20_000.0);
        s.ramp_exp(&lead_cut, 600.0, 5_000.0);
        s.ramp(&wheel, 0.0, 1.0);
        s.ramp_in(&scoop, 7.bars()..8.bars(), -12.0, 0.0, Curve::Smooth); // octave scoop into the drop
        s.ramp_in(&crush, 6.bars()..8.bars(), 0.0, 0.8, Curve::Linear);
        s.marker("riser-peak", 7.bars());
    });
    song.section("drop", 16.bars(), |s| {
        s.set(&crush, 0.0);
        s.set(&lead_cut, 7_000.0);
        s.set(&wheel, 0.25);
        s.play(&kick, &four);
        s.play(&drums, &beat & p("~ ~ ~ ~ ~ ~ ~ cr").slow(4));
        s.play(&bass, &acid);
        s.play(&sub, &sub_line);
        s.play(&pad, &pads);
        s.play(&lead, &melody);
        s.play(&bells, &sparkle);
        s.play(&harp, &arps);
    });
    song.section("breakdown", 8.bars(), |s| {
        s.play(&pad, pads.open());
        s.play(&piano, &piano_part);
        s.play(&bells, sparkle.slow(2));
        s.ramp_exp(&sweep, 20_000.0, 1_200.0);
    });
    song.section("outro", 8.bars(), |s| {
        s.set(&sweep, 20_000.0);
        s.during(0.bars()..4.bars(), &kick, &four);
        s.play(&drums, p("hh*8").vels("0.5 0.25"));
        s.play(&pad, &pads);
        s.once(&piano, chords[0].pat().vel(0.6).slow(4));
    });
    // fade the music bus out over the last 4 bars
    let outro_start = song.locate("outro").unwrap();
    music.volume_db(auto![outro_start + 4.bars() => 0.0, outro_start + 8.bars() => -40.0]);

    song.run();
}
