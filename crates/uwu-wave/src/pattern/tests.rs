use super::*;
use crate::theory::notes::*;
use crate::theory::{Key, Scale};

fn ev(p: &Pattern, from: f64, to: f64) -> Vec<(f64, f64, f32)> {
    p.events(from, to)
        .iter()
        .map(|e| {
            (
                (e.start * 1e6).round() / 1e6,
                (e.dur * 1e6).round() / 1e6,
                e.pitch,
            )
        })
        .collect()
}
fn pitches(p: &Pattern, from: f64, to: f64) -> Vec<f32> {
    p.events(from, to).iter().map(|e| e.pitch).collect()
}

#[test]
fn sequence_and_rests() {
    let a = p("c4 e4 ~ g4");
    assert_eq!(
        ev(&a, 0.0, 1.0),
        vec![(0.0, 0.25, 60.0), (0.25, 0.25, 64.0), (0.75, 0.25, 67.0)]
    );
    assert_eq!(a.cycles(), 1.0);
    assert_eq!(pitches(&p("c4 - d4"), 0.0, 1.0), vec![60.0, 62.0]);
    // queries beyond the first cycle repeat
    assert_eq!(
        ev(&a, 1.0, 1.5),
        vec![(1.0, 0.25, 60.0), (1.25, 0.25, 64.0)]
    );
}

#[test]
fn subdivision_alternation_speed() {
    let a = p("c4 [e4 g4]");
    assert_eq!(
        ev(&a, 0.0, 1.0),
        vec![(0.0, 0.5, 60.0), (0.5, 0.25, 64.0), (0.75, 0.25, 67.0)]
    );
    let b = p("c4 <e4 g4 b4>");
    assert_eq!(b.cycles(), 3.0);
    assert_eq!(
        pitches(&b, 0.0, 4.0),
        vec![60.0, 64.0, 60.0, 67.0, 60.0, 71.0, 60.0, 64.0]
    );
    let nested = p("<a4 <b4 c5>>");
    assert_eq!(nested.cycles(), 4.0);
    assert_eq!(pitches(&nested, 0.0, 4.0), vec![69.0, 71.0, 69.0, 72.0]);
    assert_eq!(
        ev(&p("c4*2 d4"), 0.0, 1.0),
        vec![(0.0, 0.25, 60.0), (0.25, 0.25, 60.0), (0.5, 0.5, 62.0)]
    );
    let slow = p("[c4 d4]/2");
    assert_eq!(slow.cycles(), 2.0);
    assert_eq!(
        ev(&slow, 0.0, 2.0),
        vec![(0.0, 1.0, 60.0), (1.0, 1.0, 62.0)]
    );
    let alt_fast = p("<c4 d4>*2");
    assert_eq!(pitches(&alt_fast, 0.0, 1.0), vec![60.0, 62.0]);
    let ab = p("<c4 d4 e4>*2");
    assert_eq!(ab.cycles(), 3.0);
}

#[test]
fn replicate_elongate_tie() {
    assert_eq!(
        pitches(&p("c4!3 d4"), 0.0, 1.0),
        vec![60.0, 60.0, 60.0, 62.0]
    );
    assert_eq!(pitches(&p("c4 ! d4"), 0.0, 1.0), vec![60.0, 60.0, 62.0]);
    assert_eq!(
        ev(&p("c4@3 d4"), 0.0, 1.0),
        vec![(0.0, 0.75, 60.0), (0.75, 0.25, 62.0)]
    );
    assert_eq!(
        ev(&p("c4 _ _ d4"), 0.0, 1.0),
        vec![(0.0, 0.75, 60.0), (0.75, 0.25, 62.0)]
    );
}

#[test]
fn degrade_is_deterministic_and_varies() {
    let a = p("hh*16?");
    let n0 = a.events(0.0, 1.0).len();
    assert!(n0 > 2 && n0 < 14, "{n0}");
    assert_eq!(ev(&a, 0.0, 4.0), ev(&p("hh*16?"), 0.0, 4.0));
    assert_ne!(
        ev(&a, 0.0, 1.0).iter().map(|e| e.0).collect::<Vec<_>>(),
        ev(&a, 1.0, 2.0)
            .iter()
            .map(|e| e.0 - 1.0)
            .collect::<Vec<_>>()
    );
    let b = p("hh*16?0.9");
    assert!(b.events(0.0, 4.0).len() < 20);
}

#[test]
fn stacks_chords_drums_numbers() {
    assert_eq!(pitches(&p("[c4,e4,g4]"), 0.0, 1.0), vec![60.0, 64.0, 67.0]);
    assert_eq!(pitches(&p("c4 d4, c3"), 0.0, 1.0), vec![48.0, 60.0, 62.0]);
    assert_eq!(pitches(&p("Cmaj7"), 0.0, 1.0), vec![60.0, 64.0, 67.0, 71.0]);
    assert_eq!(pitches(&p("c3'm7"), 0.0, 1.0), vec![48.0, 51.0, 55.0, 58.0]);
    assert_eq!(pitches(&p("g2'7"), 0.0, 1.0), vec![43.0, 47.0, 50.0, 53.0]);
    assert_eq!(pitches(&p("Bb/D"), 0.0, 1.0), vec![62.0, 70.0, 77.0]);
    assert_eq!(
        pitches(&p("F#m7b5"), 0.0, 1.0),
        vec![66.0, 69.0, 72.0, 76.0]
    );
    assert_eq!(pitches(&p("G7"), 0.0, 1.0), vec![103.0]); // a note, not a chord
    assert_eq!(
        pitches(
            &p("bd sd hh oh cp rim lt mt ht cr rd cb sh kick snare clap ph"),
            0.0,
            1.0
        )
        .len(),
        17
    );
    assert_eq!(
        pitches(&p("bd sd ch oh"), 0.0, 1.0),
        vec![36.0, 38.0, 42.0, 46.0]
    );
    assert_eq!(
        pitches(&p("60 61.5 C#4 eb3 cs4 bb2 a"), 0.0, 1.0),
        vec![60.0, 61.5, 61.0, 51.0, 61.0, 46.0, 69.0]
    );
}

#[test]
fn velocity_suffix() {
    let a = p("c4^0.3 [d4 e4]^1");
    let v: Vec<f32> = a.first_cycle().iter().map(|e| e.vel).collect();
    assert_eq!(v, vec![0.3, 1.0, 1.0]);
    assert_eq!(p("c4").first_cycle()[0].vel, DEFAULT_VEL);
}

#[test]
fn euclid() {
    let a = p("bd(3,8)");
    assert_eq!(
        a.first_cycle()
            .iter()
            .map(|e| e.start * 8.0)
            .collect::<Vec<_>>(),
        vec![0.0, 3.0, 6.0]
    );
    let b = p("bd(3,8,2)");
    assert_eq!(
        b.first_cycle()
            .iter()
            .map(|e| e.start * 8.0)
            .collect::<Vec<_>>(),
        vec![1.0, 4.0, 6.0]
    );
    let c = p("bd(5,8)");
    assert_eq!(
        c.first_cycle()
            .iter()
            .map(|e| e.start * 8.0)
            .collect::<Vec<_>>(),
        vec![0.0, 2.0, 3.0, 5.0, 6.0]
    );
}

#[test]
fn polymeter() {
    let a = p("{c4 d4 e4, f4 g4}");
    let evs = a.events(0.0, 1.0);
    assert_eq!(evs.len(), 6);
    assert_eq!(a.cycles(), 2.0);
    let b = p("{c4 d4 e4}%4");
    assert_eq!(b.events(0.0, 1.0).len(), 4);
}

#[test]
fn errors_have_columns() {
    let e = Pattern::parse("c4 e4 x4").unwrap_err();
    assert_eq!(e.col, 7);
    assert_eq!(e.len, 2);
    let s = e.to_string();
    assert!(s.contains("column 7") && s.contains("\n        ^^"), "{s}");
    let e = Pattern::parse("c4 [e4 g4").unwrap_err();
    assert_eq!(e.col, 4);
    assert!(Pattern::parse("c4 ]").is_err());
    assert!(Pattern::parse("bd(3)").is_err());
    assert!(Pattern::parse("c4*").is_err());
    assert!(Pattern::parse("").is_err());
    assert!(Pattern::parse("_ c4").is_err());
    assert!(Pattern::parse("c4^2").is_err());
    let e = Pattern::parse("Cblah").unwrap_err();
    assert!(e.message.contains("quality"), "{}", e.message);
}

#[test]
#[should_panic(expected = "column 4")]
fn p_panics_with_caret() {
    let _ = p("c4 %");
}

#[test]
fn macros() {
    let a = pat![c4 e4 [g4 b4]*2 ~];
    assert_eq!(
        pitches(&a, 0.0, 1.0),
        vec![60.0, 64.0, 67.0, 71.0, 67.0, 71.0]
    );
    let b = crate::pat![bd(3,8) sd?0.0 eb3^0.5 Cmaj7 <a4 b4>/2 c4@3 _ -];
    assert_eq!(b.cycles(), 4.0);
}

#[test]
#[should_panic]
fn pat_macro_reports_errors() {
    let _ = pat![c4 x9];
}

#[test]
fn seq_and_conversions() {
    let a = crate::seq![C4, E4, "g4 b4", 72];
    assert_eq!(ev(&a, 0.0, 1.0).len(), 5);
    assert_eq!(a.first_cycle()[2].start, 0.5);
    let c: Pattern = C4.maj().into();
    assert_eq!(pitches(&c, 0.0, 1.0), vec![60.0, 64.0, 67.0]);
    let pr: Pattern = Key::major(C4).prog("I IV").into();
    assert_eq!(pr.cycles(), 2.0);
    assert_eq!(pitches(&pr, 1.0, 2.0), vec![65.0, 69.0, 72.0]);
    let v: Pattern = vec![C4, D4].into();
    assert_eq!(v.first_cycle().len(), 2);
}

#[test]
fn drums_and_grid() {
    let d = crate::drums! { bd: "x . . . x . . .", sd: ". . X . . . X o", hh: "x x x x x x x x", 49: "x......." };
    let e = d.first_cycle();
    assert_eq!(e.len(), 14);
    let sd: Vec<_> = e
        .iter()
        .filter(|e| e.pitch == 38.0)
        .map(|e| (e.start * 8.0, e.vel))
        .collect();
    assert_eq!(sd, vec![(2.0, 1.0), (6.0, 1.0), (7.0, grid::GHOST_VEL)]);
    let g = grid(C2, "x.x_ | x...");
    assert_eq!(g.cycles(), 2.0);
    assert_eq!(
        ev(&g, 0.0, 2.0),
        vec![(0.0, 0.25, 36.0), (0.5, 0.5, 36.0), (1.0, 0.25, 36.0)]
    );
    assert!(std::panic::catch_unwind(|| grid("bd", "x.y.")).is_err());
}

#[test]
fn combinators_time() {
    let a = p("c4 d4 e4 f4");
    assert_eq!(
        pitches(&a.clone().rev(), 0.0, 1.0),
        vec![65.0, 64.0, 62.0, 60.0]
    );
    assert_eq!(pitches(&a.clone().fast(2), 0.0, 1.0).len(), 8);
    assert_eq!(a.clone().fast(2).cycles(), 0.5);
    assert_eq!(a.clone().slow(2).cycles(), 2.0);
    assert_eq!(
        pitches(&a.clone().every(2, |p| p.rev()), 0.0, 2.0),
        vec![65.0, 64.0, 62.0, 60.0, 60.0, 62.0, 64.0, 65.0]
    );
    assert_eq!(a.clone().every(3, |p| p.rev()).cycles(), 3.0);
    assert_eq!(
        pitches(&a.clone().last_of(2, |p| p.up(12)), 1.0, 2.0)[0],
        72.0
    );
    assert_eq!(
        pitches(&a.clone().palindrome(), 1.0, 2.0),
        vec![65.0, 64.0, 62.0, 60.0]
    );
    assert_eq!(
        pitches(&a.clone().iter(4), 1.0, 2.0),
        vec![62.0, 64.0, 65.0, 60.0]
    );
    assert_eq!(
        pitches(&p("c4 d4").ply(2), 0.0, 1.0),
        vec![60.0, 60.0, 62.0, 62.0]
    );
    assert_eq!(
        ev(&p("c4 d4").late(0.25), 0.0, 1.0),
        vec![(0.25, 0.5, 60.0), (0.75, 0.5, 62.0)]
    );
    assert_eq!(
        ev(&p("c4 d4").early(0.25), 0.0, 1.0),
        vec![(0.25, 0.5, 62.0), (0.75, 0.5, 60.0)]
    );
    let cat = p("c4") + p("<d4 e4>");
    assert_eq!(cat.cycles(), 3.0);
    assert_eq!(
        pitches(&cat, 0.0, 6.0),
        vec![60.0, 62.0, 64.0, 60.0, 62.0, 64.0]
    );
    let st = p("c4") & "e4";
    assert_eq!(pitches(&st, 0.0, 1.0), vec![60.0, 64.0]);
    assert_eq!((p("c4") * 4).cycles(), 4.0);
}

#[test]
fn combinators_values() {
    let a = p("c4 e4");
    assert_eq!(pitches(&a.clone().up(2), 0.0, 1.0), vec![62.0, 66.0]);
    assert_eq!(pitches(&a.clone().oct(-1), 0.0, 1.0), vec![48.0, 52.0]);
    assert_eq!(a.clone().vel(0.5).first_cycle()[0].vel, 0.5);
    assert!((a.clone().gain(0.5).first_cycle()[0].vel - 0.4).abs() < 1e-6);
    assert_eq!(a.clone().legato(0.5).first_cycle()[0].dur, 0.25);
    let s = n("0 2 4 -1 7").scale(Key::new(C4, Scale::Minor));
    assert_eq!(pitches(&s, 0.0, 1.0), vec![60.0, 63.0, 67.0, 58.0, 72.0]);
    let s2 = n("0 1").scale("D3 dorian");
    assert_eq!(pitches(&s2, 0.0, 1.0), vec![50.0, 52.0]);
    assert_eq!(
        pitches(&p("e4 f#4").quantize("C major"), 0.0, 1.0),
        vec![64.0, 65.0]
    );
    assert_eq!(
        pitches(&p("c3").chord("m7"), 0.0, 1.0),
        vec![48.0, 51.0, 55.0, 58.0]
    );
    assert_eq!(
        pitches(&n("1").triads("C3 major"), 0.0, 1.0),
        vec![50.0, 53.0, 57.0]
    );
    assert_eq!(
        pitches(&p("Cmaj7").drop2(), 0.0, 1.0),
        vec![55.0, 60.0, 64.0, 71.0]
    );
    assert_eq!(pitches(&p("Cmaj").inv(1), 0.0, 1.0), vec![64.0, 67.0, 72.0]);
    let v = p("hh*4").vels("1 .5");
    assert_eq!(
        v.first_cycle().iter().map(|e| e.vel).collect::<Vec<_>>(),
        vec![1.0, 1.0, 0.5, 0.5]
    );
}

#[test]
fn arp_modes() {
    let c = p("c4'");
    assert_eq!(
        pitches(&c.clone().arp("up", 4), 0.0, 1.0),
        vec![60.0, 64.0, 67.0, 60.0]
    );
    assert_eq!(
        pitches(&c.clone().arp(ArpMode::Down, 3), 0.0, 1.0),
        vec![67.0, 64.0, 60.0]
    );
    assert_eq!(
        pitches(&c.clone().arp("updown", 6), 0.0, 1.0),
        vec![60.0, 64.0, 67.0, 64.0, 60.0, 64.0]
    );
    assert_eq!(
        pitches(&p("Cmaj7").arp("converge", 4), 0.0, 1.0),
        vec![60.0, 71.0, 64.0, 67.0]
    );
    let r = p("Cmaj7").arp("random", 16);
    assert_eq!(r.events(0.0, 1.0).len(), 16);
    // arp spanning a slow chord: queried in the middle still continues
    let slow = p("Cmaj/2").arp("up", 4);
    assert_eq!(pitches(&slow, 1.0, 2.0), vec![64.0, 67.0, 60.0, 64.0]);
}

#[test]
fn structure_and_mask() {
    let a = p("<c3 eb3>").struct_("x ~ x x");
    assert_eq!(
        ev(&a, 0.0, 1.0),
        vec![(0.0, 0.25, 48.0), (0.5, 0.25, 48.0), (0.75, 0.25, 48.0)]
    );
    assert_eq!(pitches(&a, 1.0, 2.0), vec![51.0, 51.0, 51.0]);
    let m = p("c4 d4 e4 f4").mask("1 0 1 1");
    assert_eq!(pitches(&m, 0.0, 1.0), vec![60.0, 64.0, 65.0]);
    let e = p("c2").euclid(3, 8);
    assert_eq!(e.first_cycle().len(), 3);
}

#[test]
fn random_combinators() {
    let a = p("c4*16").sometimes(|p| p.up(12));
    let ev1 = a.events(0.0, 1.0);
    let hi = ev1.iter().filter(|e| e.pitch == 72.0).count();
    assert_eq!(ev1.len(), 16);
    assert!(hi > 2 && hi < 14, "{hi}");
    assert_eq!(
        pitches(&a, 0.0, 2.0),
        pitches(&p("c4*16").sometimes(|p| p.up(12)), 0.0, 2.0)
    );
    assert_ne!(
        pitches(&a, 0.0, 1.0),
        pitches(&p("c4*16").seed(7).sometimes(|p| p.up(12)), 0.0, 1.0)
    );
    let h = p("hh*8").humanize(0.05, 0.1);
    let e = h.events(0.0, 4.0);
    assert!((30..=34).contains(&e.len()), "{}", e.len());
    assert!(
        e.iter()
            .all(|e| e.start.is_finite() && e.vel > 0.0 && e.vel <= 1.0)
    );
    assert!(
        e.iter()
            .any(|e| (e.start * 8.0 - (e.start * 8.0).round()).abs() > 1e-4)
    );
}

#[test]
fn swing_moves_offbeats() {
    crate::time::set_meter(4, 4);
    let a = p("hh*8").swing(1.0);
    let s: Vec<f64> = a
        .first_cycle()
        .iter()
        .map(|e| (e.start * 4.0 * 1000.0).round() / 1000.0)
        .collect();
    // beats: 0, 0.667, 1, 1.667, ...
    assert_eq!(s, vec![0.0, 0.667, 1.0, 1.667, 2.0, 2.667, 3.0, 3.667]);
}

#[test]
fn strum_offsets_chord_notes() {
    let a = p("c4'").strum(0.1);
    let s: Vec<f64> = a
        .first_cycle()
        .iter()
        .map(|e| (e.start * 4.0 * 1000.0).round() / 1000.0)
        .collect();
    assert_eq!(s, vec![0.0, 0.1, 0.2]);
}

#[test]
fn seq_trait_beats_and_looping() {
    crate::time::set_meter(4, 4);
    let a = p("c4 <e4 g4>");
    assert_eq!(Seq::len(&a), 8.0);
    let n = a.notes(0.0, 16.0);
    assert_eq!(n.len(), 8);
    assert_eq!(n[1].start, 2.0);
    assert_eq!(n[1].dur, 2.0);
    assert_eq!(n[3].pitch, 67.0);
    let b = p("c4 d4").cycle(2.0);
    assert_eq!(Seq::len(&b), 2.0);
    assert_eq!(
        b.notes(0.0, 4.0)
            .iter()
            .map(|n| n.start)
            .collect::<Vec<_>>(),
        vec![0.0, 1.0, 2.0, 3.0]
    );
    // window queries partition exactly
    let c = p("hh*16?0.3 [c4 e4]*3");
    let whole = c.notes(0.0, 32.0).len();
    let parts: usize = (0..64)
        .map(|i| c.notes(i as f64 * 0.5, (i + 1) as f64 * 0.5).len())
        .sum();
    assert_eq!(whole, parts);
    let baked = a.bake();
    assert_eq!(baked.len, 8.0);
    assert_eq!(baked.notes.len(), 4);
}

#[test]
fn plays_on_a_song() {
    use crate::prelude::*;
    let song = Song::new("pat").bpm(120);
    let t = song.track("t", Silence);
    t.play(bars(0..4), "c4 e4 g4 [b4 c5]");
    song.section("a", 2.bars(), |s| {
        s.play(&t, "c2 ~ c2 <eb2 g1>");
        s.play(&t, String::from("bd*2"));
        s.play(&t, C4.maj7());
        s.play(&t, prog("I V", Key::major(C3)));
    });
    let notes = t.notes();
    // 4 bars * 5 + 2 bars * 3 + 2 * 2 + 2 * 4 + 2 * 3
    assert_eq!(notes.len(), 20 + 6 + 4 + 8 + 6);
}

#[test]
fn patterned_modifier_args() {
    let a = p("hh*<2 3>");
    assert_eq!(a.cycles(), 2.0);
    assert_eq!(a.events(0.0, 1.0).len(), 2);
    assert_eq!(a.events(1.0, 2.0).len(), 3);
    let b = p("hh*[2 4]");
    let s: Vec<f64> = b.first_cycle().iter().map(|e| e.start).collect();
    assert_eq!(s, vec![0.0, 0.5, 0.75]);
    let c = p("[c4 d4]/<1 2>");
    assert_eq!(pitches(&c, 0.0, 1.0), vec![60.0, 62.0]);
    assert_eq!(pitches(&c, 1.0, 2.0).len(), 1);
    let d = p("bd(<3 5>,8)");
    assert_eq!(d.events(0.0, 1.0).len(), 3);
    assert_eq!(d.events(1.0, 2.0).len(), 5);
    assert_eq!(d.cycles(), 2.0);
    let e = p("bd(3,8,<0 2>)");
    assert_eq!(
        e.events(1.0, 2.0)
            .iter()
            .map(|e| ((e.start - 1.0) * 8.0).round())
            .collect::<Vec<_>>(),
        vec![1.0, 4.0, 6.0]
    );
    assert_eq!(p("c4").fast_by("<1 2>").events(0.0, 2.0).len(), 3);
    assert!(Pattern::parse("bd(<3 5>,8").is_err());
}

#[test]
fn chop_stutter_echo() {
    assert_eq!(
        pitches(&p("c4 d4").chop(2), 0.0, 1.0),
        vec![60.0, 60.0, 62.0, 62.0]
    );
    let s = p("c4 ~ ~ ~").stutter(3, 0.125);
    assert_eq!(
        s.first_cycle().iter().map(|e| e.start).collect::<Vec<_>>(),
        vec![0.0, 0.125, 0.25]
    );
    let e = p("c4 ~ ~ ~").echo(3, 0.125, 0.5);
    let v: Vec<f32> = e.first_cycle().iter().map(|e| e.vel).collect();
    assert_eq!(v, vec![0.8, 0.4, 0.2]);
}

#[test]
fn borrowed_operators_and_conversions() {
    let beat = p("bd*4");
    let fill = p("~ ~ ~ sd");
    let a = &beat & &fill;
    let b = &beat & "~ ~ ~ cr";
    let c = &beat + &fill;
    let d = &beat * 2;
    let e = beat.fast(2).rev(); // no clones needed, beat still usable
    let f = beat.last_of(4, |p| p & &fill);
    assert_eq!(a.first_cycle().len(), 5);
    assert_eq!(b.first_cycle().len(), 5);
    assert_eq!(c.cycles(), 2.0);
    assert_eq!(d.cycles(), 2.0);
    assert_eq!(e.first_cycle().len(), 8);
    assert_eq!(f.events(3.0, 4.0).len(), 5);
    let chords = Key::major(C4).prog("I IV V");
    let g = Pattern::from(&chords) & C4 & &C4.maj7();
    assert_eq!(g.cycles(), 3.0);
    assert_eq!(chords.pat().cycles(), 3.0);
    assert_eq!("c4 e4".pat().first_cycle().len(), 2);
    assert_eq!(C4.maj7().pat().first_cycle().len(), 4);
    assert_eq!(C4.pat().first_cycle().len(), 1);
    assert_eq!(Pattern::from([C4, E4, G4]).first_cycle().len(), 3);
    let _: std::sync::Arc<dyn Seq> = (&chords).into_seq();
    let _: std::sync::Arc<dyn Seq> = (&C4.maj()).into_seq();
}

#[test]
fn combining_different_cycle_lengths_keeps_beats() {
    crate::time::set_meter(4, 4);
    let two = p("c4 d4").cycle(2.0); // 1 note per beat
    let bar = p("e4 f4 g4 a4"); // 1 note per beat in 4/4
    let st = &bar & &two;
    // every beat has two notes
    let n = st.notes(0.0, 4.0);
    assert_eq!(n.len(), 8, "{n:?}");
    assert!(
        n.iter()
            .all(|n| (n.start - n.start.round()).abs() < 1e-9 && (n.dur - 1.0).abs() < 1e-9)
    );
    let cat = &bar + &two;
    assert_eq!(Seq::len(&cat), 6.0);
    let n = cat.notes(0.0, 6.0);
    assert_eq!(
        n.iter().map(|n| n.start).collect::<Vec<_>>(),
        vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
    );
}

fn artics_of(p: &Pattern) -> Vec<crate::clip::Artic> {
    p.first_cycle().iter().map(|e| e.artic).collect()
}

#[test]
fn articulation_notation() {
    use crate::clip::Artic::*;
    assert_eq!(
        artics_of(&p("c4.stacc d4 e4.pz f4.ten")),
        vec![Staccato, Normal, Pizzicato, Tenuto]
    );
    assert_eq!(
        artics_of(&p("[c4 d4].trem e4.arco")),
        vec![Tremolo, Tremolo, Normal]
    );
    assert_eq!(
        artics_of(&p("c4.st^ff bd*2.acc")),
        vec![Staccato, Accent, Accent]
    );
    assert_eq!(
        p("c4^ff d4^pp")
            .first_cycle()
            .iter()
            .map(|e| e.vel)
            .collect::<Vec<_>>(),
        vec![0.92, 0.2]
    );
    // number then artic, artic after euclid and speed
    assert_eq!(
        artics_of(&p("60.marc bd(3,8).sp")),
        vec![Marcato, Spiccato, Spiccato, Spiccato]
    );
    // whitespace before '.' is not an articulation
    assert!(Pattern::parse("c4 .stacc").is_err());
    let e = Pattern::parse("c4.blah").unwrap_err();
    assert!(e.message.contains("articulation") && e.col == 4, "{e}");
    assert!(Pattern::parse("c4^loud").is_err());
    // artic reaches clip notes
    let n = p("c4.pizz").notes(0.0, 4.0);
    assert_eq!(n[0].artic, Pizzicato);
}

#[test]
fn slurs() {
    use crate::clip::Artic::*;
    let a = p("(c4 d4 e4) f4");
    let e = a.first_cycle();
    assert_eq!(
        e.iter().map(|e| e.artic).collect::<Vec<_>>(),
        vec![Legato, Legato, Normal, Normal]
    );
    // slurred notes overlap their successor
    assert!(e[0].end() > e[1].start && e[1].end() > e[2].start);
    assert!(e[2].end() <= e[3].start + 1e-9);
    let s = p("c4 [d4 e4] g4").slur();
    let ev = s.first_cycle();
    assert_eq!(ev.iter().filter(|e| e.artic == Legato).count(), 3);
    assert!(ev[0].end() > ev[1].start);
    assert!(Pattern::parse("c4 (d4 e4").is_err());
    assert_eq!(p("bd (c4 d4)").first_cycle().len(), 3);
    assert_eq!(p("bd(3,8) (c4 d4)").first_cycle().len(), 5);
}

#[test]
fn articulation_combinators() {
    use crate::clip::Artic::*;
    let a = p("c4 d4");
    assert_eq!(artics_of(&a.stacc()), vec![Staccato, Staccato]);
    assert_eq!(a.stacc().first_cycle()[0].dur, 0.25);
    assert_eq!(artics_of(&a.pizz()), vec![Pizzicato, Pizzicato]);
    assert!(a.marcato().first_cycle()[0].vel > 0.95);
    assert_eq!(artics_of(&a.artic(Trill)), vec![Trill, Trill]);
    let pa = p("c4*4").artics("<pizz arco> [stacc ~]");
    assert_eq!(artics_of(&pa), vec![Pizzicato, Pizzicato, Staccato, Normal]);
    assert_eq!(
        pa.events(1.0, 2.0)
            .iter()
            .map(|e| e.artic)
            .collect::<Vec<_>>(),
        vec![Normal, Normal, Staccato, Normal]
    );
    // combinators keep the articulation
    let k = p("c4.pizz e4")
        .fast(2)
        .rev()
        .up(12)
        .every(2, |p| p.ply(2))
        .struct_("x x x x");
    assert!(k.first_cycle().iter().any(|e| e.artic == Pizzicato));
    assert_eq!(artics_of(&p("C").pizz().arp("up", 3)), vec![Pizzicato; 3]);
    assert_eq!(
        artics_of(&p("c4").struct_("x.stacc x")),
        vec![Staccato, Normal]
    );
}

#[test]
fn dynamics() {
    use crate::units::Dyn;
    let a = p("c4 d4^1");
    let v: Vec<f32> = a
        .dynamic(Dyn::P)
        .first_cycle()
        .iter()
        .map(|e| e.vel)
        .collect();
    assert!((v[0] - 0.35).abs() < 1e-6 && v[1] > v[0]);
    let d = p("c4*4").dyns("<pp ff>");
    assert!((d.first_cycle()[0].vel - 0.2).abs() < 1e-6);
    assert!((d.events(1.0, 2.0)[0].vel - 0.92).abs() < 1e-6);
    let c = p("c4*4").cresc(Dyn::P, Dyn::F);
    let v: Vec<f32> = c.first_cycle().iter().map(|e| e.vel).collect();
    assert!(
        v.windows(2).all(|w| w[1] > w[0]) && (v[0] - 0.35).abs() < 1e-6,
        "{v:?}"
    );
    let c2 = p("c4*4").cresc_over(Dyn::P, Dyn::F, 2);
    assert!(c2.events(1.0, 2.0)[0].vel > c2.first_cycle()[3].vel);
    assert_eq!(c2.cycles(), 2.0);
    let dm = p("c4*4").dim(Dyn::F, Dyn::PP);
    assert!(dm.first_cycle()[3].vel < dm.first_cycle()[0].vel);
    let ph = p("c4*8").phrase(1.0);
    assert!((30..=34).contains(&ph.events(0.0, 4.0).len()));
}

#[test]
fn orchestration() {
    let chords = Key::major(C4).prog("I vi IV V");
    let [v1, v2, va, vc] = orchestrate4(&chords);
    assert_eq!(v1.cycles(), 4.0);
    for c in 0..4 {
        let t = |p: &Pattern| p.events(c as f64, c as f64 + 1.0)[0].pitch;
        assert!(t(&v1) > t(&v2) && t(&v2) > t(&va) && t(&va) > t(&vc));
    }
    let b = vc.double_oct(-1);
    assert_eq!(b.first_cycle().len(), 2);
    assert_eq!(
        p("c7 c1")
            .fit(crate::theory::Range::CELLO)
            .first_cycle()
            .iter()
            .map(|e| e.pitch)
            .collect::<Vec<_>>(),
        vec![84.0, 36.0]
    );
    assert_eq!(p("c4").double(7).first_cycle().len(), 2);
}
