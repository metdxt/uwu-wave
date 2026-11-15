//! # uwu-wave — a code-only DAW
//!
//! Music pieces are Rust programs. A [`Song`](song::Song) owns tracks (instrument + effect chain),
//! buses, sends, sidechains, automation lanes and an arrangement made of sections and clips.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let song = Song::new("hello").bpm(120);
//! // ...
//! song.run();
//! ```
#![allow(clippy::new_without_default)]

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
