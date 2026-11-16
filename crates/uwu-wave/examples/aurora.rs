//! # Aurora — a full-orchestra piece in uwu-wave
//!
//! A short symphonic cue for the sampled orchestra: strings, woodwinds, brass, percussion,
//! harp, celesta and choir, seated on a virtual stage inside a convolution hall.
//!
//! It shows off:
//! * `Orchestra::*` articulated instruments (legato engine, CC1 dynamics crossfades)
//! * articulations in mini-notation (`d5.stacc`, `[..].pizz`, `( slurs )`, `^ff`)
//! * hairpins written by sections (`s.cresc`, `s.hairpin`)
//! * automatic 4-part voicing of a progression (`orchestrate4`)
//! * `Stage` seating + `Convolver` hall + orchestral master chain
//!
//! ```text
//! cargo run --release --example aurora              # → out/Aurora.wav
//! cargo run --release --example aurora -- --play
//! cargo run --release --example aurora -- --info
//! ```

use uwu_wave::prelude::*;

fn main() {
    let song = Song::new("Aurora").bpm(auto![0 => 66, bar(20) => 66, bar(24) => 72 => smooth, bar(36) => 72, bar(40) => 60 => smooth]).meter(4, 4);
    let key = Key::new(D3, Scale::Minor);

    // ───────────────────────── the room ─────────────────────────
    let hall = song.bus("hall").fx(Convolver::hall().wet().low_cut(120.0)).gain(-7.db());
    song.master()
        .fx(Eq::orchestral_air())
        .fx(Compressor::orchestral())
        .fx(Limiter::new().ceiling(-1.db()));

    // every section: instrument, its seat on stage, a send into the hall
    // (`.gain` trims below are mix balance: soloists up, accompaniment down)
    let seat = |name: &str, inst: Articulated, stage: &str, send: Db| {
        song.track(name, inst).fx(Stage::at(stage)).send(&hall, send)
    };
    let vln1 = seat("violins 1", Orchestra::violins_1(), "violins-1", -8.db());
    let vln2 = seat("violins 2", Orchestra::violins_2(), "violins-2", -8.db());
    let vla = seat("violas", Orchestra::violas(), "violas", -8.db());
    let vc = seat("cellos", Orchestra::cellos(), "cellos", -9.db());
    let cb = seat("basses", Orchestra::basses(), "basses", -10.db());
    let fl = seat("flute", Orchestra::flute(), "flute", -6.db()).gain(7.db());
    let ob = seat("oboe", Orchestra::oboe(), "oboe", -6.db()).gain(4.db());
    let cl = seat("clarinet", Orchestra::clarinet(), "clarinet", -6.db()).gain(10.db());
    let bn = seat("bassoon", Orchestra::bassoon(), "bassoon", -9.db()).gain(1.db());
    let hn = seat("horns", Orchestra::horns(), "horns", -5.db()).gain(4.db());
    let tpt = seat("trumpet", Orchestra::trumpet(), "trumpet", -6.db()).gain(6.db());
    let tbn = seat("trombones", Orchestra::trombones(), "trombone", -6.db()).gain(4.db());
    let tba = seat("tuba", Orchestra::tuba(), "tuba", -7.db()).gain(5.db());
    let timp = seat("timpani", Orchestra::timpani(), "timpani", -6.db());
    let cym = seat("cymbals", Orchestra::cymbals(), "percussion", -4.db()).gain(6.db());
    let harp = seat("harp", Orchestra::harp(), "harp", -5.db()).gain((-4).db());
    let cel = seat("celesta", Orchestra::celesta(), "celesta", -4.db()).gain((-4).db());
    let choir = seat("choir", Orchestra::choir(), "choir", -4.db());

    // ───────────────────────── material ─────────────────────────
    // i – VI – III – VII | iv – i – V – V   (Dm Bb F C | Gm Dm A A)
    let chords = voice_lead(key.oct(1).prog("i VI III VII iv i V V"));
    let [hi, mid, low, bass] = orchestrate4(&chords);
    let pad_hi = hi.legato(1.02);
    let pad_mid = mid.legato(1.02);
    let pad_low = low.legato(1.02);
    let roots = bass.legato(1.02);

    // the theme: a slurred, rising minor melody (scale degrees), two bars per phrase
    let theme = n("<[(0 1 2) 4@2 ~] [(5 4 3) 2@2 ~] [(2 4 5) 7@3] [(6 5 4) 3 4@2] \
                   [(3 4 5) 6@2 ~] [(7 6 5) 4@2 ~] [(4 3 1) 0@2 ~] [~ ~ ~ ~]>")
        .scale(key.oct(1));
    let countermelody = n("<[~ 4 3 2] [~ 2 1 0] [~ 5 4 2] [~ 1 2 3] [~ 3 2 0] [~ 4 2 1] [~ -1 0 1] [~ 1 ~ ~]>")
        .scale(key)
        .slur();

    // rhythmic strings for the climax: driving staccato eighths on chord tones
    let ostinato = mid.struct_("x x x x x x x x").stacc().vels("1 .6 .8 .6 .9 .6 .8 .6");
    let low_pulse = bass.struct_("x ~ x x ~ x x ~").marcato();
    let pizz = low.struct_("x ~ ~ x ~ ~ x ~").pizz();

    // harp & celesta sparkle: arpeggiated chords
    let harp_arp = chords.pat().arp("up", 8).vel(0.6);
    let bells = chords.pat().oct(1).arp("updown", 16).vel(0.45).sometimes(|p| p.gain(0.5));

    // percussion
    let timp_rolls = p("<d2 ~ ~ ~ ~ ~ a1 [a1 a1 a1 a1]>").vel(Dyn::MF);
    let timp_hits = p("d2.acc ~ ~ [a1 a1] d2.acc ~ a1 ~").vel(Dyn::F);
    let crash = p("<c4 ~ ~ ~ ~ ~ ~ ~>").vel(Dyn::F);

    // brass chorale
    let chorale_hi = hi.oct(-1).fit(Range::TRUMPET).tenuto();
    let chorale_mid = mid.oct(-1).fit(Range::HORN).tenuto();
    let chorale_low = low.oct(-1).fit(Range::TROMBONE).tenuto();
    let chorale_bass = bass.oct(-1).fit(Range::TUBA).tenuto();

    // ───────────────────────── arrangement ─────────────────────────
    song.section("dawn", 8.bars(), |s| {
        // strings alone, from nothing
        s.play(&vln2, &pad_hi).play(&vla, &pad_mid).play(&vc, &pad_low).play(&cb, &roots);
        for t in [&vln2, &vla, &vc, &cb] {
            s.hairpin(t, 0.bars()..6.bars(), Dyn::PPP, Dyn::MP);
            s.hairpin(t, 6.bars()..8.bars(), Dyn::MP, Dyn::P);
        }
        s.during(4.bars()..8.bars(), &harp, &harp_arp);
        s.during(2.bars()..8.bars(), &cl, &countermelody);
        s.dynamics(&cl, Dyn::P);
    });

    song.section("theme", 8.bars(), |s| {
        s.play(&ob, &theme);
        s.cresc(&ob, Dyn::MP, Dyn::MF);
        s.play(&vln2, &pad_hi).play(&vla, &pad_mid).play(&vc, &pad_low).play(&cb, &pizz);
        for t in [&vln2, &vla, &vc] {
            s.dynamics(t, Dyn::P);
        }
        s.dynamics(&cb, Dyn::MF);
        s.play(&bn, &countermelody); // background countermelody: stays under the oboe
        s.dynamics(&bn, Dyn::P);
        s.play(&harp, &harp_arp);
        s.play(&cel, &bells);
        s.play(&timp, &timp_rolls);
    });

    song.section("rising", 8.bars(), |s| {
        // the theme passes to the violins, flute doubles an octave up; everything swells
        s.play(&vln1, &theme);
        s.play(&fl, theme.up(12));
        s.play(&vln2, &ostinato).play(&vla, ostinato.oct(-1)).play(&vc, &pad_low).play(&cb, &roots);
        s.play(&hn, &chorale_mid);
        s.play(&choir, &pad_mid);
        for t in [&vln1, &vln2, &vla, &vc, &cb, &hn, &choir, &fl] {
            s.cresc(t, Dyn::MP, Dyn::FF);
        }
        s.play(&timp, &timp_rolls);
        s.at(7.bars(), &cym, "c4^ff");
    });

    song.section("glory", 8.bars(), |s| {
        // tutti: brass chorale, theme in octaves, driving strings, timpani
        s.play(&vln1, theme.up(12)).play(&fl, theme.up(12)).play(&ob, &theme);
        s.play(&vln2, &ostinato).play(&vla, ostinato.oct(-1)).play(&vc, &low_pulse).play(&cb, &low_pulse);
        s.play(&tpt, &chorale_hi).play(&hn, &chorale_mid).play(&tbn, &chorale_low).play(&tba, &chorale_bass);
        s.play(&choir, &pad_hi);
        s.play(&timp, &timp_hits);
        s.play(&cym, &crash);
        s.play(&harp, &harp_arp);
        for t in [&vln1, &vln2, &vla, &vc, &cb, &tpt, &hn, &tbn, &tba, &choir, &fl, &ob] {
            s.dynamics(t, Dyn::FF);
        }
        s.hairpin(&choir, 6.bars()..8.bars(), Dyn::FF, Dyn::MF);
    });

    song.section("dusk", 8.bars(), |s| {
        // a last, quiet statement: solo clarinet over high strings, celesta, a final chord
        s.play(&cl, theme.slow(1).down(12));
        s.dynamics(&cl, Dyn::P);
        s.play(&vln1, &pad_hi).play(&vln2, &pad_mid).play(&vla, &pad_low);
        for t in [&vln1, &vln2, &vla] {
            s.dim(t, Dyn::MP, Dyn::PPP);
        }
        s.during(0.bars()..4.bars(), &cel, &bells);
        s.at(6.bars(), &cb, Clip::new(2.bars()).note(0, 8, D2, 0.6));
        s.at(6.bars(), &vc, Clip::new(2.bars()).note(0, 8, D3, 0.6).note(0, 8, A3, 0.6));
        s.hairpin(&cb, 6.bars()..8.bars(), Dyn::P, Dyn::PPP);
        s.hairpin(&vc, 6.bars()..8.bars(), Dyn::P, Dyn::PPP);
        s.at(6.bars(), &timp, "d2^pp");
    });

    song.run();
}
