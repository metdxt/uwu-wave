//! Input/output: WAV files, the command-line runner, live playback, MIDI export.
//!
//! * [`wav::write_wav`] — stereo WAV writer (16/24-bit dithered, 32-bit float)
//! * [`cli::run`] — what [`Song::run`](crate::song::Song::run) does: argument parsing, rendering,
//!   `--play`, `--info`, `--stems`, `--midi`... (see [`cli`] for the full list)
//! * [`midi::write_midi`] — Standard MIDI File export
//! * `play::play` — real-time playback (feature `play`)
pub mod cli;
pub mod midi;
#[cfg(feature = "play")]
pub mod play;
pub mod wav;

pub mod prelude {
    //! I/O items re-exported by `uwu_wave::prelude`.
}
