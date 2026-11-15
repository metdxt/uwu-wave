//! # uwu-wave — a code-only DAW
//!
//! Music pieces are Rust programs. A [`Song`](song::Song) owns tracks (instrument + effect chain),
//! buses, sends, sidechains, automation lanes and an arrangement made of sections and clips.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let song = Song::new("hello").bpm(124);
//! let kick = song.track("kick", DrumMachine::tr909());
//! let bass = song.track("bass", Synth::acid()).fx(Ducker::new("kick"));
//! song.section("loop", 4.bars(), |s| {
//!     s.play(&kick, "bd*4");
//!     s.play(&bass, "a1 a1 [a2 a1] e2");
//! });
//! song.run();
//! ```
//!
//! See the repository README for the full tour.
#![allow(clippy::new_without_default, clippy::needless_range_loop, clippy::should_implement_trait, clippy::len_without_is_empty)]
#![allow(rustdoc::redundant_explicit_links, rustdoc::broken_intra_doc_links)]

pub mod error;
pub mod units;
pub mod time;
pub mod param;
pub mod dsp;
pub mod ctx;
pub mod clip;
pub mod instrument;
pub mod fx;
pub mod engine;
pub mod song;
pub mod pattern;
pub mod theory;
pub mod synth;
pub mod sample;
pub mod io;
pub mod prelude;

pub use ctx::Ctx;
pub use dsp::Frame;
pub use error::{Error, Result};
