# uwu-wave 〜 a code-only DAW in Rust

**Songs are Rust programs.** No piano roll, no mouse. You get tracks, buses, sends, sidechains,
automation lanes, LFOs, instruments, samplers and a mastering chain, all written as
code with plenty of syntactic sugar.

```rust
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
```

## Quick start

```sh
cargo run --release --example neon_rain                 # render → out/Neon_Rain.wav + report
cargo run --release --example neon_rain -- --play       # realtime playback (cpal)
cargo run --release --example neon_rain -- --info       # ASCII arrangement view
cargo run --release --example neon_rain -- --play --from drop --solo bass,kick --loop
cargo run --release --example neon_rain -- --stems --midi out/neon_rain.mid

cargo run --release --bin uwu -- new my_song            # scaffold examples/my_song.rs
cargo run --release --bin uwu -- live my_song           # live-coding: replays on every save
```

Examples (`crates/uwu-wave/examples/`):

| example | shows |
|---|---|
| `neon_rain` | the flagship: full arrangement with lanes, sidechain, sends, tempo automation, a sampled piano |
| `synth_demo` | every synth preset: acid, supersaw, pad, FM, plucks, organ, 808 bass, drum machines |
| `fx_demo` | reverbs, delays, filters, dynamics, modulation, automation of effect knobs |
| `pattern_demo` | mini-notation, drum grids, scale degrees, roman numerals, arps, swing, humanize |
| `sample_demo` | sampled piano, bass guitar, harp and acoustic kit from `assets/` |

## Command line (`song.run()`)

| flag | |
|---|---|
| `-o, --out <file>` | output wav (default `out/<name>.wav`) |
| `-p, --play` / `--loop` | realtime playback with a status line (bar:beat, section, meter) |
| `--from` / `--to <pos>` | `16` (bar), `16:2` (bar:beat), `30s`, `1:30s`, or a section/marker name |
| `--solo a,b` / `--mute a,b` | audition tracks |
| `--stems [dir]` | one wav per track/bus |
| `--midi <file>` | Standard MIDI File export (tempo map, markers, one track per track) |
| `--info` | arrangement overview |
| `--sr`, `--bits 16\|24\|32`, `--no-tail` | output format |

After every render you get a report with peak/RMS per channel, an integrated LUFS reading
(BS.1770, gated) for the master, and a clipping warning if the master goes over full scale.

## The tour

### Units & time

```rust
-6.db()    440.hz()   2.khz()   120.ms()   50.pct()   7.st()      // -6.db() really is -6 dB
bar(4)     bars(0..8) 2.bars()  3.beats()  quarter()  dotted(eighth())  triplet(quarter())  note(1, 16)
```
Everything on the timeline is measured in beats. Bars are **0-based** (`bar(0)` is the start)
and follow the song's meter (`Song::new(..).meter(6, 8)`).

### Parameters: every knob is automatable

Every knob takes `impl Into<Param>`:

```rust
LowPass::new(800.0)                                          // constant
LowPass::new(auto![0 => 200.0, bar(8) => 8000.0 => exp])     // automation (lin/exp/step/smooth)
LowPass::new(lfo::sine(2.bars()).range(300.0, 3000.0))       // tempo-synced LFO (or lfo::sine(5.0) = 5 Hz)
LowPass::new(lfo::tri(1.bars()) * 400.0 + 1200.0)            // arithmetic on params
LowPass::new(&cutoff)                                        // an automation Lane written later
track.volume_db(auto![bar(60) => 0, bar(64) => -40])         // fades in dB
auto![0 => 0.2, 0.25 => 1.0 => exp, 1 => 1.0].looped(1.beats())   // repeating per-beat shape
```

A **`Lane`** is a shared automation lane. Create it, give it to any knob, and let the
arrangement write it:

```rust
let sweep = Lane::new(20_000.0);
let music = song.bus("music").fx(LowPass::new(&sweep));
song.section("build", 8.bars(), |s| { s.ramp_exp(&sweep, 400.0, 20_000.0); });
```

### Patterns (Strudel/Tidal-style mini-notation)

```rust
"c4 e4 g4 [b4 c5]"          // one cycle (= one bar) divided equally; [..] subdivides
"bd*4, ~ cp, hh*8"          // , stacks layers; *n speeds up; GM drum names
"c4 <e4 g4 bb4>"            // <..> alternates per cycle
"c4@3 e4 _ ~"               // @ weight, _ tie, ~ rest
"bd(3,8,2) hh?0.3"          // euclid, random drop (seeded and repeatable)
"[c4,e4,g4] Cmaj7 F#m7b5 a3'm7 c4^0.5"   // chords, chord symbols, ^velocity
```

Plain strings work anywhere a sequence is expected, so `s.play(&bass, "c2 ~ c2 eb2")` is valid.
Builders and combinators:

```rust
p("c4 e4 g4").fast(2).rev().every(4, |p| p.up(12)).swing16(0.2).humanize(0.01, 0.1)
n("0 2 4 <6 7>").scale(Key::parse("D dorian").unwrap())        // scale degrees
Pattern::from(chords).arp("updown", 16).legato(0.8)              // arpeggiator
drums! { bd: "X... x... X... x...", sd: ".... x... .... x...", hh: "x.x.x.x.x.x.x.x." }
pat![c4 e4 [g4 b4]*2 ~]                                          // unquoted mini-notation
a & b    // layer        a + b    // one after the other        a * 4    // repeat
```
More: `sometimes/often/rarely`, `off`, `ply`, `struct_`, `mask`, `euclid`, `palindrome`, `iter`,
`strum`, `chord("m7")`, `triads(key)`, `vels("1 .5")`, `legato/staccato`, `late/early`, `degrade_by`.

### Theory

```rust
C4.maj7()   A3.min().inv(1)   Chord::parse("Bb/D")   Key::new(A3, Scale::Minor).degree(9)
prog("ii7 V7 Imaj7", "C major")  voice_lead(key.prog("i VI III VII"))  Scale::Dorian, Scale::Blues, …
```

### Instruments

| instrument | presets & highlights |
|---|---|
| `Synth` | `saw square sine tri supersaw acid pad pluck bass reese lead brass keys chip zap`; 3 osc + unison, sub, noise, SVF/ladder filter with env & key tracking, accent, pitch env, vibrato, mono/legato glide |
| `Fm` | `epiano bell bass metallic marimba`; 2–4 operators, 6 algorithms, automatable index |
| `Pluck` | Karplus–Strong `guitar harp koto bass` |
| `Organ` | drawbars `Organ::new("888000000")`, `jazz gospel rock`, percussion, rotary |
| `DrumMachine` | `tr808 tr909 lofi`, GM-mapped synthesized kit (`Kick Snare Clap HiHat Tom Cymbal Cowbell Rimshot Shaker`), choke groups |
| `Sampler` | `piano bass_guitar guitar guitar_fly harp zither`, zones, velocity layers, round-robin, loops, `from_dir`, `one_shot` |
| `SampleKit` | `acoustic` (wavebase drums), `from_files([(36, "kick.wav"), …])` |
| your own | implement `Voice` and wrap it in `Poly` (voice allocation, mono/legato/glide) |

The `assets/wavebase` samples are git-LFS pointers. uwu-wave downloads the real files the
first time a sample is played and caches them in `~/.cache/uwu-wave/lfs`. Set
`UWU_OFFLINE=1` to turn fetching off and `UWU_ASSETS=<dir>` to point at another asset folder.

### Effects

| area | effects |
|---|---|
| filters | `LowPass HighPass BandPass Notch` (12/24/48 dB), `Ladder`, `Eq` (cuts, shelves, bells) |
| space | `Reverb` (`room chamber hall plate cathedral shimmer`), `Delay` (synced, ping-pong, tape, dub) |
| modulation | `Chorus` (`juno`), `Flanger`, `Phaser`, `Tremolo`, `AutoPan` |
| colour | `Distortion` (`soft hard tube fold fuzz`, 2× oversampled), `TapeSat`, `Bitcrush` |
| dynamics | `Compressor` (`glue punch pump`, sidechain), `Limiter` (look-ahead), `Gate`, `Ducker`, `TransientShaper` |
| utility | `Utility`, `Width` (bass mono), `Chain` / `chain![..]`, `Parallel`, `FnFx` (closure) |

Level knobs (thresholds, ceilings, EQ gains) take linear gains, so write them as `-18.db()`.
`Reverb`/`Delay` on return buses: use `.wet()`.

### Mixer & routing

```rust
let verb  = song.bus("verb").fx(Reverb::plate().wet());
let drums = song.bus("drum bus").fx(Compressor::punch());
song.track("kick", DrumMachine::tr909()).to(&drums);          // route into a group bus
song.track("lead", Synth::lead()).send(&verb, -9.db())         // post-fader send
    .send_pre(&verb, -20.db()).pan(-0.3).gain(-4.db());        // pre-fader send, pan, gain
song.track("bass", Synth::bass()).fx(Compressor::new().sidechain("kick"));  // sidechain by name
song.master().fx(Eq::new().low_cut(30.0)).fx(Limiter::master());
track.mute(); track.solo(); track.transpose(-12); track.velocity(0.8);
```
Channels are processed in dependency order, and routing cycles are reported as errors.

### Arrangement

```rust
song.section("verse", 16.bars(), |s| {
    s.play(&drums, &beat);                        // loop over the whole section
    s.once(&piano, &intro_phrase);                // play once at the start
    s.at(4.bars(), &fx, "cr");                    // one-shot at an offset
    s.during(8.bars()..16.bars(), &lead, &hook);  // loop over a sub-range
    s.ramp(&lane, 0.0, 1.0);  s.set_at(12.bars(), &lane, 0.5);  s.marker("hook", 8.bars());
});
track.play(bar(64), clip);   track.play(bars(0..8), pat);   track.play(bar(8).., pat);  // free placement
song.bpm(auto![0 => 118, bar(32) => 124 => smooth]);   // tempo automation
```

### Bring your own DSP

```rust
#[derive(Clone)] struct MyVoice { /* … */ }
impl Voice for MyVoice { fn start(..) {..} fn release(..) {..} fn render(..) -> bool {..} }
song.track("mine", Poly::new(MyVoice { .. }).voices(8));
song.track("x", Synth::saw()).fx(FnFx::new(|f, _ctx| f.map(|x| (x * 3.0).tanh())));
```
Building blocks live in `uwu_wave::dsp`: `Osc` (PolyBLEP), `Svf`, `Biquad`, `Adsr`,
`DelayLine`, `Rng` and smoothing helpers.

## Layout

```
crates/uwu-wave/src/
  units.rs time.rs param.rs   ← sugar, musical time, Param/automation/LFO/Lane
  song.rs engine.rs ctx.rs    ← Song/Track/Section API, block renderer, routing graph
  clip.rs instrument.rs       ← Note/Seq/Clip, Instrument/Voice/Poly
  dsp/                        ← primitives
  synth/ sample/ fx/          ← instruments & effects
  pattern/ theory/            ← mini-notation, combinators, scales/chords/keys
  io/ bin/uwu.rs              ← wav, CLI, playback, MIDI, live-coding helper
```

`cargo test -p uwu-wave` runs about 145 unit and doc tests.
