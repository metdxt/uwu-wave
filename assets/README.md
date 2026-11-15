# assets

`library/` is uwu-wave's structured sample library: `library/<family>/<instrument>/<articulation>/`
with a `zones.txt` manifest next to the audio. See [library/INDEX.md](library/INDEX.md) for every
instrument (generated) and [library/LICENSES.md](library/LICENSES.md) for sources and licenses.

```rust
let violins = Sampler::library("strings/violins-1/staccato");
let horn = Library::patch("brass/horn/sustain")?.sampler();
```

The library is built from the original source mirrors (Virtual Playing Orchestra, the
Philharmonia samples, the piano set, cluesurf/wave) with:

```sh
cargo run --release --bin uwu -- library build [--sources DIR] [--only vpo,phil,piano,wavebase]
```

`--sources` defaults to `~/.cache/uwu-wave/sources` (where the mirrors were moved). Zones with
`url=` (cluesurf/wave drums, guitars, harps) are downloaded on first use into
`~/.cache/uwu-wave/lfs`; set `UWU_OFFLINE=1` to disable that.
