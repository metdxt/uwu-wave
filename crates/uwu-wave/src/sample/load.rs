//! Loading audio files into shareable [`Sample`]s, the global sample cache, asset discovery and
//! note-name parsing helpers.

use crate::Error;
use crate::dsp::Frame;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Decoded audio, always stereo, at its native sample rate. Shared via `Arc`.
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let s = Sample::load("piano/c1.wav").unwrap(); // relative paths also search the assets folder
/// println!("{} frames @ {} Hz", s.len(), s.sr);
/// ```
#[derive(Debug)]
pub struct Sample {
    /// Stereo frames (mono files are duplicated to both channels).
    pub frames: Vec<Frame>,
    /// Native sample rate (Hz).
    pub sr: f32,
    /// Root pitch (MIDI note number) if known.
    pub root: Option<f32>,
    /// Display name (usually the file name).
    pub name: String,
    /// Source file, if any.
    pub path: Option<PathBuf>,
    peak: f32,
    onset: usize,
    half: OnceLock<Arc<Sample>>,
}

impl Clone for Sample {
    fn clone(&self) -> Sample {
        Sample::from_frames(self.frames.clone(), self.sr)
            .named(self.name.clone())
            .with_root_opt(self.root)
    }
}

impl Sample {
    /// Build from frames.
    pub fn from_frames(frames: Vec<Frame>, sr: f32) -> Sample {
        let peak = frames.iter().fold(0f32, |m, f| m.max(f.peak()));
        let onset = find_onset(&frames, sr, peak);
        Sample {
            frames,
            sr,
            root: None,
            name: "sample".into(),
            path: None,
            peak,
            onset,
            half: OnceLock::new(),
        }
    }
    /// Build from a mono buffer.
    pub fn from_mono(samples: &[f32], sr: f32) -> Sample {
        Sample::from_frames(samples.iter().map(|&x| Frame::mono(x)).collect(), sr)
    }
    /// Set the root pitch.
    pub fn with_root(mut self, root: f32) -> Sample {
        self.root = Some(root);
        self
    }
    fn with_root_opt(mut self, root: Option<f32>) -> Sample {
        self.root = root;
        self
    }
    /// Set the display name.
    pub fn named(mut self, name: impl Into<String>) -> Sample {
        self.name = name.into();
        self
    }
    /// Load a WAV file through the global cache (each file is decoded once per process).
    /// Relative paths are tried against the current directory, then the assets folder
    /// (see [`assets_dir`]). Git-LFS pointer files are fetched transparently
    /// (see [`super::lfs`]).
    pub fn load(path: impl AsRef<Path>) -> Result<Arc<Sample>, Error> {
        let p = find_file(path.as_ref())?;
        let key = std::fs::canonicalize(&p).unwrap_or(p.clone());
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(s) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(s.clone());
        }
        let s = Arc::new(Sample::load_uncached(&p)?);
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, s.clone());
        Ok(s)
    }
    /// Decode a WAV file without touching the cache (8/16/24/32-bit int or 32-bit float,
    /// any channel count: mono is duplicated, extra channels are dropped).
    pub fn load_uncached(path: impl AsRef<Path>) -> Result<Sample, Error> {
        let path = path.as_ref();
        let real = super::lfs::resolve(path)?;
        let mut r = hound::WavReader::open(&real)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let k = 1.0 / (1u64 << (spec.bits_per_sample.clamp(8, 32) - 1)) as f64;
                r.samples::<i32>()
                    .map(|s| s.map(|v| (v as f64 * k) as f32))
                    .collect::<Result<_, _>>()?
            }
        };
        let frames: Vec<Frame> = raw
            .chunks_exact(ch)
            .map(|c| {
                if ch == 1 {
                    Frame::mono(c[0])
                } else {
                    Frame::new(c[0], c[1])
                }
            })
            .collect();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut s = Sample::from_frames(frames, spec.sample_rate as f32).named(name.clone());
        s.path = Some(path.to_path_buf());
        s.root = note_in_name(&name);
        Ok(s)
    }
    /// Number of frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Duration in seconds.
    pub fn duration(&self) -> f32 {
        self.frames.len() as f32 / self.sr
    }
    /// Peak absolute value.
    pub fn peak(&self) -> f32 {
        self.peak
    }
    /// Frame index where the sound actually starts (first frame above -34 dB of the peak, minus
    /// ~1.5 ms). Used to trim leading silence.
    pub fn onset(&self) -> usize {
        self.onset
    }
    /// A copy decimated by two (anti-alias filtered), used when repitching far upwards.
    /// Computed once and cached.
    pub fn half(&self) -> &Arc<Sample> {
        self.half.get_or_init(|| {
            let mut s = Sample::from_frames(decimate2(&self.frames), self.sr * 0.5)
                .named(self.name.clone());
            s.root = self.root;
            Arc::new(s)
        })
    }
    /// Frame with zero padding outside the buffer.
    #[inline]
    pub fn at(&self, i: isize) -> Frame {
        if i >= 0 && (i as usize) < self.frames.len() {
            self.frames[i as usize]
        } else {
            Frame::ZERO
        }
    }
    /// 4-point, 3rd-order Hermite interpolation at a fractional frame position.
    #[inline]
    pub fn hermite(&self, pos: f64) -> Frame {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let i = i as isize;
        let (x0, x1, x2, x3) = if i >= 1 && (i as usize) + 2 < self.frames.len() {
            let f = &self.frames[(i - 1) as usize..(i + 3) as usize];
            (f[0], f[1], f[2], f[3])
        } else {
            (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2))
        };
        Frame::new(
            hermite(x0.l, x1.l, x2.l, x3.l, t),
            hermite(x0.r, x1.r, x2.r, x3.r, t),
        )
    }
}

#[inline]
fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + x1
}

fn find_onset(frames: &[Frame], sr: f32, peak: f32) -> usize {
    if peak <= 0.0 {
        return 0;
    }
    let th = (peak * 0.02).max(1e-5);
    let i = frames.iter().position(|f| f.peak() > th).unwrap_or(0);
    i.saturating_sub((0.0015 * sr) as usize)
}

/// 2:1 decimation with a 63-tap Blackman-windowed sinc lowpass (passband to ~0.45·new Nyquist).
fn decimate2(x: &[Frame]) -> Vec<Frame> {
    const N: usize = 63;
    const M: isize = (N / 2) as isize;
    let fc = 0.225f64; // cycles per input sample
    let mut h = [0f32; N];
    let mut sum = 0.0;
    for (k, hk) in h.iter_mut().enumerate() {
        let n = k as f64 - M as f64;
        let sinc = if n == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * n).sin() / (std::f64::consts::PI * n)
        };
        let a = 2.0 * std::f64::consts::PI * k as f64 / (N - 1) as f64;
        let w = 0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos();
        *hk = (sinc * w) as f32;
        sum += sinc * w;
    }
    for hk in &mut h {
        *hk /= sum as f32;
    }
    let len = x.len() as isize;
    (0..x.len().div_ceil(2))
        .map(|j| {
            let c = 2 * j as isize;
            let mut acc = Frame::ZERO;
            for (k, &hk) in h.iter().enumerate() {
                let i = c + k as isize - M;
                if i >= 0 && i < len {
                    acc += x[i as usize] * hk;
                }
            }
            acc
        })
        .collect()
}

static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Sample>>>> = OnceLock::new();

/// Drop every cached sample (they stay alive while instruments hold them).
pub fn clear_cache() {
    if let Some(c) = CACHE.get() {
        c.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

// ---------------------------------------------------------------------------------------------
// asset discovery

/// The assets folder: `$UWU_ASSETS`, else the first `assets` directory found walking up from
/// the current directory, else walking up from this crate's manifest directory.
pub fn assets_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(p) = std::env::var_os("UWU_ASSETS").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let starts = [
            std::env::current_dir().ok(),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        ];
        for s in starts.into_iter().flatten() {
            for a in s.ancestors() {
                let cand = a.join("assets");
                if cand.is_dir() {
                    return Some(cand);
                }
            }
        }
        None
    })
    .clone()
}

/// `assets_dir()/rel` (or `rel` itself if no assets folder was found).
pub fn asset(rel: impl AsRef<Path>) -> PathBuf {
    match assets_dir() {
        Some(d) => d.join(rel),
        None => rel.as_ref().to_path_buf(),
    }
}

/// Find a file: as given, else relative to the assets folder (with or without a leading
/// `assets/`), else below `assets/wavebase/base`.
pub fn find_file(p: &Path) -> Result<PathBuf, Error> {
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if p.is_relative()
        && let Some(a) = assets_dir()
    {
        let stripped = p.strip_prefix("assets").unwrap_or(p);
        for cand in [
            a.join(stripped),
            a.join("wavebase").join(stripped),
            a.join("wavebase/base").join(stripped),
        ] {
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    Err(Error::Invalid(format!(
        "sample file not found: {} (assets folder: {})",
        p.display(),
        assets_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "not found; set UWU_ASSETS".into())
    )))
}

/// Find a directory like [`find_file`] does.
pub fn find_dir(p: &Path) -> Result<PathBuf, Error> {
    if p.is_dir() {
        return Ok(p.to_path_buf());
    }
    if p.is_relative()
        && let Some(a) = assets_dir()
    {
        let stripped = p.strip_prefix("assets").unwrap_or(p);
        for cand in [
            a.join(stripped),
            a.join("wavebase").join(stripped),
            a.join("wavebase/base").join(stripped),
        ] {
            if cand.is_dir() {
                return Ok(cand);
            }
        }
    }
    Err(Error::Invalid(format!(
        "sample folder not found: {} (assets folder: {})",
        p.display(),
        assets_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "not found; set UWU_ASSETS".into())
    )))
}

/// All `.wav` files of a directory, sorted by name.
pub fn wav_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|e| e.eq_ignore_ascii_case("wav"))
                    .unwrap_or(false)
        })
        .collect();
    v.sort();
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// note names

/// Parse a scientific note name: `C4` = 60, `A4` = 69, `F#2`, `Bb3`, `Ax2` (x = sharp, as in
/// the wavebase file names), `Cs4` (s = sharp), `C-1` = 0.
pub fn parse_note(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let pc = match b.first()?.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut i = 1;
    let mut acc = 0i32;
    while i < b.len() {
        match b[i] {
            b'#' | b'x' | b's' => acc += 1,
            b'b' => acc -= 1,
            _ => break,
        }
        i += 1;
    }
    let oct: i32 = s.get(i..)?.parse().ok()?;
    if !(-1..=9).contains(&oct) {
        return None;
    }
    let m = (oct + 1) * 12 + pc + acc;
    (0..=127).contains(&m).then_some(m as f32)
}

/// Find a note name inside a file name: the last `-`/`_`/space/`.`-separated token that parses
/// with [`parse_note`]. `"string-1-A-as-A2.wav"` → 45, `"Ax3-take-2.wav"` → 58,
/// `"string-1-note-A4-fret-05-1.wav"` → 69.
pub fn note_in_name(name: &str) -> Option<f32> {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    stem.split(['-', '_', ' ', '.']).rev().find_map(parse_note)
}

/// MIDI note of a file in `assets/piano`, whose names use Helmholtz notation:
///
/// | file | octave | MIDI |
/// |---|---|---|
/// | `A_2`, `B_2` | sub-contra | 21, 23 |
/// | `C_1` … `B_1` | contra | 24 … 35 |
/// | `C` … `B` | great | 36 … 47 |
/// | `cc` … `bb` (doubled: `c` would clash with `C` on case-insensitive filesystems) | small | 48 … 59 |
/// | `c1` … `b1` (= c¹ …) | one-line | 60 … 71 |
/// | `c2` … `c5` | | 72 … 108 |
///
/// A trailing `s` is sharp, `f` is flat (`A_2s` = 22, `Df` = 37). `cc1`, `dd1`, `ee1` (and
/// their sharps/flats) are byte-identical duplicates of `c1`, `d1`, `e1` (60, 62, 64).
pub fn piano_key(file_name: &str) -> Option<u8> {
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    let b = stem.as_bytes();
    let first = *b.first()?;
    let pc = match first.to_ascii_lowercase() {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let mut i = 1;
    let octave: i32;
    if first.is_ascii_uppercase() {
        if b.get(1) == Some(&b'_') {
            octave = match b.get(2)? {
                b'2' => 0,
                b'1' => 1,
                _ => return None,
            };
            i = 3;
        } else {
            octave = 2;
        }
    } else {
        if b.get(1) == Some(&first) {
            i = 2;
        }
        match b.get(i) {
            Some(d) if d.is_ascii_digit() => {
                octave = 3 + (d - b'0') as i32;
                i += 1;
            }
            _ => octave = 3,
        }
    }
    let acc = match &b[i..] {
        b"" => 0,
        b"s" => 1,
        b"f" => -1,
        _ => return None,
    };
    let m = (octave + 1) * 12 + pc + acc;
    (21..=108).contains(&m).then_some(m as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names() {
        assert_eq!(parse_note("C4"), Some(60.0));
        assert_eq!(parse_note("A4"), Some(69.0));
        assert_eq!(parse_note("Ax2"), Some(46.0));
        assert_eq!(parse_note("Bb3"), Some(58.0));
        assert_eq!(parse_note("as"), None);
        assert_eq!(parse_note("1"), None);
        assert_eq!(note_in_name("string-1-A-as-A2.wav"), Some(45.0));
        assert_eq!(note_in_name("Ax3-take-2.wav"), Some(58.0));
        assert_eq!(note_in_name("string-1-note-A4-fret-05-1.wav"), Some(69.0));
        assert_eq!(note_in_name("kick.wav"), None);
    }

    #[test]
    fn piano_names() {
        let cases = [
            ("A_2.wav", 21),
            ("A_2s.wav", 22),
            ("B_2f.wav", 22),
            ("B_2.wav", 23),
            ("C_1.wav", 24),
            ("B_1.wav", 35),
            ("C.wav", 36),
            ("Df.wav", 37),
            ("B.wav", 47),
            ("cc.wav", 48),
            ("ccs.wav", 49),
            ("ff.wav", 53),
            ("ffs.wav", 54),
            ("bb.wav", 59),
            ("c1.wav", 60),
            ("cc1.wav", 60),
            ("dd1f.wav", 61),
            ("a1.wav", 69),
            ("c2.wav", 72),
            ("a4.wav", 105),
            ("c5.wav", 108),
        ];
        for (n, m) in cases {
            assert_eq!(piano_key(n), Some(m), "{n}");
        }
        assert_eq!(piano_key("README.md"), None);
        assert_eq!(piano_key("Piano Keyboard.png"), None);
    }

    #[test]
    fn decodes_all_formats() {
        let dir = std::env::temp_dir().join(format!("uwu-sample-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (bits, fmt, ch) in [
            (8, hound::SampleFormat::Int, 1),
            (16, hound::SampleFormat::Int, 2),
            (24, hound::SampleFormat::Int, 1),
            (32, hound::SampleFormat::Int, 2),
            (32, hound::SampleFormat::Float, 1),
        ] {
            let p = dir.join(format!("t{bits}{ch}{:?}.wav", fmt));
            let spec = hound::WavSpec {
                channels: ch,
                sample_rate: 22050,
                bits_per_sample: bits,
                sample_format: fmt,
            };
            let mut w = hound::WavWriter::create(&p, spec).unwrap();
            for i in 0..100 {
                let x = if i == 10 { 0.5f64 } else { 0.0 };
                for _ in 0..ch {
                    match fmt {
                        hound::SampleFormat::Float => w.write_sample(x as f32).unwrap(),
                        hound::SampleFormat::Int => {
                            let v = (x * (1u64 << (bits - 1)) as f64) as i32;
                            match bits {
                                8 => w.write_sample(v as i8).unwrap(),
                                16 => w.write_sample(v as i16).unwrap(),
                                _ => w.write_sample(v).unwrap(),
                            }
                        }
                    }
                }
            }
            w.finalize().unwrap();
            let s = Sample::load_uncached(&p).unwrap();
            assert_eq!(s.len(), 100);
            assert_eq!(s.sr, 22050.0);
            assert!(
                (s.frames[10].l - 0.5).abs() < 0.01 && (s.frames[10].r - 0.5).abs() < 0.01,
                "{bits} {ch}: {:?}",
                s.frames[10]
            );
            assert_eq!(s.frames[11], Frame::ZERO);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hermite_reproduces_lines_and_decimation_keeps_dc() {
        let s = Sample::from_mono(
            &(0..64).map(|i| i as f32 * 0.01).collect::<Vec<_>>(),
            48000.0,
        );
        assert!((s.hermite(10.25).l - 0.1025).abs() < 1e-5);
        let dc = Sample::from_mono(&[0.5; 400], 48000.0);
        let h = dc.half();
        assert_eq!(h.sr, 24000.0);
        assert!((h.frames[100].l - 0.5).abs() < 1e-3);
    }
}
