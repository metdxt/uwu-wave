//! Samples: load WAV files and play them with the [`Sampler`] (multi-zone, round-robin,
//! velocity layers, loops, reverse...) and [`SampleKit`] (drum pads with choke groups).
//!
//! Presets built from the bundled `assets/` folder (found via `$UWU_ASSETS` or by walking up from
//! the current directory):
//!
//! | preset | source |
//! |---|---|
//! | [`Sampler::piano()`] | `assets/piano` (88 keys, enharmonic pairs alternate round-robin) |
//! | [`Sampler::bass_guitar()`] | `wavebase/base/bass` (D1–G4) |
//! | [`Sampler::guitar()`] | `wavebase/base/guitar/stratocaster` (D2–Cx6) |
//! | [`Sampler::guitar_fly()`] | `wavebase/base/guitar/parker-fly` (8 takes per note) |
//! | [`Sampler::harp()`] | `wavebase/base/harp/celtic` (8–9 takes per note) |
//! | [`Sampler::zither()`] | `wavebase/base/harp/zither` |
//! | [`SampleKit::acoustic()`] | `wavebase/base/drum/*.wav` |
//!
//! The `wavebase` WAVs are git-LFS pointers: they are downloaded on first use into
//! `~/.cache/uwu-wave/lfs` (see [`lfs`]; `UWU_OFFLINE=1` disables this). Sampler zones load
//! lazily (only notes you actually play are fetched/decoded) unless you call
//! [`Sampler::preload`]; kits load at `prepare()`.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let song = Song::new("samples").bpm(96);
//! let keys = song.track("piano", Sampler::piano().release(0.5).tone(0.6));
//! let drums = song.track("drums", SampleKit::acoustic());
//! let bass = song.track("bass", Sampler::bass_guitar().gain(-2.db()));
//! ```

pub mod kit;
pub mod lfs;
pub mod load;
mod presets;
pub mod sampler;
pub mod voice;

pub use kit::{SampleKit, gm};
pub use load::{Sample, asset, assets_dir, clear_cache, note_in_name, parse_note, piano_key};
pub use sampler::{RootPitch, Sampler, SamplerVoice, Zone};
pub use voice::Region;

pub mod prelude {
    //! Sample items re-exported by `uwu_wave::prelude`.
    pub use super::{Sample, SampleKit, Sampler, Zone};
}
