//! The [`Sampler`] instrument: multi-zone, round-robin, velocity-layered sample playback.

use super::load::{Sample, find_dir, find_file, note_in_name, parse_note, wav_files};
use super::voice::{Fader, Play, Region};
use crate::ctx::Ctx;
use crate::dsp::{Adsr, Frame, Ramp, Svf, midi_to_hz};
use crate::instrument::{Instrument, NoteOn, Poly, Voice};
use crate::param::Param;
use crate::units::Val;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

/// Something usable as a root pitch: a MIDI number (`60`, `57.5`) or a note name (`"C4"`, `"F#2"`).
pub trait RootPitch {
    fn midi(self) -> Option<f32>;
}
macro_rules! root_num { ($($t:ty),*) => { $(impl RootPitch for $t { fn midi(self) -> Option<f32> { Some(self as f32) } })* } }
root_num!(i32, i64, u8, u32, usize, f32, f64);
impl RootPitch for &str {
    fn midi(self) -> Option<f32> {
        parse_note(self).or_else(|| self.parse().ok())
    }
}
impl RootPitch for String {
    fn midi(self) -> Option<f32> {
        self.as_str().midi()
    }
}
impl<P: RootPitch> RootPitch for Option<P> {
    fn midi(self) -> Option<f32> {
        self.and_then(|p| p.midi())
    }
}

/// Lazily loaded sample source shared by zone clones.
pub(crate) struct Src {
    path: Option<PathBuf>,
    cell: OnceLock<Option<Arc<Sample>>>,
}

impl Src {
    pub(crate) fn path(p: PathBuf) -> Arc<Src> {
        Arc::new(Src {
            path: Some(p),
            cell: OnceLock::new(),
        })
    }
    pub(crate) fn sample(s: Arc<Sample>) -> Arc<Src> {
        let cell = OnceLock::new();
        let _ = cell.set(Some(s));
        Arc::new(Src { path: None, cell })
    }
    /// Load on first use (errors are reported once and the source stays silent).
    pub(crate) fn get(&self) -> Option<&Arc<Sample>> {
        self.cell
            .get_or_init(|| {
                let p = self.path.as_ref()?;
                match Sample::load(p) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("uwu-wave: sample {}: {e}", p.display());
                        None
                    }
                }
            })
            .as_ref()
    }
}

/// One mapped sample: key range, velocity range, root pitch, gain and playback region.
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let z = Zone::new("piano/c1.wav", "C4").keys(58, 62).vel(0.0, 0.7).gain(-3.db());
/// let lead = Sampler::new().zone(z).release(0.3);
/// ```
#[derive(Clone)]
pub struct Zone {
    pub(crate) src: Arc<Src>,
    /// Inclusive key range (MIDI). `None` = chosen by nearest root.
    pub keys: Option<(f32, f32)>,
    /// Inclusive velocity range (0..1).
    pub vel: (f32, f32),
    /// Pitch at which the sample plays unaltered.
    pub root: f32,
    /// Linear gain.
    pub gain: f32,
    /// Fine tune in semitones.
    pub tune: f32,
    /// Stereo balance -1..1.
    pub pan: f32,
    /// Playback region.
    pub region: Region,
}

impl Zone {
    /// A zone for a file (loaded lazily on first use). `root` is a MIDI number or note name.
    /// Panics if the root is an unparsable note name.
    pub fn new(path: impl AsRef<Path>, root: impl RootPitch) -> Zone {
        let root = root
            .midi()
            .unwrap_or_else(|| panic!("uwu-wave: bad root pitch for {}", path.as_ref().display()));
        let p = find_file(path.as_ref()).unwrap_or_else(|_| path.as_ref().to_path_buf());
        Zone::with_src(Src::path(p), root)
    }
    /// A zone for an in-memory sample. Uses the sample's root if `root` is `None`.
    pub fn from_sample(s: impl Into<Arc<Sample>>, root: impl RootPitch) -> Zone {
        let s = s.into();
        let r = root.midi().or(s.root).unwrap_or(60.0);
        Zone::with_src(Src::sample(s), r)
    }
    pub(crate) fn with_src(src: Arc<Src>, root: f32) -> Zone {
        Zone {
            src,
            keys: None,
            vel: (0.0, 1.0),
            root,
            gain: 1.0,
            tune: 0.0,
            pan: 0.0,
            region: Region::default(),
        }
    }
    /// Restrict to keys `lo..=hi`.
    pub fn keys(mut self, lo: impl Val, hi: impl Val) -> Zone {
        let (a, b) = (lo.val(), hi.val());
        self.keys = Some((a.min(b), a.max(b)));
        self
    }
    /// Restrict to a single key.
    pub fn key(self, k: impl Val) -> Zone {
        let k = k.val();
        self.keys(k, k)
    }
    /// Restrict to velocities `lo..=hi` (0..1) — stack zones to build velocity layers.
    pub fn vel(mut self, lo: f32, hi: f32) -> Zone {
        self.vel = (lo.min(hi), lo.max(hi));
        self
    }
    /// Gain (linear, or `-3.db()`).
    pub fn gain(mut self, g: impl Val) -> Zone {
        self.gain = g.val();
        self
    }
    /// Fine tune (semitones).
    pub fn tune(mut self, st: f32) -> Zone {
        self.tune = st;
        self
    }
    /// Stereo balance -1..1.
    pub fn pan(mut self, p: f32) -> Zone {
        self.pan = p;
        self
    }
    /// Start offset (seconds).
    pub fn start(mut self, secs: f32) -> Zone {
        self.region.start = secs;
        self
    }
    /// End point (seconds).
    pub fn end(mut self, secs: f32) -> Zone {
        self.region.end = Some(secs);
        self
    }
    /// Loop between two points (seconds) while the note sounds.
    pub fn looped(mut self, start: f32, end: f32) -> Zone {
        self.region.looped = Some((start, end));
        self
    }
    /// Loop crossfade (seconds, default 10 ms).
    pub fn xfade(mut self, secs: f32) -> Zone {
        self.region.xfade = secs;
        self
    }
    /// Play backwards.
    pub fn reverse(mut self) -> Zone {
        self.region.reverse = true;
        self
    }
    /// Skip leading silence.
    pub fn trim(mut self, on: bool) -> Zone {
        self.region.trim = on;
        self
    }
    /// The loaded sample (loads it now).
    pub fn sample(&self) -> Option<Arc<Sample>> {
        self.src.get().cloned()
    }
    fn in_vel(&self, v: f32) -> bool {
        v >= self.vel.0 - 1e-6 && v <= self.vel.1 + 1e-6
    }
    fn score(&self, pitch: f32) -> f32 {
        match self.keys {
            Some((lo, hi)) => {
                let k = pitch.round();
                if k >= lo && k <= hi {
                    0.0
                } else {
                    1000.0 + (lo - k).max(k - hi)
                }
            }
            None => (self.root - pitch).abs(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Cfg {
    attack: Param,
    decay: Param,
    sustain: Param,
    release: Param,
    gain: Param,
    tune: Param,
    cutoff: Option<Param>,
    tone: f32,
    vel_sens: f32,
    vel_curve: f32,
    oneshot: bool,
    normalize: Option<f32>,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg {
            attack: Param::Const(0.001),
            decay: Param::Const(0.0),
            sustain: Param::Const(1.0),
            release: Param::Const(0.15),
            gain: Param::Const(1.0),
            tune: Param::Const(0.0),
            cutoff: None,
            tone: 0.0,
            vel_sens: 1.0,
            vel_curve: 1.3,
            oneshot: false,
            normalize: None,
        }
    }
}

pub(crate) struct Shared {
    zones: Vec<Zone>,
    cfg: Cfg,
    rr: Vec<AtomicU32>,
}

impl Shared {
    /// Pick a zone: velocity filter, best key/root match (ties prefer the zone above, which
    /// repitches downwards), then round-robin among equivalent zones.
    fn select(&self, pitch: f32, vel: f32) -> Option<usize> {
        let z = &self.zones;
        if z.is_empty() {
            return None;
        }
        let any_vel = z.iter().any(|x| x.in_vel(vel));
        let ok = |x: &Zone| !any_vel || x.in_vel(vel);
        let mut best: Option<usize> = None;
        let mut best_score = f32::INFINITY;
        for (i, x) in z.iter().enumerate() {
            if !ok(x) {
                continue;
            }
            let s = x.score(pitch);
            if s < best_score - 1e-4
                || (s <= best_score + 1e-4
                    && best.map(|b| x.root > z[b].root + 1e-4).unwrap_or(true))
            {
                best_score = best_score.min(s);
                best = Some(i);
            }
        }
        let b = &z[best?];
        let same = |x: &Zone| {
            ok(x) && (x.root - b.root).abs() < 1e-4 && x.keys == b.keys && x.vel == b.vel
        };
        let n = z.iter().filter(|x| same(x)).count() as u32;
        if n <= 1 {
            return best;
        }
        let slot = (b.root.round().clamp(0.0, 127.0)) as usize;
        let k = self.rr[slot].fetch_add(1, Ordering::Relaxed) % n;
        z.iter()
            .enumerate()
            .filter(|(_, x)| same(x))
            .nth(k as usize)
            .map(|(i, _)| i)
    }
}

/// A [`Sampler`] voice (used through [`Poly`]).
#[derive(Clone, Default)]
pub struct SamplerVoice {
    shared: Option<Arc<Shared>>,
    sr: f32,
    play: Play,
    fade: Fader,
    env: Adsr,
    base_step: f64,
    amp: f32,
    gl: f32,
    gr: f32,
    gain: Ramp,
    filt: [Svf; 2],
    filt_on: bool,
    vel_cut: f32,
    f0: f32,
    oneshot: bool,
}

impl Voice for SamplerVoice {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn start(&mut self, n: &NoteOn, ctx: &Ctx) {
        let Some(sh) = self.shared.clone() else {
            return;
        };
        let sr = if self.sr > 0.0 { self.sr } else { ctx.sr };
        self.sr = sr;
        // de-click whatever was playing
        if self.play.is_playing() {
            let g = self.env.level() * self.amp;
            self.fade.begin(&mut self.play, g, 0.004, sr);
        }
        let Some(zi) = sh.select(n.pitch, n.vel) else {
            return;
        };
        let z = &sh.zones[zi];
        let Some(smp) = z.src.get() else { return };
        let c = &sh.cfg;
        let ratio = 2f64.powf(((n.pitch - z.root + z.tune) / 12.0) as f64);
        self.play = Play::start(smp, &z.region, ratio, sr);
        self.base_step = self.play.step();
        let v = n.vel.clamp(0.0, 1.0);
        self.amp = z.gain * ((1.0 - c.vel_sens) + c.vel_sens * v.powf(c.vel_curve));
        if let Some(target) = c.normalize {
            self.amp *= target / smp.peak().max(1e-4);
        }
        let (gl, gr) = crate::dsp::pan_gains(z.pan);
        // balance law: keep center at unity
        self.gl = gl.min(1.0);
        self.gr = gr.min(1.0);
        self.env = Adsr::new(
            ctx.p(&c.attack).max(0.0),
            ctx.p(&c.decay).max(0.0),
            ctx.p(&c.sustain).clamp(0.0, 1.0),
            ctx.p(&c.release).max(0.0),
        );
        self.env.reset();
        self.env.gate_on(sr);
        self.oneshot = c.oneshot;
        self.f0 = midi_to_hz(n.pitch);
        self.filt_on = c.tone > 0.0 || c.cutoff.is_some();
        self.vel_cut = 20000.0 * 2f32.powf(-c.tone * (1.0 - v) * 5.0);
        self.filt = [Svf::default(); 2];
        self.gain = Ramp::default();
    }
    fn release(&mut self, _ctx: &Ctx) {
        if !self.oneshot {
            self.env.gate_off(self.sr);
        }
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool {
        let (gl, gr) = (self.gl, self.gr);
        self.fade.render(out, gl, gr);
        let Some(sh) = self.shared.as_ref() else {
            return false;
        };
        if !self.play.is_playing() {
            return self.fade.active();
        }
        let c = &sh.cfg;
        self.gain.start(ctx.p(&c.gain) * self.amp, out.len());
        if !c.tune.is_const() || c.tune.as_const() != Some(0.0) {
            self.play
                .set_step(self.base_step * 2f64.powf(ctx.p(&c.tune) as f64 / 12.0));
        }
        if self.filt_on {
            let mut cut = self.vel_cut.max(self.f0 * 3.0);
            if let Some(p) = &c.cutoff {
                cut = cut.min(ctx.p(p));
            }
            let cut = cut.clamp(20.0, self.sr * 0.45);
            for f in &mut self.filt {
                f.set(cut, 0.6, self.sr);
            }
        }
        for f in out.iter_mut() {
            let e = self.env.next();
            let Some(mut x) = self.play.next() else { break };
            if self.filt_on {
                x = Frame::new(self.filt[0].lp(x.l), self.filt[1].lp(x.r));
            }
            let g = e * self.gain.next();
            f.l += x.l * g * gl;
            f.r += x.r * g * gr;
            if self.env.is_idle() {
                self.play.stop();
                break;
            }
        }
        self.play.is_playing() || self.fade.active()
    }
}

/// A multi-sample instrument.
///
/// * zones map key/velocity ranges to samples; notes pick the best zone (exact key range, else
///   nearest root) and repitch it with Hermite interpolation (sample-rate conversion included)
/// * equivalent zones (same root/keys/velocity) alternate round-robin
/// * gated (ADSR, default) or one-shot playback, start offset, reverse, crossfaded loops
/// * velocity → gain curve and optional velocity → brightness (`.tone()`)
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let song = Song::new("keys");
/// let piano = song.track("piano", Sampler::piano().release(0.4).gain(-3.db()));
/// let vox = song.track("vox", Sampler::one_shot("my/chop.wav").reverse());
/// let mine = Sampler::new()
///     .zone(Zone::new("a.wav", "C3").keys(0, 54))
///     .zone(Zone::new("b.wav", "C4").keys(55, 127))
///     .adsr(0.005, 0.2, 0.8, 0.5);
/// let strat = Sampler::from_dir("wavebase/base/guitar/stratocaster", |name| uwu_wave::sample::note_in_name(name));
/// ```
pub struct Sampler {
    zones: Vec<Zone>,
    cfg: Cfg,
    defaults: Region,
    poly: Poly<SamplerVoice>,
    preload: bool,
    name: String,
}

impl Sampler {
    /// Empty sampler (32 voices). Add zones with [`Sampler::zone`].
    pub fn new() -> Sampler {
        Sampler {
            zones: vec![],
            cfg: Cfg::default(),
            defaults: Region::default(),
            poly: Poly::new(SamplerVoice::default()).voices(32),
            preload: false,
            name: "sampler".into(),
        }
    }
    /// One sample across the whole keyboard (root C4 = original pitch), played to its end
    /// regardless of note length.
    pub fn one_shot(path: impl AsRef<Path>) -> Sampler {
        let p = path.as_ref();
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "one-shot".into());
        Sampler::new().zone(Zone::new(p, 60)).oneshot().named(name)
    }
    /// A one-shot from an in-memory sample.
    pub fn from_sample(s: impl Into<Arc<Sample>>) -> Sampler {
        Sampler::new().zone(Zone::from_sample(s, 60)).oneshot()
    }
    /// Every `.wav` in `dir` (relative paths also search the assets folder) whose file name maps
    /// to a pitch. Several files with the same pitch become round-robin alternatives.
    /// Panics if the folder doesn't exist or contains no mapped file.
    pub fn from_dir<P: RootPitch>(
        dir: impl AsRef<Path>,
        pitch_of: impl Fn(&str) -> Option<P>,
    ) -> Sampler {
        let d = find_dir(dir.as_ref()).unwrap_or_else(|e| panic!("uwu-wave: {e}"));
        let files = wav_files(&d).unwrap_or_else(|e| panic!("uwu-wave: {}: {e}", d.display()));
        let mut s = Sampler::new().named(
            d.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        for f in files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(p) = pitch_of(&name).and_then(|p| p.midi()) {
                s = s.zone(Zone::with_src(Src::path(f), p));
            }
        }
        if s.zones.is_empty() {
            panic!("uwu-wave: no sample in {} matched a pitch", d.display());
        }
        s
    }
    /// [`Sampler::from_dir`] with pitches parsed from note names in the file names
    /// (`C4`, `F#2`, `Ax3`... see [`note_in_name`]).
    pub fn dir(dir: impl AsRef<Path>) -> Sampler {
        Sampler::from_dir(dir, note_in_name)
    }
    /// Add a zone (the sampler's default region settings are applied unless the zone set its own).
    pub fn zone(mut self, mut z: Zone) -> Sampler {
        if z.region == Region::default() {
            z.region = self.defaults;
        }
        self.zones.push(z);
        self
    }
    /// Shortcut: add a zone for `path` with `root`.
    pub fn sample(self, path: impl AsRef<Path>, root: impl RootPitch) -> Sampler {
        self.zone(Zone::new(path, root))
    }
    /// The zones.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }
    /// Mutable access to the zones (e.g. to retune or rebalance individual samples).
    pub fn zones_mut(&mut self) -> &mut [Zone] {
        &mut self.zones
    }
    fn region(mut self, f: impl Fn(&mut Region)) -> Sampler {
        f(&mut self.defaults);
        for z in &mut self.zones {
            f(&mut z.region);
        }
        self
    }
    /// Attack time (seconds).
    pub fn attack(mut self, s: impl Into<Param>) -> Sampler {
        self.cfg.attack = s.into();
        self
    }
    /// Decay time (seconds) towards the sustain level.
    pub fn decay(mut self, s: impl Into<Param>) -> Sampler {
        self.cfg.decay = s.into();
        self
    }
    /// Sustain level 0..1 (1 = let the sample decay naturally).
    pub fn sustain(mut self, l: impl Into<Param>) -> Sampler {
        self.cfg.sustain = l.into();
        self
    }
    /// Release time (seconds) after note-off.
    pub fn release(mut self, s: impl Into<Param>) -> Sampler {
        self.cfg.release = s.into();
        self
    }
    /// All four envelope settings. Evaluated at each note start, so they can be automated.
    pub fn adsr(
        self,
        a: impl Into<Param>,
        d: impl Into<Param>,
        s: impl Into<Param>,
        r: impl Into<Param>,
    ) -> Sampler {
        self.attack(a).decay(d).sustain(s).release(r)
    }
    /// Output gain (linear or `-6.db()`), automatable.
    pub fn gain(mut self, g: impl Into<Param>) -> Sampler {
        self.cfg.gain = g.into();
        self
    }
    /// Pitch offset in semitones, automatable (per block).
    pub fn tune(mut self, st: impl Into<Param>) -> Sampler {
        self.cfg.tune = st.into();
        self
    }
    /// Low-pass cutoff (Hz), automatable.
    pub fn cutoff(mut self, hz: impl Into<Param>) -> Sampler {
        self.cfg.cutoff = Some(hz.into());
        self
    }
    /// Velocity → brightness amount 0..1 (soft notes get darker, like real instruments).
    pub fn tone(mut self, amount: f32) -> Sampler {
        self.cfg.tone = amount.clamp(0.0, 1.0);
        self
    }
    /// Velocity sensitivity 0..1 (0 = every note at full level).
    pub fn velocity(mut self, sens: f32) -> Sampler {
        self.cfg.vel_sens = sens.clamp(0.0, 1.0);
        self
    }
    /// Velocity → gain curve exponent (gain = vel^k; default 1.3, higher = more dynamic).
    pub fn velocity_curve(mut self, k: f32) -> Sampler {
        self.cfg.vel_curve = k.max(0.05);
        self
    }
    /// Scale every sample so its peak is `target` (evens out recordings made at different
    /// levels; velocity still applies on top).
    pub fn normalize(mut self, target: impl Val) -> Sampler {
        self.cfg.normalize = Some(target.val());
        self
    }
    /// Play every note to the end of its sample, ignoring note-offs.
    pub fn oneshot(mut self) -> Sampler {
        self.cfg.oneshot = true;
        self
    }
    /// Stop notes at note-off with the release envelope (default).
    pub fn gated(mut self) -> Sampler {
        self.cfg.oneshot = false;
        self
    }
    /// Start offset (seconds) for every zone.
    pub fn start(self, secs: f32) -> Sampler {
        self.region(|r| r.start = secs)
    }
    /// End point (seconds) for every zone.
    pub fn end(self, secs: f32) -> Sampler {
        self.region(|r| r.end = Some(secs))
    }
    /// Play every zone backwards.
    pub fn reverse(self) -> Sampler {
        self.region(|r| r.reverse = true)
    }
    /// Loop points (seconds) for every zone.
    pub fn looped(self, start: f32, end: f32) -> Sampler {
        self.region(|r| r.looped = Some((start, end)))
    }
    /// Loop crossfade (seconds).
    pub fn xfade(self, secs: f32) -> Sampler {
        self.region(|r| r.xfade = secs)
    }
    /// Skip leading silence of every sample.
    pub fn trim(self, on: bool) -> Sampler {
        self.region(|r| r.trim = on)
    }
    /// Maximum polyphony.
    pub fn voices(mut self, n: usize) -> Sampler {
        self.poly = self.poly.voices(n);
        self
    }
    /// Monophonic (each note retriggers).
    pub fn mono(mut self) -> Sampler {
        self.poly = self.poly.mono();
        self
    }
    /// Load every zone at `prepare()` instead of on first use (avoids loading/downloading
    /// during realtime playback).
    pub fn preload(mut self) -> Sampler {
        self.preload = true;
        self
    }
    /// Instrument name.
    pub fn named(mut self, name: impl Into<String>) -> Sampler {
        self.name = name.into();
        self
    }
}

impl Instrument for Sampler {
    fn prepare(&mut self, sr: f32) {
        if self.preload {
            for z in &self.zones {
                z.src.get();
            }
        }
        let sh = Arc::new(Shared {
            zones: self.zones.clone(),
            cfg: self.cfg.clone(),
            rr: (0..128).map(|_| AtomicU32::new(0)).collect(),
        });
        self.poly.proto_mut().shared = Some(sh);
        self.poly.prepare(sr);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.poly.note_on(note, ctx)
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        self.poly.note_off(id, ctx)
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        self.poly.render(out, ctx)
    }
    fn active(&self) -> bool {
        self.poly.active()
    }
    fn panic(&mut self) {
        self.poly.panic()
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, secs: f32) -> Arc<Sample> {
        let sr = 48000.0;
        let n = (sr * secs) as usize;
        Arc::new(Sample::from_mono(
            &(0..n)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5)
                .collect::<Vec<_>>(),
            sr,
        ))
    }

    fn run(s: &mut Sampler, notes: &[(f32, f32)], frames: usize) -> Vec<Frame> {
        let sr = 48000.0;
        s.prepare(sr);
        let ctx = Ctx::offline(sr);
        for (i, (p, v)) in notes.iter().enumerate() {
            s.note_on(
                &NoteOn {
                    id: i as u64,
                    pitch: *p,
                    vel: *v,
                    dur: 1.0,
                },
                &ctx,
            );
        }
        let mut out = vec![Frame::ZERO; frames];
        for (k, ch) in out.chunks_mut(64).enumerate() {
            s.render(ch, &ctx.advanced(k * 64));
        }
        out
    }

    fn hz(v: &[Frame]) -> f32 {
        let c = v
            .windows(2)
            .filter(|w| w[0].l <= 0.0 && w[1].l > 0.0)
            .count();
        c as f32 / (v.len() as f32 / 48000.0)
    }

    #[test]
    fn nearest_zone_repitch() {
        let mut s = Sampler::new()
            .zone(Zone::from_sample(sine(220.0, 1.0), 57))
            .zone(Zone::from_sample(sine(440.0, 1.0), 69));
        let out = run(&mut s, &[(72.0, 1.0)], 24000);
        assert!(out.iter().all(|f| f.l.is_finite()));
        let f = hz(&out[1000..]);
        assert!((f - 523.25).abs() < 6.0, "{f}");
    }

    #[test]
    fn round_robin_alternates() {
        let a = sine(100.0, 0.1);
        let b = sine(300.0, 0.1);
        let sh = Shared {
            zones: vec![Zone::from_sample(a, 60), Zone::from_sample(b, 60)],
            cfg: Cfg::default(),
            rr: (0..128).map(|_| AtomicU32::new(0)).collect(),
        };
        let picks: Vec<usize> = (0..4).map(|_| sh.select(60.0, 1.0).unwrap()).collect();
        assert_eq!(picks, vec![0, 1, 0, 1]);
    }

    #[test]
    fn velocity_layers_and_key_ranges() {
        let sh = Shared {
            zones: vec![
                Zone::from_sample(sine(100.0, 0.1), 60)
                    .keys(0, 63)
                    .vel(0.0, 0.5),
                Zone::from_sample(sine(100.0, 0.1), 60)
                    .keys(0, 63)
                    .vel(0.5, 1.0),
                Zone::from_sample(sine(100.0, 0.1), 70).keys(64, 127),
            ],
            cfg: Cfg::default(),
            rr: (0..128).map(|_| AtomicU32::new(0)).collect(),
        };
        assert_eq!(sh.select(60.0, 0.2), Some(0));
        assert_eq!(sh.select(60.0, 0.9), Some(1));
        assert_eq!(sh.select(80.0, 0.9), Some(2));
    }

    #[test]
    fn gated_release_and_oneshot() {
        let sr = 48000.0;
        let ctx = Ctx::offline(sr);
        let mut s = Sampler::new()
            .zone(Zone::from_sample(sine(440.0, 2.0), 69))
            .release(0.01);
        s.prepare(sr);
        s.note_on(
            &NoteOn {
                id: 1,
                pitch: 69.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        let mut buf = [Frame::ZERO; 64];
        s.render(&mut buf, &ctx);
        assert!(buf.iter().any(|f| f.l.abs() > 0.01));
        s.note_off(1, &ctx);
        for _ in 0..200 {
            buf = [Frame::ZERO; 64];
            s.render(&mut buf, &ctx);
        }
        assert!(!s.active());

        let mut o = Sampler::from_sample(sine(440.0, 0.2)).release(0.01);
        o.prepare(sr);
        o.note_on(
            &NoteOn {
                id: 1,
                pitch: 60.0,
                vel: 1.0,
                dur: 0.1,
            },
            &ctx,
        );
        o.note_off(1, &ctx);
        for _ in 0..20 {
            buf = [Frame::ZERO; 64];
            o.render(&mut buf, &ctx);
        }
        assert!(
            o.active() && buf.iter().any(|f| f.l.abs() > 0.01),
            "one-shot ignores note-off"
        );
    }

    #[test]
    fn velocity_curve_and_tone() {
        let mut loud = Sampler::new()
            .zone(Zone::from_sample(sine(440.0, 1.0), 69))
            .tone(1.0);
        let mut soft = Sampler::new()
            .zone(Zone::from_sample(sine(440.0, 1.0), 69))
            .tone(1.0);
        let pk = |v: &[Frame]| v.iter().fold(0f32, |m, f| m.max(f.peak()));
        let a = pk(&run(&mut loud, &[(69.0, 1.0)], 4800));
        let b = pk(&run(&mut soft, &[(69.0, 0.4)], 4800));
        assert!(a > 0.45 && b < a * 0.5, "{a} {b}");
    }
}
