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
        s.play(&pad, &chords);
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
| `aurora` | the full orchestra: 18 sections on stage in a convolution hall, articulations, hairpins, legato |
| `orchestra_sketch` | articulations, dynamics crossfades and legato, one feature at a time |
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

Pitch bend and mod wheel are track-level controls (semitones, 0..1) that every instrument follows:

```rust
let lead = song.track("lead", Synth::lead().mod_cutoff(1.5))
    .bend(auto![0 => 0.0, bar(7) => 0.0, bar(8) => -12.0])   // dive an octave
    .modwheel(lfo::tri(4.bars()).unipolar());                // vibrato + filter swell
```
Synths add vibrato and open the filter, `Fm` gets brighter, `Organ` speeds up its rotary,
`Pluck` strings bend while ringing, and samplers re-pitch smoothly. `--midi` exports both
as pitch-bend (with an RPN bend range) and CC1.

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
"hh*<8 16> rim(<3 5>,8)"    // patterned arguments: *<..>, /<..>, *[2 4], euclid(<..>,..)
"[c4,e4,g4] Cmaj7 F#m7b5 a3'm7 c4^0.5"   // chords, chord symbols, ^velocity
"c4^mf (d4 e4 f4) g4.stacc [a4 b4].pizz" // ^dynamics, (slur), .articulation (stacc st spicc sp pizz pz trem tr acc marc ten leg arco…)
```

Plain strings work anywhere a sequence is expected, so `s.play(&bass, "c2 ~ c2 eb2")` is valid; so do
chords, progressions and pitches (`s.play(&pad, &chords)`). Every combinator takes `&self`, so you
build variations of a pattern without ever writing `.clone()`:

```rust
let beat  = drums! { bd: "X... x... X... x...", sd: ".... x... .... x...", hh: "x.x.x.x.x.x.x.x." };
let fill  = p("~ ~ ~ [sd*4]");
let groove = beat.last_of(4, |p| p & &fill);                   // fill on every 4th bar
let hook  = p("c4 e4 g4").fast(2).rev().every(4, |p| p.up(12)).swing16(0.2).humanize(0.01, 0.1);
let degs  = n("0 2 4 <6 7>").scale("D4 dorian");               // scale degrees
let arps  = chords.pat().arp("updown", 16).legato(0.8);        // .pat(): strings/chords/progressions → Pattern
let quick = pat![c4 e4 [g4 b4]*2 ~];                            // unquoted mini-notation
&beat & "~ ~ ~ cr"    hook + degs    beat * 4                    // layer · one after the other · repeat
```
More: `sometimes/often/rarely`, `off`, `ply`/`chop`, `echo`/`stutter`, `struct_`, `mask`, `euclid`,
`palindrome`, `iter`, `strum`, `chord("m7")`, `triads(key)`, `inv/drop2/open`, `vels("1 .5")`,
`legato/staccato`, `late/early`, `degrade_by`, `fast_by("<1 2>")`, `.cycle(2.beats())`.

Articulations, dynamics and orchestration (the tags reach instruments as `NoteOn::artic`):

```rust
let line = n("0 2 4 5 4 2 0 ~").scale("A4 minor").slur().cresc(Dyn::P, Dyn::F);
let ost  = p("a2*8").stacc().dyns("<mp mf f ff>").artics("<spicc spicc stacc marc>");
let [vln1, vln2, vla, vc] = orchestrate4(&chords);     // smooth 4-part voicing, one pattern per section
let basses = vc.oct(-1).fit(Range::BASS);              // also .double_oct(-1), .pizz(), .trem(), .marcato()
```

### Theory

```rust
C4.maj7()   A3.min().inv(1)   Chord::parse("Bb/D")   Key::new(A3, Scale::Minor).degree(9)
prog("ii7 V7 Imaj7", "C major")  voice_lead(key.prog("i VI III VII"))  Scale::Dorian, Scale::Blues, …
Cs4 == Db4   C4 + 7 == G4   Key::parse("F# dorian")   key.quantize(E4)   Clip::note(0, 1, C4, 0.8)
Range::VIOLIN.fit(C2)   C4.maj().spread_over(&[Range::VIOLIN, Range::VIOLA, Range::CELLO])   voice_satb(&chords, &STRING_SECTION)
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

Velocity layers crossfade with equal power:
`Sampler::new().zone(Zone::new("soft.wav", "C4").vel(0.0, 0.5)).zone(Zone::new("hard.wav", "C4").vel(0.5, 1.0)).vel_xfade(0.15)`.
Before rendering or `--play`, samplers and kits **preload exactly the samples your notes will
hit**, so nothing is loaded in the audio thread. Git-LFS files are fetched 6 at a time with a
progress line. `Sampler::preload_all()` loads everything.

The `assets/wavebase` samples are git-LFS pointers. uwu-wave downloads the real files the
first time a sample is played and caches them in `~/.cache/uwu-wave/lfs`. Set
`UWU_OFFLINE=1` to turn fetching off and `UWU_ASSETS=<dir>` to point at another asset folder.

### The orchestra

`assets/library/` is a structured sample library: `<family>/<instrument>/<articulation>/`, with a
`zones.txt` manifest next to the audio. It holds 292 patches built from the Virtual Playing
Orchestra, the Philharmonia samples, a piano set and cluesurf/wave. See
[INDEX.md](assets/library/INDEX.md) for the full list and
[LICENSES.md](assets/library/LICENSES.md) for licences.

```rust
let vln1 = song.track("violins 1", Orchestra::violins_1())
    .fx(Stage::at("violins-1"))                 // seat on stage: pan, depth, air, early reflections
    .send(&hall, -8.db());                      // hall = song.bus("hall").fx(Convolver::hall().wet())
let [hi, mid, low, bass] = orchestrate4(&chords);   // 4-part voice leading for a string section

song.section("rising", 8.bars(), |s| {
    s.play(&vln1, n("(0 1 2) 4@2 ~").scale(key));   // ( ) slur → legato engine, no re-attack
    s.play(&vln2, mid.struct_("x*8").stacc());       // articulations pick sample sets
    s.play(&cb, "[d2 a1].pizz d2^ff");                // .pizz .stacc .trem .acc …, ^dynamics
    s.cresc(&vln1, Dyn::P, Dyn::FF);                 // CC1 crossfades pp → ff layers live
});
```

- **Instruments:** `Orchestra::violins_1() violins_2() violas() cellos() basses() strings() flute() oboe()
  cor_anglais() clarinet() bass_clarinet() bassoon() contrabassoon() piccolo() horns() horn_solo()
  trumpet() trombones() tuba() timpani() glockenspiel() xylophone() vibraphone() celesta()
  tubular_bells() harp() choir() cymbals() tam_tam() triangle()` and more (`Orchestra::available()`).
- **Articulations:** `Artic::{Sustain, Legato, Staccato, Spiccato, Pizzicato, Tremolo, Accent, Marcato,
  Tenuto, Trill, Harmonic, Muted…}`. Missing ones fall back sensibly (staccatissimo → staccato →
  spiccato…), and short notes can switch to staccato automatically.
- **Dynamics:** `track.dynamics(..)` / `s.dynamics` / `s.cresc` / `s.hairpin` drive CC1, which
  crossfades the sample layers and opens the tone while notes sustain. `track.expression(..)` drives
  CC11, and `track.cc(n, ..)` sets any controller. Without a CC1 lane, note velocity is the dynamic.
- **Legato engine:** overlapping notes on section and solo instruments (or `Artic::Legato`, or CC68)
  hand over with a crossfade and a short portamento instead of a new attack. The libraries contain no
  recorded legato transitions, so this is simulated.
- **Space:** `Convolver` is a partitioned-FFT convolution reverb with church, hall, studio, room and
  spring rooms; its latency is compensated. `Stage::at(..)` handles American seating, and
  `Compressor::orchestral()` and `Eq::orchestral_air()` are bus presets.
- **Theory:** `Range::VIOLIN` and friends, `pattern.fit(range)`, `voice_satb`, `orchestrate4`, `.double_oct(n)`.

The full example is `cargo run --release --example aurora`, a 2½-minute cue for 18 sections.
The library is git-ignored; rebuild it from the source mirrors with
`cargo run --release --bin uwu -- library build` (see [assets/README.md](assets/README.md)).

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
An insert can listen to several sidechains (`chain![Ducker::new("kick"), Ducker::new("snare")]`
gives each ducker its own source). **Plugin delay compensation** is automatic: look-ahead
limiters and oversampled distortion report their latency, every path to a bus or the master
is delayed to match, and stems stay sample-aligned with the mix.

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
