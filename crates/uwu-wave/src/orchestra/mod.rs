//! # Orchestra — articulated sampled instruments
//!
//! A small Kontakt/SWAM-style engine on top of the sound library
//! (`assets/library/<family>/<instrument>/<articulation>/zones.txt`):
//!
//! * [`Articulated`] — per-note articulation switching with fallbacks, CC1 dynamic-layer
//!   crossfades, CC11 expression, a legato/portamento engine, CC21 vibrato, humanize, round
//!   robins, pitch bend.
//! * [`Orchestra`] — presets for every library instrument with stage seats and gain staging.
//! * [`ArticPatch`] / [`ArticZone`] — build your own patches in code or load one articulation.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let song = Song::new("nocturne").bpm(70);
//! let vc = song.track("cellos", Orchestra::cellos()).dynamics(auto![0 => Dyn::PP, bar(4) => Dyn::F => smooth]);
//! let fl = song.track("flute", Orchestra::flute().vibrato(0.1));
//! ```

mod engine;
mod patch;
mod presets;

pub use engine::{Articulated, MAX_LAYERS, fallback_chain};
pub use patch::{ArticPatch, ArticZone, instrument_dirs, library_root, load_dir};
pub use presets::{Orchestra, available};

pub mod prelude {
    //! Orchestra names re-exported by `uwu_wave::prelude`.
    pub use super::{ArticPatch, ArticZone, Articulated, Orchestra};
}
