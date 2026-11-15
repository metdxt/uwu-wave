//! Compile-checks the API snippets used in the README.
use uwu_wave::prelude::*;

#[test]
fn readme_snippets_compile_and_render() {
    let song = Song::new("readme").bpm(auto![0 => 118, bar(4) => 124 => smooth]);
    let _a = -6.db();
    let _b = (440.hz(), 2.khz(), 120.ms(), 50.pct(), 7.st());
    let _t = (bar(4), bars(0..8), 2.bars(), 3.beats(), quarter(), dotted(eighth()), triplet(quarter()), note(1, 16));
    let cutoff = Lane::new(20_000.0);
    let _p: Vec<Param> = vec![
        auto![0 => 200.0, bar(8) => 8000.0 => exp].into(),
        lfo::sine(2.bars()).range(300.0, 3000.0).into(),
        lfo::tri(1.bars()) * 400.0 + 1200.0,
        (&cutoff).into(),
        auto![0 => 0.2, 0.25 => 1.0 => exp, 1 => 1.0].looped(1.beats()).into(),
    ];
    let verb = song.bus("verb").fx(Reverb::plate().wet());
    let drums = song.bus("drum bus").fx(Compressor::punch());
    let kick = song.track("kick", DrumMachine::tr909()).to(&drums);
    let lead = song.track("lead", Synth::lead()).send(&verb, -9.db()).send_pre(&verb, -20.db()).pan(-0.3).gain(-4.db());
    let bass = song.track("bass", Synth::bass()).fx(Compressor::new().sidechain("kick")).transpose(-12).velocity(0.8);
    let mine = song.track("x", Synth::saw()).fx(FnFx::new(|f: Frame, _ctx: &Ctx| f.map(|x| (x * 3.0).tanh())));
    let music = song.bus("music").fx(LowPass::new(&cutoff));
    music.volume_db(auto![bar(6) => 0, bar(8) => -40]);
    song.master().fx(Eq::new().low_cut(30.0)).fx(Limiter::master());

    let key = Key::new(A3, Scale::Minor);
    let _c = (C4.maj7(), A3.min().inv(1), Chord::parse("Bb/D"), key.degree(9));
    let _pr = prog("ii7 V7 Imaj7", "C major");
    let chords = voice_lead(key.prog("i VI III VII"));
    let beat = drums! { bd: "X... x... X... x...", sd: ".... x... .... x...", hh: "x.x.x.x.x.x.x.x." };
    let hook = p("c4 e4 g4").fast(2).rev().every(4, |p| p.up(12)).swing16(0.2).humanize(0.01, 0.1);
    let degs = n("0 2 4 <6 7>").scale(Key::parse("D dorian").unwrap());
    let arps = Pattern::from(chords.clone()).arp("updown", 16).legato(0.8);
    let _m = pat![c4 e4 [g4 b4]*2 ~];
    let _ops = hook.clone() & degs.clone();
    let _ops2 = hook.clone() + degs.clone();
    let _ops3 = hook.clone() * 4;

    song.section("verse", 4.bars(), |s| {
        s.play(&kick, &beat);
        s.once(&lead, &hook);
        s.at(1.bars(), &mine, "cr");
        s.during(2.bars()..4.bars(), &lead, &degs);
        s.play(&bass, "c2 ~ c2 eb2");
        s.ramp(&cutoff, 400.0, 20_000.0);
        s.set_at(3.bars(), &cutoff, 5000.0);
        s.marker("hook", 2.bars());
    });
    song.section("drop", 2.bars(), |s| {
        s.ramp_exp(&cutoff, 400.0, 20_000.0);
        s.play(&lead, &arps);
    });
    lead.play(bar(6), Clip::new(4.beats()).note(0, 1, C4, 0.8));
    let r = song.render_with(&RenderOptions { no_tail: true, ..Default::default() }).unwrap();
    assert!(r.report.master_peak_db < 0.0 && r.report.master_peak_db > -40.0);
    assert_eq!(song.locate("hook"), Some(bar(2)));
}
