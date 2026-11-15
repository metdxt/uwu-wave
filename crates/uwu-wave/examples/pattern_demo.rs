//! pattern_demo — mini-notation + music theory in a short D-minor house tune.
//!
//! `cargo run --example pattern_demo` → writes `out/pattern_demo.wav`.
//!
//! Everything musical here is written with patterns: drum grids, euclidean rhythms, scale
//! degrees, roman-numeral progressions with voice leading, arpeggios, swing and humanize.

use uwu_wave::prelude::*;

fn main() {
    let song = Song::new("pattern_demo").bpm(122);
    let key: Key = "D3 minor".into();

    // ---- mixer
    let verb = song.bus("verb").fx(Reverb::hall().mix(1.0)).gain(-3.db());
    song.master().fx(Limiter::master());
    let drums = song.track("drums", DrumMachine::tr909()).gain(-6.db());
    let bass = song.track("bass", Synth::bass()).gain(-6.db());
    let pad = song
        .track("pad", Synth::pad())
        .gain(-11.db())
        .send(&verb, -6.db());
    let arp = song
        .track("arp", Pluck::harp())
        .gain(-9.db())
        .pan(0.35)
        .send(&verb, -8.db());
    let lead = song
        .track("lead", Fm::epiano())
        .gain(-7.db())
        .pan(-0.2)
        .send(&verb, -10.db());

    // ---- harmony: i VI III VII in D minor (Dm Bb F C), voice-led around D4
    let chords = voice_lead(key.oct(1).prog("i VI III VII"));
    let pads = Pattern::from(chords.clone()).legato(0.97).vel(0.6);

    // ---- drums
    let beat = drums! {
        bd: "X... x... X... x...",
        cp: ".... x... .... x...",
        oh: "..x. ..x. ..x. ..x.",
    };
    let hats = p("hh*16")
        .vels("0.9 0.35 0.6 0.35")
        .swing16(0.35)
        .humanize(0.01, 0.08)
        .degrade_by(0.08);
    let fill = p("~ ~ ~ [sd sd sd [sd ht mt lt]]^0.7");
    let groove = (beat.clone() & hats.clone()).last_of(4, |p| p & fill.clone());
    let perc = p("rim(3,8,2) ~, sh*8?0.3").gain(0.7);

    // ---- bass: chord roots as scale degrees, re-rhythmed, the odd octave jump
    let bassline = n("<0 5 2 6>")
        .scale(key.oct(-1))
        .struct_("x ~ x x ~ x ~ x")
        .legato(0.8)
        .rarely(|p| p.oct(1));

    // ---- arp: the same progression arpeggiated in 16ths
    let arps = Pattern::from(chords.clone())
        .oct(1)
        .arp("updown", 16)
        .vel(0.55)
        .sometimes(|p| p.gain(0.6));

    // ---- lead: two-bar degree melody, reversed every 4th cycle, with an octave echo
    let melody = n("<[0 2 4 ~ 3 2 ~ 1] [4 ~ 6 5 4@2 2 ~]>")
        .scale(key.oct(1))
        .every(4, |p| p.rev())
        .legato(0.9)
        .off(0.125, |p| p.up(12).gain(0.35));
    let hook = pat![~ ~ <[a4 c5] [bb4 d5] [c5 f5] [c5 e5]> ~];

    // ---- arrangement
    song.section("intro", 4.bars(), |s| {
        s.play(&pad, pads.clone());
        s.play(&drums, hats.clone().mask("<0 0 1 1>"));
        s.play(&arp, arps.clone().gain(0.7));
    });
    song.section("groove", 8.bars(), |s| {
        s.play(&drums, groove.clone() & perc.clone());
        s.play(&bass, bassline.clone());
        s.play(&pad, pads.clone());
        s.play(&arp, arps.clone());
        s.play(&lead, melody.clone() & hook.clone());
    });
    song.section("break", 4.bars(), |s| {
        s.play(&pad, pads.clone().open());
        s.play(&lead, melody.clone().palindrome().slow(2).staccato());
        s.play(&drums, p("bd ~ ~ ~, ~ cp").every(2, |p| p.ply(2)));
    });
    song.section("outro", 4.bars(), |s| {
        s.play(&drums, beat.clone() & hats.clone());
        s.play(&bass, bassline.clone());
        s.play(&pad, Pattern::from(chords.clone()).strum(eighth() * 0.25));
    });

    song.run();
}
