//! Articulation patches: key/velocity-mapped sample zones loaded from the sound library
//! (`assets/library/<family>/<instrument>/<articulation>/zones.txt`) or built in code.

use crate::sample::Sample;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One sample zone of an [`ArticPatch`].
#[derive(Clone, Debug)]
pub struct ArticZone {
    pub sample: Arc<Sample>,
    /// MIDI note the sample was recorded at.
    pub root: f32,
    /// Key range (inclusive, MIDI).
    pub lo: f32,
    pub hi: f32,
    /// Velocity / dynamics range 0..1 (a *dynamic layer*).
    pub vlo: f32,
    pub vhi: f32,
    /// Linear gain.
    pub gain: f32,
    /// Fine tuning in semitones.
    pub tune: f32,
    /// Round-robin group (zones sharing key + layer with different `rr` alternate).
    pub rr: u32,
    /// Sustain loop (frames, end exclusive).
    pub loop_pts: Option<(usize, usize)>,
    /// Stack: zones of different stacks sound *together* (e.g. a patch layering two sample
    /// sets). A stack > 0 only plays where one of its zones covers the key.
    pub stack: u32,
    /// Playback start frame (`0` = the sample's detected onset).
    pub start: usize,
    /// Playback end frame (exclusive).
    pub end: usize,
    pub(crate) layer: usize,
}

impl ArticZone {
    /// A zone covering just its root key and the full dynamic range.
    pub fn new(sample: impl Into<Arc<Sample>>, root: f32) -> ArticZone {
        let sample: Arc<Sample> = sample.into();
        let end = sample.frames.len();
        ArticZone {
            start: 0,
            end,
            sample,
            root,
            lo: root,
            hi: root,
            vlo: 0.0,
            vhi: 1.0,
            gain: 1.0,
            tune: 0.0,
            rr: 0,
            loop_pts: None,
            stack: 0,
            layer: 0,
        }
    }
    pub fn keys(mut self, lo: f32, hi: f32) -> ArticZone {
        self.lo = lo.min(hi);
        self.hi = hi.max(lo);
        self
    }
    /// Dynamic layer (velocity range 0..1).
    pub fn layer(mut self, vlo: f32, vhi: f32) -> ArticZone {
        self.vlo = vlo;
        self.vhi = vhi;
        self
    }
    pub fn gain(mut self, g: f32) -> ArticZone {
        self.gain = g;
        self
    }
    pub fn rr(mut self, group: u32) -> ArticZone {
        self.rr = group;
        self
    }
    /// Stack index (see [`ArticZone::stack`]).
    pub fn stacked(mut self, stack: u32) -> ArticZone {
        self.stack = stack;
        self
    }
    /// Sustain loop in frames.
    pub fn looped(mut self, start: usize, end: usize) -> ArticZone {
        if end > start + 16 {
            self.loop_pts = Some((start, end));
        }
        self
    }
}

/// A set of zones for one articulation (e.g. "violins-1 / staccato"), with dynamic layers.
#[derive(Clone, Debug)]
pub struct ArticPatch {
    pub name: String,
    pub zones: Vec<ArticZone>,
    /// Amp attack (seconds).
    pub attack: f32,
    /// Release time (seconds to -60 dB).
    pub release: f32,
    /// Patch gain (linear).
    pub gain: f32,
    /// Patch tuning (semitones).
    pub tune: f32,
    /// Notes ignore note-off (percussion manifests set `@oneshot=1`).
    pub oneshot: bool,
    /// Loudness normalization factor (computed by [`ArticPatch::finish`]).
    pub norm: f32,
    /// Distinct dynamic layers, sorted soft → loud, as (vlo, vhi).
    pub layers: Vec<(f32, f32)>,
}

impl ArticPatch {
    pub fn new(name: impl Into<String>) -> ArticPatch {
        ArticPatch {
            name: name.into(),
            zones: vec![],
            attack: 0.005,
            release: 0.4,
            gain: 1.0,
            tune: 0.0,
            norm: 1.0,
            oneshot: false,
            layers: vec![(0.0, 1.0)],
        }
    }
    pub fn zone(mut self, z: ArticZone) -> ArticPatch {
        self.zones.push(z);
        self
    }
    pub fn attack(mut self, s: f32) -> ArticPatch {
        self.attack = s;
        self
    }
    pub fn release(mut self, s: f32) -> ArticPatch {
        self.release = s;
        self
    }

    /// Compute dynamic layers, zone→layer indices and the loudness normalization.
    /// Call after adding zones (loading does it for you).
    pub fn finish(mut self) -> ArticPatch {
        let mut layers: Vec<(f32, f32)> = vec![];
        for z in &self.zones {
            if !layers
                .iter()
                .any(|l| (l.0 - z.vlo).abs() < 1e-3 && (l.1 - z.vhi).abs() < 1e-3)
            {
                layers.push((z.vlo, z.vhi));
            }
        }
        layers.sort_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)));
        if layers.is_empty() {
            layers.push((0.0, 1.0));
        }
        for z in &mut self.zones {
            z.layer = layers
                .iter()
                .position(|l| (l.0 - z.vlo).abs() < 1e-3 && (l.1 - z.vhi).abs() < 1e-3)
                .unwrap_or(0);
        }
        self.layers = layers;
        let rms = self.measure_rms();
        self.norm = if rms > 1e-5 {
            (0.1 / rms).clamp(0.05, 30.0)
        } else {
            1.0
        };
        self
    }

    /// Centre (0..1) of every dynamic layer.
    pub fn centers(&self) -> Vec<f32> {
        self.layers.iter().map(|l| 0.5 * (l.0 + l.1)).collect()
    }

    /// Loudness of the layer closest to *mf*: the loudest 20 ms RMS window within ~0.8 s after
    /// the onset, averaged over ≤ 8 zones (×√stacks).
    pub fn measure_rms(&self) -> f32 {
        let c = self.centers();
        let mf = (0..c.len())
            .min_by(|&a, &b| (c[a] - 0.65).abs().total_cmp(&(c[b] - 0.65).abs()))
            .unwrap_or(0);
        let zs: Vec<&ArticZone> = self.zones.iter().filter(|z| z.layer == mf).collect();
        if zs.is_empty() {
            return 0.0;
        }
        let step = (zs.len() / 8).max(1);
        let (mut acc, mut n) = (0.0f64, 0usize);
        for z in zs.iter().step_by(step) {
            let s = &z.sample;
            let a = if z.start > 0 { z.start } else { s.onset() };
            let b = (a + (0.8 * s.sr) as usize).min(z.end);
            if b <= a {
                continue;
            }
            // loudest 20 ms window: works for sustains and decaying one-shots alike
            let win = ((0.02 * s.sr) as usize).max(16);
            let mut mx = 0.0f64;
            let mut i = a;
            while i + win <= b {
                let e: f64 = s.frames[i..i + win]
                    .iter()
                    .map(|f| (f.l * f.l + f.r * f.r) as f64 * 0.5)
                    .sum();
                mx = mx.max(e / win as f64);
                i += win;
            }
            acc += mx.sqrt() * z.gain as f64;
            n += 1;
        }
        // stacked sample sets sound together
        let stacks = self.stacks().len().max(1) as f64;
        if n == 0 {
            0.0
        } else {
            (acc / n as f64 * stacks.sqrt()) as f32
        }
    }

    /// Best zone for a pitch in a layer (nearest key range; round-robin among equals).
    pub fn pick(&self, pitch: f32, layer: usize, rr_counter: u32) -> Option<usize> {
        self.pick_stack(pitch, layer, 0, rr_counter)
    }

    /// The dynamic layers available for one key in one stack: `(layer centre 0..1, zone)`,
    /// soft → loud. Only zones at the best key distance are used (libraries often split
    /// velocity differently per note); round robins rotate with `rr_counter`.
    pub fn note_layers(&self, pitch: f32, stack: u32, rr_counter: u32) -> Vec<(f32, usize)> {
        let lowest = self.zones.iter().map(|z| z.stack).min().unwrap_or(0);
        let dist = |z: &ArticZone| {
            if pitch < z.lo - 0.5 {
                z.lo - pitch
            } else if pitch > z.hi + 0.5 {
                pitch - z.hi
            } else {
                0.0
            }
        };
        let cand: Vec<usize> = (0..self.zones.len())
            .filter(|&i| self.zones[i].stack == stack)
            .collect();
        let best = cand
            .iter()
            .map(|&i| dist(&self.zones[i]))
            .fold(f32::MAX, f32::min);
        if best == f32::MAX {
            return vec![];
        }
        if stack != lowest
            && best > 0.0
            && !cand.iter().any(|&i| {
                self.zones[i].lo == self.zones[i].hi && (self.zones[i].root - pitch).abs() <= 6.0
            })
        {
            return vec![];
        }
        let near: Vec<usize> = cand
            .into_iter()
            .filter(|&i| (dist(&self.zones[i]) - best).abs() < 1e-3)
            .collect();
        let rbest = near
            .iter()
            .map(|&i| (self.zones[i].root - pitch).abs())
            .fold(f32::MAX, f32::min);
        let near: Vec<usize> = near
            .into_iter()
            .filter(|&i| ((self.zones[i].root - pitch).abs() - rbest).abs() < 1e-3)
            .collect();
        let mut groups: Vec<((f32, f32), Vec<usize>)> = vec![];
        for i in near {
            let z = &self.zones[i];
            match groups
                .iter_mut()
                .find(|g| (g.0.0 - z.vlo).abs() < 1e-3 && (g.0.1 - z.vhi).abs() < 1e-3)
            {
                Some(g) => g.1.push(i),
                None => groups.push(((z.vlo, z.vhi), vec![i])),
            }
        }
        let mut out: Vec<(f32, usize)> = groups
            .into_iter()
            .map(|((a, b), v)| (0.5 * (a + b), v[rr_counter as usize % v.len()]))
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// Distinct stacks, ascending.
    pub fn stacks(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.zones.iter().map(|z| z.stack).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Like [`ArticPatch::pick`] within one stack. Stacks other than the lowest only answer
    /// when a zone covers the key (or its root is within ±6 st when it has no key range).
    pub fn pick_stack(
        &self,
        pitch: f32,
        layer: usize,
        stack: u32,
        rr_counter: u32,
    ) -> Option<usize> {
        let lowest = self.zones.iter().map(|z| z.stack).min().unwrap_or(0);
        let mut best = f32::MAX;
        for z in self
            .zones
            .iter()
            .filter(|z| z.layer == layer && z.stack == stack)
        {
            let d = if pitch < z.lo - 0.5 {
                z.lo - pitch
            } else if pitch > z.hi + 0.5 {
                pitch - z.hi
            } else {
                0.0
            };
            best = best.min(d);
        }
        if best == f32::MAX {
            return None;
        }
        if stack != lowest && best > 0.0 {
            let near = self.zones.iter().any(|z| {
                z.layer == layer
                    && z.stack == stack
                    && z.lo == z.hi
                    && (z.root - pitch).abs() <= 6.0
            });
            if !near {
                return None;
            }
        }
        // all zones at that distance
        let cands: Vec<usize> = self
            .zones
            .iter()
            .enumerate()
            .filter(|(_, z)| z.layer == layer && z.stack == stack)
            .filter(|(_, z)| {
                let d = if pitch < z.lo - 0.5 {
                    z.lo - pitch
                } else if pitch > z.hi + 0.5 {
                    pitch - z.hi
                } else {
                    0.0
                };
                (d - best).abs() < 1e-3
            })
            .map(|(i, _)| i)
            .collect();
        if cands.len() == 1 {
            return Some(cands[0]);
        }
        // prefer the closest root, then rotate round robins
        let rbest = cands
            .iter()
            .map(|&i| (self.zones[i].root - pitch).abs())
            .fold(f32::MAX, f32::min);
        let rr: Vec<usize> = cands
            .into_iter()
            .filter(|&i| ((self.zones[i].root - pitch).abs() - rbest).abs() < 1e-3)
            .collect();
        Some(rr[rr_counter as usize % rr.len()])
    }

    /// Load a library patch by id (`"strings/violins-1/staccato"`) or a directory path that
    /// contains a `zones.txt`.
    pub fn load(id: &str) -> Result<ArticPatch, String> {
        let dir = resolve_dir(id).ok_or_else(|| {
            format!(
                "no articulation dir with zones.txt for {id:?} (library: {})",
                library_root().display()
            )
        })?;
        load_dir(&dir, id)
    }

    /// Playable key range (lowest lo, highest hi).
    pub fn range(&self) -> (f32, f32) {
        let lo = self.zones.iter().map(|z| z.lo).fold(f32::MAX, f32::min);
        let hi = self.zones.iter().map(|z| z.hi).fold(f32::MIN, f32::max);
        if lo > hi { (60.0, 60.0) } else { (lo, hi) }
    }

    /// Is any zone looped (sustained instrument)?
    pub fn has_loops(&self) -> bool {
        self.zones.iter().any(|z| z.loop_pts.is_some())
    }

    /// Longest zone duration (s).
    pub fn max_len(&self) -> f32 {
        self.zones
            .iter()
            .map(|z| z.sample.duration())
            .fold(0.0, f32::max)
    }
}

/// Library roots searched for instruments, highest priority first (the sample library's
/// roots: `$UWU_LIBRARY` entries, else the git-ignored `<assets>/local` overlay, then the
/// redistributable `<assets>/library`).
pub fn library_roots() -> Vec<PathBuf> {
    crate::sample::library::library_roots()
}

/// The base (last) library root, for messages.
pub fn library_root() -> PathBuf {
    library_roots()
        .pop()
        .unwrap_or_else(|| PathBuf::from("assets/library"))
}

fn has_artics(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| e.path().join("zones.txt").is_file()))
        .unwrap_or(false)
}

/// Where an instrument (`"family/inst"`) lives: the first root that has it (an overlay shadows
/// the whole instrument), plus that root's index (0 = highest priority).
pub fn instrument_location(instrument: &str) -> Option<(PathBuf, usize)> {
    let p = Path::new(instrument);
    if p.is_absolute() && has_artics(p) {
        return Some((p.to_path_buf(), 0));
    }
    library_roots()
        .into_iter()
        .enumerate()
        .map(|(i, r)| (r.join(instrument), i))
        .find(|(d, _)| has_artics(d))
}

fn resolve_dir(id: &str) -> Option<PathBuf> {
    let p = Path::new(id);
    if p.join("zones.txt").is_file() {
        return Some(p.to_path_buf());
    }
    let (inst, artic) = id.rsplit_once('/')?;
    let (d, _) = instrument_location(inst)?;
    let d = d.join(artic);
    d.join("zones.txt").is_file().then_some(d)
}

/// Articulation sub-directories (with a `zones.txt`) of a library instrument.
pub fn instrument_dirs(instrument: &str) -> Vec<(String, PathBuf)> {
    let Some((base, _)) = instrument_location(instrument) else {
        return vec![];
    };
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join("zones.txt").is_file() {
                v.push((e.file_name().to_string_lossy().to_string(), p));
            }
        }
    }
    v.sort();
    v
}

fn dyn_label(v: &str) -> Option<(f32, f32)> {
    Some(match v {
        "ppp" => (0.0, 0.15),
        "pp" => (0.0, 0.3),
        "p" => (0.25, 0.45),
        "mp" => (0.4, 0.6),
        "mf" => (0.55, 0.72),
        "f" => (0.7, 0.88),
        "ff" => (0.85, 1.0),
        "fff" => (0.92, 1.0),
        _ => return None,
    })
}

/// Parse a `zones.txt` articulation directory (the library manifest format, see
/// [`crate::sample::library::Manifest`]). Remote zones are downloaded on first load.
pub fn load_dir(dir: &Path, name: &str) -> Result<ArticPatch, String> {
    use crate::sample::library::Manifest;
    let text = std::fs::read_to_string(dir.join("zones.txt"))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let m = Manifest::parse(&text).map_err(|e| format!("{}: {e}", dir.display()))?;
    let hnum = |k: &str| m.get(k).and_then(|v| v.parse::<f32>().ok());
    let mut p = ArticPatch::new(name);
    p.attack = hnum("attack").unwrap_or(p.attack);
    p.release = hnum("release").unwrap_or(p.release);
    p.gain = crate::units::db_to_gain(hnum("gain_db").unwrap_or(0.0));
    p.tune = hnum("tune").unwrap_or(0.0);
    p.oneshot = m.get("oneshot").map(|v| v != "0").unwrap_or(false);
    let loops = m.get("loop").map(|v| v != "0").unwrap_or(true);
    // remote (download-on-first-use) one-shot folders can hold dozens of hits: fetch at most
    // MAX_REMOTE of them so a fallback preset loads quickly
    const MAX_REMOTE: usize = 8;
    let mut remote = 0;
    for spec in &m.zones {
        if spec.url.is_some() && !dir.join(&spec.file).is_file() {
            if remote >= MAX_REMOTE {
                continue;
            }
            remote += 1;
        }
        let zone = spec.to_zone(dir, loops);
        let Some(sample) = zone.sample() else {
            continue;
        };
        let sr = sample.sr;
        let n = sample.frames.len();
        let (mut vlo, mut vhi) = (spec.vlo.min(spec.vhi), spec.vlo.max(spec.vhi));
        let explicit_v = spec.vlo != 0.0 || spec.vhi != 1.0;
        if !explicit_v && let Some((a, b)) = spec.dynamic.as_deref().and_then(dyn_label) {
            vlo = a;
            vhi = b;
        }
        let mut z = ArticZone::new(sample, spec.root)
            .keys(spec.lo.unwrap_or(spec.root), spec.hi.unwrap_or(spec.root))
            .layer(vlo, vhi)
            .gain(crate::units::db_to_gain(spec.gain_db))
            .rr(spec.rr)
            .stacked(spec.layer as u32);
        z.tune = spec.tune;
        z.start = ((zone.region.start.max(0.0) * sr) as usize).min(n);
        z.end = zone
            .region
            .end
            .map(|e| ((e * sr) as usize).min(n))
            .unwrap_or(n)
            .max(z.start);
        if let Some((a, b)) = zone.region.looped {
            let end = z.end;
            z = z.looped((a * sr) as usize, ((b * sr) as usize).min(end));
        }
        p.zones.push(z);
    }
    if p.zones.is_empty() {
        return Err(format!(
            "{}: zones.txt has no loadable zones",
            dir.display()
        ));
    }
    Ok(p.finish())
}

/// Test helper: a synthetic sample (sine at `hz`, optional loud attack burst).
#[cfg(test)]
pub(crate) fn synth_sample(hz: f32, amp: f32, secs: f32, burst: bool) -> Sample {
    let sr = 48_000.0;
    let n = (secs * sr) as usize;
    use crate::dsp::Frame;
    let mut v = Vec::with_capacity(n);
    let mut seed = 1u32;
    for i in 0..n {
        let t = i as f32 / sr;
        let mut x = (std::f32::consts::TAU * hz * t).sin() * amp;
        if burst && t < 0.04 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x += ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.9;
        }
        // tiny fade in to avoid a click at 0
        let f = (i as f32 / 48.0).min(1.0);
        v.push(Frame::mono(x * f));
    }
    Sample::from_frames(v, sr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_and_pick() {
        let a = Arc::new(synth_sample(220.0, 0.2, 0.5, false));
        let b = Arc::new(synth_sample(220.0, 0.8, 0.5, false));
        let p = ArticPatch::new("t")
            .zone(
                ArticZone::new(b.clone(), 57.0)
                    .keys(55.0, 59.0)
                    .layer(0.5, 1.0),
            )
            .zone(
                ArticZone::new(a.clone(), 57.0)
                    .keys(55.0, 59.0)
                    .layer(0.0, 0.5),
            )
            .zone(
                ArticZone::new(a.clone(), 64.0)
                    .keys(60.0, 66.0)
                    .layer(0.0, 0.5)
                    .rr(1),
            )
            .zone(
                ArticZone::new(a, 64.0)
                    .keys(60.0, 66.0)
                    .layer(0.0, 0.5)
                    .rr(2),
            )
            .finish();
        assert_eq!(p.layers.len(), 2);
        assert_eq!(p.zones[0].layer, 1);
        assert_eq!(p.pick(57.0, 0, 0), Some(1));
        assert_eq!(p.pick(57.0, 1, 0), Some(0));
        assert_ne!(p.pick(64.0, 0, 0), p.pick(64.0, 0, 1)); // round robin
        assert_eq!(p.pick(90.0, 0, 0).map(|i| p.zones[i].root), Some(64.0)); // nearest
        assert!(p.norm > 0.0);
    }

    #[test]
    fn parse_zones_txt() {
        let dir = std::env::temp_dir().join(format!("uwu-orch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = synth_sample(440.0, 0.5, 0.3, false);
        let mut w = hound::WavWriter::create(
            dir.join("a4.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for f in &s.frames {
            w.write_sample((f.l * 32000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(dir.join("zones.txt"), "# test\n@attack=0.02 @release=1.5\nfile=a4.wav root=A4 lo=67 hi=71 dyn=f\nfile=missing.wav root=60\n").unwrap();
        let p = load_dir(&dir, "t").unwrap();
        assert_eq!(p.zones.len(), 1);
        assert_eq!(p.zones[0].root, 69.0);
        assert_eq!((p.zones[0].lo, p.zones[0].hi), (67.0, 71.0));
        assert!((p.zones[0].vlo - 0.7).abs() < 1e-6);
        assert_eq!(p.release, 1.5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
