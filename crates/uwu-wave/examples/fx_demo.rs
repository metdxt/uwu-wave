//! fx_demo — a short track that shows off the effects.
//!
//! The instruments are tiny private voices so this example only depends on the core + fx; the
//! interesting part is the routing and the effect chains.
//!
//! ```text
//! cargo run --release --example fx_demo            # → out/fx_demo.wav + report
//! cargo run --release --example fx_demo -- --play  # listen live
//! ```

use uwu_wave::dsp::{Adsr, Osc, Rng, Wave, midi_to_hz};
use uwu_wave::prelude::*;

// ---- a tiny multi-purpose voice ----------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    Thump,
    Tick,
    Clap,
    Bass,
    Pad,
    Pluck,
}

#[derive(Clone)]
struct Tone {
    kind: Kind,
    osc: [Osc; 3],
    env: Adsr,
    rng: Rng,
    hz: f32,
    vel: f32,
    sr: f32,
    t: f32,
}

fn tone(kind: Kind) -> Poly<Tone> {
    let (wave, env) = match kind {
        Kind::Bass => (Wave::Saw, Adsr::new(0.003, 0.25, 0.6, 0.06)),
        Kind::Pad => (Wave::Saw, Adsr::new(0.8, 1.0, 0.8, 1.5)),
        Kind::Pluck => (Wave::Square, Adsr::new(0.002, 0.22, 0.0, 0.15)),
        _ => (Wave::Sine, Adsr::new(0.001, 0.1, 1.0, 0.05)),
    };
    let osc = [
        Osc::new(wave),
        Osc::new(wave).with_phase(0.33),
        Osc::new(wave).with_phase(0.71),
    ];
    let v = Tone {
        kind,
        osc,
        env,
        rng: Rng::new(7),
        hz: 110.0,
        vel: 1.0,
        sr: 48_000.0,
        t: 0.0,
    };
    match kind {
        Kind::Bass => Poly::new(v).legato(0.03),
        Kind::Thump | Kind::Tick | Kind::Clap => Poly::new(v).voices(4),
        _ => Poly::new(v).voices(12),
    }
}

impl Voice for Tone {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, _: &Ctx) {
        self.hz = midi_to_hz(n.pitch);
        self.vel = n.vel;
        self.t = 0.0;
        self.env.gate_on(self.sr);
    }
    fn legato(&mut self, n: &NoteOn, _glide: f32, _: &Ctx) {
        self.hz = midi_to_hz(n.pitch);
        self.vel = n.vel;
    }
    fn release(&mut self, _: &Ctx) {
        self.env.gate_off(self.sr);
    }
    fn render(&mut self, out: &mut [Frame], _: &Ctx) -> bool {
        let dt = 1.0 / self.sr;
        for f in out.iter_mut() {
            let t = self.t;
            self.t += dt;
            *f += match self.kind {
                Kind::Thump => {
                    let hz = 46.0 + 120.0 * (-t * 38.0).exp();
                    Frame::mono(self.osc[0].next(hz, self.sr) * (-t * 7.0).exp() * self.vel * 0.9)
                }
                Kind::Tick => Frame::mono(self.rng.bipolar() * (-t * 70.0).exp() * self.vel * 0.3),
                Kind::Clap => {
                    let burst = if t < 0.03 {
                        0.6 + 0.4 * ((t * 300.0).fract() < 0.5) as u8 as f32
                    } else {
                        1.0
                    };
                    Frame::mono(self.rng.bipolar() * (-t * 22.0).exp() * burst * self.vel * 0.45)
                }
                Kind::Bass => {
                    let e = self.env.next();
                    let saw = self.osc[0].next(self.hz, self.sr);
                    let sub = self.osc[1].next(self.hz * 0.5, self.sr);
                    Frame::mono((saw * 0.6 + sub * 0.5) * e * self.vel * 0.35)
                }
                Kind::Pad => {
                    let e = self.env.next() * self.vel * 0.07;
                    let a = self.osc[0].next(self.hz * 1.0046, self.sr);
                    let b = self.osc[1].next(self.hz * 0.9954, self.sr);
                    let c = self.osc[2].next(self.hz, self.sr);
                    Frame::new((a + c * 0.7) * e, (b + c * 0.7) * e)
                }
                Kind::Pluck => {
                    let e = self.env.next();
                    Frame::mono(self.osc[0].next(self.hz, self.sr) * e * e * self.vel * 0.16)
                }
            };
        }
        match self.kind {
            Kind::Thump => self.t < 0.7,
            Kind::Tick => self.t < 0.12,
            Kind::Clap => self.t < 0.35,
            _ => !self.env.is_idle(),
        }
    }
}

// ---- clips -------------------------------------------------------------------------------------

const ROOTS: [i32; 4] = [45, 41, 48, 43]; // A F C G
const CHORDS: [[i32; 3]; 4] = [[57, 60, 64], [53, 57, 60], [55, 60, 64], [55, 59, 62]];

fn four_on_the_floor() -> Clip {
    (0..4).fold(Clip::new(4.beats()), |c, b| c.note(b, 0.5, 36, 1.0))
}
fn hats() -> Clip {
    (0..16).fold(Clip::new(4.beats()), |c, i| {
        c.note(
            i as f64 * 0.25,
            0.1,
            42,
            if i % 4 == 2 { 0.9 } else { 0.35 },
        )
    })
}
fn claps() -> Clip {
    Clip::new(4.beats())
        .note(1, 0.5, 39, 0.9)
        .note(3, 0.5, 39, 1.0)
}
fn bassline() -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, &r) in ROOTS.iter().enumerate() {
        for i in 0..8 {
            if i % 2 == 1 || i == 0 {
                let p = r - 12 + if i == 7 { 12 } else { 0 };
                c = c.note(
                    bar as f64 * 4.0 + i as f64 * 0.5,
                    0.42,
                    p,
                    if i == 0 { 1.0 } else { 0.8 },
                );
            }
        }
    }
    c
}
fn pads() -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, ch) in CHORDS.iter().enumerate() {
        for &p in ch {
            c = c.note(bar as f64 * 4.0, 3.9, p, 0.8);
        }
    }
    c
}
fn arps() -> Clip {
    let mut c = Clip::new(16.beats());
    for (bar, ch) in CHORDS.iter().enumerate() {
        let tones = [
            ch[0] + 12,
            ch[1] + 12,
            ch[2] + 12,
            ch[1] + 24,
            ch[2] + 12,
            ch[0] + 24,
            ch[1] + 12,
            ch[2] + 24,
        ];
        for i in 0..16 {
            c = c.note(
                bar as f64 * 4.0 + i as f64 * 0.25,
                0.2,
                tones[(i * 3) % 8],
                if i % 4 == 0 { 0.9 } else { 0.6 },
            );
        }
    }
    c
}

fn main() {
    let song = Song::new("fx_demo").bpm(124);

    // automation lanes written by the sections below
    let pad_cut = Lane::new(400.0);
    let shimmer_send = Lane::new(0.0);
    let crush_mix = Lane::new(0.0);
    let crush_bits = Lane::new(12.0);

    // ---- effect returns
    let verb = song
        .bus("verb")
        .fx(Reverb::hall().wet().predelay(30.ms()).low_cut(200.0));
    let space = song
        .bus("space")
        .fx(Reverb::shimmer().wet())
        .fx(Eq::new().low_cut(300.0))
        .gain(-4.db());
    let echo = song
        .bus("echo")
        .fx(Delay::new(dotted(eighth()))
            .ping_pong()
            .feedback(0.5)
            .wet()
            .tape())
        .fx(HighPass::new(250.0))
        .fx(Ducker::new("kick").amount(0.5).release(200.ms()));

    // ---- drums
    let kick = song
        .track("kick", tone(Kind::Thump))
        .fx(chain![
            Eq::new().low_shelf(60.0, 2.db()).peak(320.0, -4.db(), 1.4),
            TransientShaper::new().attack(0.5).sustain(-0.2),
            Distortion::tube(2.0).mix(0.35),
        ])
        .gain(-5.db());
    let hat = song
        .track("hats", tone(Kind::Tick))
        .fx(HighPass::new(7_000.0).slope(24))
        .fx(AutoPan::new(2.bars()).depth(0.5))
        .fx(Ducker::new("kick").amount(0.4).release(90.ms()))
        .gain(-4.db());
    let clap = song
        .track("clap", tone(Kind::Clap))
        .fx(BandPass::new(1_300.0).q(0.8))
        .fx(Compressor::punch())
        .send(&verb, -4.db());

    // ---- music
    let bass = song
        .track("bass", tone(Kind::Bass))
        .fx(Ladder::new(lfo::sine(2.bars()).range(250.0, 2_200.0))
            .resonance(0.55)
            .drive(2.0))
        .fx(Distortion::tube(3.0).mix(0.4).tone(5_000.0))
        .fx(Compressor::pump().threshold(-20.db()).sidechain("kick"))
        .gain(5.db());
    let pad = song
        .track("pad", tone(Kind::Pad))
        .fx(LowPass::new(&pad_cut).q(0.9).slope(24))
        .fx(Chorus::juno())
        .fx(Phaser::new().rate(4.bars()).feedback(0.4).mix(0.3))
        .fx(Ducker::new("kick").amount(0.6).release(180.ms()))
        .gain(3.db())
        .send(&verb, -6.db())
        .send(&space, &shimmer_send);
    let arp = song
        .track("arp", tone(Kind::Pluck))
        .fx(Parallel::new(Bitcrush::new().bits(&crush_bits).rate(8_000.0)).mix(&crush_mix))
        .fx(Flanger::new().rate(1.bars()).mix(0.25))
        .fx(Tremolo::new(eighth()).depth(0.25))
        .fx(Width::new(1.3))
        .gain(3.db())
        .pan(-0.1)
        .send(&echo, -7.db())
        .send(&verb, -12.db());

    // ---- master: tone, glue, colour, width, loudness
    song.master()
        .fx(Eq::new()
            .low_cut(28.0)
            .peak(250.0, -1.db(), 0.8)
            .high_shelf(10_000.0, 1.5.db()))
        .fx(Compressor::glue())
        .fx(TapeSat::new().drive(1.5))
        .fx(Width::new(1.1).bass_mono(120.0))
        .fx(Limiter::new().ceiling(-1.db()).gain(3.db()));

    // ---- arrangement
    song.section("intro", 4.bars(), |s| {
        s.play(&pad, pads());
        s.during(bar(2)..bar(4), &hat, hats());
        s.ramp_exp(&pad_cut, 300.0, 2_500.0);
        s.ramp(&shimmer_send, 0.0, 0.5);
    });
    song.section("build", 4.bars(), |s| {
        s.play(&pad, pads());
        s.play(&hat, hats());
        s.play(&arp, arps());
        s.during(bar(2)..bar(4), &clap, claps());
        s.ramp_exp(&pad_cut, 2_500.0, 7_000.0);
        s.ramp(&crush_mix, 0.0, 0.8);
        s.ramp(&crush_bits, 12.0, 3.0);
        s.set(&shimmer_send, 0.0);
    });
    song.section("drop", 8.bars(), |s| {
        s.play(&kick, four_on_the_floor());
        s.play(&hat, hats());
        s.play(&clap, claps());
        s.play(&bass, bassline());
        s.play(&pad, pads());
        s.play(&arp, arps());
        s.set(&pad_cut, 5_000.0);
        s.set(&crush_mix, 0.0);
    });
    song.section("break", 4.bars(), |s| {
        s.play(&pad, pads());
        s.play(&arp, arps());
        s.ramp(&shimmer_send, 0.2, 1.0);
        s.ramp_exp(&pad_cut, 5_000.0, 900.0);
        s.ramp(&crush_mix, 0.0, 0.5);
        s.set(&crush_bits, 5.0);
    });
    song.section("outro", 4.bars(), |s| {
        s.play(&kick, four_on_the_floor());
        s.play(&bass, bassline());
        s.during(bar(0)..bar(2), &hat, hats());
        s.once(
            &pad,
            Clip::new(4.beats())
                .note(0, 8, 57, 0.8)
                .note(0, 8, 60, 0.8)
                .note(0, 8, 64, 0.8),
        );
        s.set(&crush_mix, 0.0);
        s.ramp(&shimmer_send, 1.0, 0.0);
        s.ramp_exp(&pad_cut, 900.0, 300.0);
    });

    song.run();
}
