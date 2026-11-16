# assets

`library/` is uwu-wave's bundled, redistributable sample library:
`library/<family>/<instrument>/<articulation>/` with a `zones.txt` manifest next to the audio. It
holds the Virtual Playing Orchestra sections and percussion, the piano, choirs, and cluesurf/wave
drums/guitars/harps. [library/INDEX.md](library/INDEX.md) lists every instrument and
[library/LICENSES.md](library/LICENSES.md) its sources and licenses.

`local/` (git-ignored) is an overlay for libraries you import yourself. An instrument there
replaces the instrument with the same id in `library/`, so imports upgrade presets
transparently, and everything still works without them.

## Philharmonia Orchestra samples (solo violin, viola, cello, winds, brass, percussion)

Their licence forbids redistributing the samples "as is", so they're not in this repo. To add them:

1. Download the archives (free) from <https://philharmonia.co.uk/resources/sound-samples/>
   (`all-samples.zip`, or `Brass.zip` / `Strings.zip` / `Woodwind.zip` / `Percussion.zip`).
2. Put them next to the repo, in its parent folder or in `~/Downloads`.
3. Run `cargo run --release --bin uwu -- library import philharmonia` (or pass the paths). It
   needs `unzip` and `ffmpeg` and takes about a minute.

```rust
let violin = Sampler::library("strings/violin/sustain");   // after the import
let violins = Sampler::library("strings/violins-1/staccato"); // always available
```

`uwu library list` marks overlay instruments with `[local]`. To rebuild the bundled library
from its source mirrors, use `uwu library build [--sources DIR]` (`--sources` defaults to
`~/.cache/uwu-wave/sources`). Zones with `url=` are downloaded on first use into
`~/.cache/uwu-wave/lfs` (`UWU_OFFLINE=1` disables that). `UWU_LIBRARY=dir1:dir2` overrides the
roots (first wins).
