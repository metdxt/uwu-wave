//! WAV writing.

use crate::Error;
use crate::dsp::{Frame, Rng};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    Int16,
    Int24,
    Float32,
}

/// Write stereo audio. Integer formats get TPDF dither and hard clipping at full scale.
pub fn write_wav(
    path: impl AsRef<Path>,
    sr: u32,
    audio: &[Frame],
    bits: BitDepth,
) -> Result<(), Error> {
    if let Some(dir) = path.as_ref().parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sr,
        bits_per_sample: match bits {
            BitDepth::Int16 => 16,
            BitDepth::Int24 => 24,
            BitDepth::Float32 => 32,
        },
        sample_format: if bits == BitDepth::Float32 {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    match bits {
        BitDepth::Float32 => {
            for f in audio {
                w.write_sample(f.l)?;
                w.write_sample(f.r)?;
            }
        }
        BitDepth::Int16 | BitDepth::Int24 => {
            let max = if bits == BitDepth::Int16 {
                32767.0
            } else {
                8_388_607.0
            };
            let mut rng = Rng::new(7);
            for f in audio {
                for x in [f.l, f.r] {
                    let d = (rng.f32() - rng.f32()) / max as f32;
                    let v = ((x + d).clamp(-1.0, 1.0) as f64 * max).round() as i32;
                    w.write_sample(v)?;
                }
            }
        }
    }
    w.finalize()?;
    Ok(())
}
