use uwu_wave::dsp::{midi_to_hz, Adsr, Osc, Wave};
use uwu_wave::prelude::*;

#[derive(Clone)]
struct Beep {
    osc: Osc,
    env: Adsr,
    hz: f32,
    vel: f32,
    sr: f32,
}
impl Voice for Beep {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        self.hz = midi_to_hz(n.pitch);
        self.vel = n.vel;
        self.env.gate_on(self.sr);
    }
    fn release(&mut self, _: &Ctx) {
        self.env.gate_off(self.sr);
    }
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        for f in out.iter_mut() {
            let e = self.env.next();
            *f += Frame::mono(self.osc.next(self.hz, self.sr) * e * self.vel * 0.3);
        }
        !self.env.is_idle()
    }
}

fn beep() -> Poly<Beep> {
    Poly::new(Beep { osc: Osc::new(Wave::Sine), env: Adsr::new(0.01, 0.1, 0.7, 0.2), hz: 440.0, vel: 1.0, sr: 48000.0 })
}

#[test]
fn renders_notes_routes_and_tail() {
    let song = Song::new("smoke").bpm(120);
    let bus = song.bus("verb").gain(-6.db());
    let t = song.track("beep", beep()).send(&bus, 0.5).pan(lfo::sine(1.bars()));
    let clip = Clip::new(4.beats()).note(0, 1, 60, 1.0).note(1, 1, 64, 0.8).note(2, 2, 67, 0.6);
    song.section("a", 2.bars(), |s| {
        s.play(&t, clip.clone());
    });
    let r = song.render_with(&RenderOptions { stems: true, ..Default::default() }).unwrap();
    println!("{}", r.report);
    // 2 bars at 120 bpm = 4 s, plus a short tail
    assert!(r.duration_secs() >= 4.0 && r.duration_secs() < 5.5, "{}", r.duration_secs());
    assert!(r.report.master_peak_db > -20.0 && r.report.master_peak_db < 0.0);
    assert_eq!(r.report.channels.iter().find(|c| c.name == "beep").unwrap().notes, 6);
    assert_eq!(r.stems.len(), 2);
    assert!(song.render().is_err());
}

#[test]
fn routing_cycle_is_an_error() {
    let song = Song::new("cycle");
    let a = song.bus("a");
    let b = song.bus("b").to(&a);
    a.send(&b, 1.0);
    assert!(song.render().is_err());
}
