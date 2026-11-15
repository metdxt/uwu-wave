//! `use uwu_wave::prelude::*;` — everything you need to write a song.

pub use crate::auto;
pub use crate::clip::{rest, Clip, IntoSeq, Note, Seq};
pub use crate::ctx::Ctx;
pub use crate::dsp::Frame;
pub use crate::engine::{Render, RenderOptions, Report};
pub use crate::instrument::{Instrument, Layer, NoteOn, Poly, PolyMode, Silence, Voice};
pub use crate::param::{lfo, Automation, Clock, Curve, Lane, Lfo, Param, Shape};
pub use crate::song::{Section, Song, Track};
pub use crate::time::{bar, bars, beat, dotted, eighth, half, note, quarter, sixteenth, thirty_second, triplet, whole, Span, Time};
pub use crate::units::{Db, Units, Val};

#[allow(unused_imports)]
pub use crate::fx::prelude::*;
#[allow(unused_imports)]
pub use crate::io::prelude::*;
#[allow(unused_imports)]
pub use crate::pattern::prelude::*;
#[allow(unused_imports)]
pub use crate::sample::prelude::*;
#[allow(unused_imports)]
pub use crate::synth::prelude::*;
#[allow(unused_imports)]
pub use crate::theory::prelude::*;
