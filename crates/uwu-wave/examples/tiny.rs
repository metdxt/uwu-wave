//! The README's first example.
use uwu_wave::prelude::*;

fn main() {
    let song = Song::new("tiny").bpm(124);
    let key = Key::new(A3, Scale::Minor);

    let verb  = song.bus("verb").fx(Reverb::hall().wet());
    let kick  = song.track("kick", DrumMachine::tr909());
    let drums = song.track("drums", DrumMachine::tr808()).gain(-8.db());
    let bass  = song.track("bass", Synth::acid().cutoff(lfo::sine(2.bars()).range(300, 2400)))
        .fx(Ducker::new("kick").amount(0.6));
    let pad   = song.track("pad", Synth::pad()).send(&verb, -6.db()).gain(-10.db());
    song.master().fx(Limiter::master());

    let chords = voice_lead(key.oct(1).prog("i VI III VII"));

    song.section("drop", 8.bars(), |s| {
        s.play(&kick, "bd*4");
        s.play(&drums, drums! { cp: "....x.......x...", hh: "x.x.x.xxx.x.x.x." });
        s.play(&bass, n("0 0 7 0 3 0 10 5").scale(key.oct(-1)).fast(2));
        s.play(&pad, Pattern::from(chords.clone()));
    });

    song.run(); // CLI: render / --play / --info / --stems / --midi ...
}
