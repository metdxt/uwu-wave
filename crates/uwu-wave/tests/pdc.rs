//! Plugin delay compensation & multi-sidechain routing.
use uwu_wave::prelude::*;

/// Emits a single-sample click per note.
#[derive(Clone)]
struct Click {
    pending: bool,
    vel: f32,
}
impl Voice for Click {
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        self.pending = true;
        self.vel = n.vel;
    }
    fn release(&mut self, _: &Ctx) {}
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        if self.pending && !out.is_empty() {
            out[0] += Frame::mono(self.vel * 0.4);
            self.pending = false;
        }
        false
    }
}

fn click() -> Poly<Click> {
    Poly::new(Click { pending: false, vel: 1.0 })
}

fn peaks(a: &[Frame]) -> Vec<usize> {
    a.iter().enumerate().filter(|(_, f)| f.l.abs() > 0.05).map(|(i, _)| i).collect()
}

#[test]
fn latent_insert_is_compensated() {
    let song = Song::new("pdc");
    let a = song.track("a", click()).fx(Limiter::new()); // look-ahead => latency
    let b = song.track("b", click());
    let bus = song.bus("bus");
    let c = song.track("c", click()).to(&bus);
    let clip = Clip::new(4.beats()).note(0, 1, 60, 1.0);
    a.play(bar(0), &clip);
    b.play(bar(0), &clip);
    c.play(bar(0), &clip);
    let mut e = song.engine(&RenderOptions { stems: true, no_tail: true, ..Default::default() }).unwrap();
    assert!(e.latency() > 0);
    let r = e.render_all();
    let p = peaks(&r.audio);
    assert_eq!(p.first(), Some(&0), "mix peaks at {p:?}");
    assert!(p.iter().all(|&i| i < 4), "all clicks must coincide: {p:?}");
    for (name, s) in &r.stems {
        if let Some(&first) = peaks(s).first() {
            assert!(first < 4, "stem {name} starts at {first}");
        }
    }
}

/// Records which sidechain names it received.
struct Probe {
    want: Vec<String>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
impl Effect for Probe {
    fn process(&mut self, _buf: &mut [Frame], ctx: &Ctx) {
        let mut s = self.seen.lock().unwrap();
        for (n, b) in ctx.sidechains {
            if b.iter().any(|f| f.l != 0.0) && !s.contains(&n.to_string()) {
                s.push(n.to_string());
            }
        }
    }
    fn sidechains(&self) -> Vec<String> {
        self.want.clone()
    }
}

#[test]
fn multiple_sidechains_are_delivered() {
    let song = Song::new("sc");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let k = song.track("kick", click());
    let s = song.track("snare", click());
    let pad = song.bus("pad").fx(Probe { want: vec!["kick".into(), "snare".into()], seen: seen.clone() });
    let _ = pad;
    k.play(bar(0), Clip::new(4.beats()).note(0, 1, 60, 1.0));
    s.play(bar(0), Clip::new(4.beats()).note(1, 1, 60, 1.0));
    song.render().unwrap();
    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, vec!["kick".to_string(), "snare".to_string()]);
    let bad = Song::new("bad");
    bad.bus("x").fx(Probe { want: vec!["nope".into()], seen });
    assert!(bad.render().is_err());
}
