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
    /// Already loaded (or failed)?
    pub(crate) fn is_loaded(&self) -> bool {
        self.cell.get().is_some()
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
    wheel_octs: f32,
    tone: f32,
    vel_sens: f32,
    vel_curve: f32,
    vel_xfade: f32,
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
            wheel_octs: 0.0,
            tone: 0.0,
            vel_sens: 1.0,
            vel_curve: 1.3,
            vel_xfade: 0.0,
            oneshot: false,
            normalize: None,
        }
    }
}

/// Up to two velocity layers a note plays (and their equal-power weights).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layers {
    /// Index of a zone defining the key group (root + key range).
    group: usize,
    vel: [(f32, f32); 2],
    w: [f32; 2],
    n: usize,
}

/// Zones chosen for one note.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sel {
    idx: [usize; 2],
    w: [f32; 2],
    n: usize,
}

pub(crate) struct Shared {
    zones: Vec<Zone>,
    cfg: Cfg,
    rr: Vec<AtomicU32>,
}

impl Shared {
    fn new(zones: Vec<Zone>, cfg: Cfg) -> Shared {
        Shared {
            zones,
            cfg,
            rr: (0..128).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    fn same_group(a: &Zone, b: &Zone) -> bool {
        (a.root - b.root).abs() < 1e-4 && a.keys == b.keys
    }

    /// Equal-power weight of zone `x` (in the key group of `g`) at velocity `v`. Edges only
    /// fade where another layer of the same group starts/ends (so lone zones keep hard edges).
    fn weight(&self, x: &Zone, g: &Zone, v: f32) -> f32 {
        let w = self.cfg.vel_xfade;
        let (lo, hi) = x.vel;
        if w <= 0.0 {
            return if x.in_vel(v) { 1.0 } else { 0.0 };
        }
        let h = 0.5 * w;
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let below = self
            .zones
            .iter()
            .any(|y| Self::same_group(y, g) && near(y.vel.1, lo) && y.vel != x.vel);
        let above = self
            .zones
            .iter()
            .any(|y| Self::same_group(y, g) && near(y.vel.0, hi) && y.vel != x.vel);
        let tin = if below {
            ((v - (lo - h)) / w).clamp(0.0, 1.0)
        } else if v >= lo - 1e-6 {
            1.0
        } else {
            0.0
        };
        let tout = if above {
            ((v - (hi - h)) / w).clamp(0.0, 1.0)
        } else if v <= hi + 1e-6 {
            0.0
        } else {
            1.0
        };
        (tin * std::f32::consts::FRAC_PI_2).sin() * (tout * std::f32::consts::FRAC_PI_2).cos()
    }

    /// Key group (best key/root match; ties prefer the zone above, which repitches downwards)
    /// and the velocity layers a note plays.
    fn layers(&self, pitch: f32, vel: f32) -> Option<Layers> {
        let z = &self.zones;
        if z.is_empty() {
            return None;
        }
        let h = 0.5 * self.cfg.vel_xfade;
        let reach = |x: &Zone| vel >= x.vel.0 - h - 1e-6 && vel <= x.vel.1 + h + 1e-6;
        let any = z.iter().any(reach);
        let mut best: Option<usize> = None;
        let mut best_score = f32::INFINITY;
        for (i, x) in z.iter().enumerate() {
            if any && !reach(x) {
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
        let g = &z[best?];
        let mut l = Layers {
            group: best?,
            vel: [(0.0, 0.0); 2],
            w: [0.0; 2],
            n: 0,
        };
        for x in z.iter().filter(|x| Self::same_group(x, g)) {
            if l.vel[..l.n].contains(&x.vel) {
                continue;
            }
            let w = self.weight(x, g, vel);
            if w < 1e-3 {
                continue;
            }
            if l.n < 2 {
                l.vel[l.n] = x.vel;
                l.w[l.n] = w;
                l.n += 1;
            } else {
                let k = if l.w[0] < l.w[1] { 0 } else { 1 };
                if w > l.w[k] {
                    l.vel[k] = x.vel;
                    l.w[k] = w;
                }
            }
            if self.cfg.vel_xfade <= 0.0 {
                break; // hard layers: the first match wins
            }
        }
        if l.n == 0 {
            // outside every velocity range: use the closest zone's layer at full weight
            l.vel[0] = g.vel;
            l.w[0] = 1.0;
            l.n = 1;
        }
        Some(l)
    }

    /// Pick the zones for a note (round-robin among equivalent zones, one counter step per note).
    fn select(&self, pitch: f32, vel: f32) -> Sel {
        let Some(l) = self.layers(pitch, vel) else {
            return Sel::default();
        };
        let g = &self.zones[l.group];
        let slot = g.root.round().clamp(0.0, 127.0) as usize;
        let mut k: Option<u32> = None;
        let mut sel = Sel::default();
        for i in 0..l.n {
            let matches = |x: &&Zone| Self::same_group(x, g) && x.vel == l.vel[i];
            let n = self.zones.iter().filter(matches).count() as u32;
            if n == 0 {
                continue;
            }
            let pick = if n == 1 {
                0
            } else {
                *k.get_or_insert_with(|| self.rr[slot].fetch_add(1, Ordering::Relaxed)) % n
            };
            if let Some((zi, _)) = self
                .zones
                .iter()
                .enumerate()
                .filter(|(_, x)| matches(x))
                .nth(pick as usize)
            {
                sel.idx[sel.n] = zi;
                sel.w[sel.n] = l.w[i];
                sel.n += 1;
            }
        }
        sel
    }

    /// Every zone a note could trigger (all round-robin alternates of all its layers).
    fn candidates(&self, pitch: f32, vel: f32, out: &mut Vec<usize>) {
        let Some(l) = self.layers(pitch, vel) else {
            return;
        };
        let g = &self.zones[l.group];
        for (i, x) in self.zones.iter().enumerate() {
            if Self::same_group(x, g) && l.vel[..l.n].contains(&x.vel) && !out.contains(&i) {
                out.push(i);
            }
        }
    }
}

/// Load many sources at once: up to 6 worker threads (each may run `curl` for git-LFS
/// pointers), with a compact progress line when anything has to be downloaded.
pub(crate) fn load_all(srcs: &[Arc<Src>]) {
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    let todo: Vec<&Arc<Src>> = srcs.iter().filter(|s| !s.is_loaded()).collect();
    if todo.is_empty() {
        return;
    }
    let fetch: Vec<u64> = todo
        .iter()
        .map(|s| {
            s.path
                .as_ref()
                .filter(|p| !super::lfs::is_available(p))
                .and_then(|p| super::lfs::LfsPointer::read(p))
                .map(|ptr| ptr.size)
                .unwrap_or(0)
        })
        .collect();
    let n_fetch = fetch.iter().filter(|&&b| b > 0).count();
    let total_bytes: u64 = fetch.iter().sum();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let done_bytes = AtomicU64::new(0);
    let fetched = AtomicUsize::new(0);
    let _quiet = super::lfs::quiet();
    let t0 = std::time::Instant::now();
    let threads = if n_fetch > 0 { 6 } else { 4 }.min(todo.len());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(s) = todo.get(i) else { break };
                    s.get();
                    if fetch[i] > 0 {
                        fetched.fetch_add(1, Ordering::Relaxed);
                        done_bytes.fetch_add(fetch[i], Ordering::Relaxed);
                    }
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        if n_fetch > 0 {
            use std::io::Write;
            let mb = |b: u64| b as f64 / 1e6;
            loop {
                let d = done.load(Ordering::Relaxed);
                let mut e = std::io::stderr().lock();
                let _ = write!(
                    e,
                    "\r⇣ fetching samples {}/{} ({:.1}/{:.1} MB)\x1b[K",
                    fetched.load(Ordering::Relaxed),
                    n_fetch,
                    mb(done_bytes.load(Ordering::Relaxed)),
                    mb(total_bytes)
                );
                let _ = e.flush();
                if d >= todo.len() {
                    let _ = writeln!(
                        e,
                        "\r⇣ fetched {n_fetch} samples ({:.1} MB) in {:.1}s\x1b[K",
                        mb(total_bytes),
                        t0.elapsed().as_secs_f32()
                    );
                    break;
                }
                drop(e);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });
}

/// Pitch-bend smoothing shared by sampler and kit voices: a one-pole on the bend target
/// (≈5 ms) and a per-sample linear ramp of the playback ratio across each block.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bend {
    st: f32,
    r0: f64,
    r1: f64,
}

impl Bend {
    pub(crate) fn reset(&mut self, st: f32) {
        self.st = st;
        self.r0 = 2f64.powf(st as f64 / 12.0);
        self.r1 = self.r0;
    }
    /// Move towards `target` semitones over this block of `n` frames.
    #[inline]
    pub(crate) fn block(&mut self, target: f32, n: usize, sr: f32) {
        self.r0 = self.r1;
        if (target - self.st).abs() > 1e-5 {
            let a = 1.0 - (-(n as f32) / (0.005 * sr)).exp();
            self.st += (target - self.st) * a;
            self.r1 = 2f64.powf(self.st as f64 / 12.0);
        }
    }
    #[inline]
    pub(crate) fn moving(&self) -> bool {
        self.r0 != self.r1
    }
    /// Current ratio at sample `i` of `n`.
    #[inline]
    pub(crate) fn at(&self, i: usize, n: usize) -> f64 {
        self.r0 + (self.r1 - self.r0) * (i + 1) as f64 / n as f64
    }
    #[inline]
    pub(crate) fn ratio(&self) -> f64 {
        self.r1
    }
}

/// A [`Sampler`] voice (used through [`Poly`]). Plays up to two velocity layers at once.
#[derive(Clone, Default)]
pub struct SamplerVoice {
    shared: Option<Arc<Shared>>,
    sr: f32,
    play: [Play; 2],
    lw: [f32; 2],
    base: [f64; 2],
    fade: [Fader; 2],
    env: Adsr,
    amp: f32,
    gl: f32,
    gr: f32,
    gain: Ramp,
    bend: Bend,
    filt: [Svf; 2],
    filt_on: bool,
    vel_cut: f32,
    f0: f32,
    oneshot: bool,
}

impl SamplerVoice {
    fn playing(&self) -> bool {
        self.play[0].is_playing() || self.play[1].is_playing()
    }
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
        for i in 0..2 {
            if self.play[i].is_playing() {
                let g = self.env.level() * self.amp * self.lw[i];
                self.fade[i].begin(&mut self.play[i], g, 0.004, sr);
            }
        }
        let c = &sh.cfg;
        let sel = sh.select(n.pitch, n.vel);
        if sel.n == 0 {
            return;
        }
        self.bend.reset(ctx.p(&c.tune) + ctx.bend);
        let r0 = self.bend.ratio();
        self.lw = [0.0; 2];
        for k in 0..sel.n {
            let z = &sh.zones[sel.idx[k]];
            let Some(smp) = z.src.get() else { continue };
            let ratio = 2f64.powf(((n.pitch - z.root + z.tune) / 12.0) as f64);
            self.play[k] = Play::start(smp, &z.region, ratio * r0, sr);
            self.base[k] = self.play[k].step() / r0;
            let norm = c.normalize.map(|t| t / smp.peak().max(1e-4)).unwrap_or(1.0);
            self.lw[k] = sel.w[k] * z.gain * norm;
        }
        if !self.playing() {
            return;
        }
        let z = &sh.zones[sel.idx[0]];
        let v = n.vel.clamp(0.0, 1.0);
        self.amp = (1.0 - c.vel_sens) + c.vel_sens * v.powf(c.vel_curve);
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
        for f in &mut self.fade {
            f.render(out, gl, gr);
        }
        let fading = self.fade[0].active() || self.fade[1].active();
        let Some(sh) = self.shared.as_ref() else {
            return false;
        };
        if !self.playing() {
            return fading;
        }
        let c = &sh.cfg;
        let n = out.len();
        self.gain.start(ctx.p(&c.gain) * self.amp, n);
        let target = if c.tune.as_const() == Some(0.0) {
            ctx.bend
        } else {
            ctx.p(&c.tune) + ctx.bend
        };
        self.bend.block(target, n, self.sr);
        let moving = self.bend.moving();
        if !moving {
            let r = self.bend.ratio();
            for k in 0..2 {
                self.play[k].set_step(self.base[k] * r);
            }
        }
        if self.filt_on {
            let mut cut = self.vel_cut.max(self.f0 * 3.0);
            if let Some(p) = &c.cutoff {
                let open = if c.wheel_octs != 0.0 {
                    2f32.powf(ctx.modwheel * c.wheel_octs)
                } else {
                    1.0
                };
                cut = cut.min(ctx.p(p) * open);
            }
            let cut = cut.clamp(20.0, self.sr * 0.45);
            for f in &mut self.filt {
                f.set(cut, 0.6, self.sr);
            }
        }
        let [w0, w1] = self.lw;
        for (i, f) in out.iter_mut().enumerate() {
            if moving {
                let r = self.bend.at(i, n);
                self.play[0].set_step(self.base[0] * r);
                self.play[1].set_step(self.base[1] * r);
            }
            let e = self.env.next();
            let a = self.play[0].next();
            let b = self.play[1].next();
            if a.is_none() && b.is_none() {
                break;
            }
            let mut x = a.unwrap_or_default() * w0 + b.unwrap_or_default() * w1;
            if self.filt_on {
                x = Frame::new(self.filt[0].lp(x.l), self.filt[1].lp(x.r));
            }
            let g = e * self.gain.next();
            f.l += x.l * g * gl;
            f.r += x.r * g * gr;
            if self.env.is_idle() {
                self.play[0].stop();
                self.play[1].stop();
                break;
            }
        }
        self.playing() || fading
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
    preload_all: bool,
    shared: Option<Arc<Shared>>,
    sr: f32,
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
            preload_all: false,
            shared: None,
            sr: 48000.0,
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
    /// Load *every* zone at `prepare()`. Usually unnecessary: the engine calls
    /// [`Instrument::preload`] with the notes that will be played, and only the zones those
    /// notes can hit are loaded (in parallel) before rendering starts.
    pub fn preload_all(mut self) -> Sampler {
        self.preload_all = true;
        self
    }
    /// Crossfade adjacent velocity layers over `width` (0..1 velocity units, e.g. `0.15`): a
    /// note near a layer boundary plays both layers with equal-power weights. Round-robin
    /// alternates keep rotating within each layer.
    ///
    /// ```no_run
    /// use uwu_wave::prelude::*;
    /// let snare = Sampler::new()
    ///     .zone(Zone::new("snare-soft.wav", 60).vel(0.0, 0.5))
    ///     .zone(Zone::new("snare-hard.wav", 60).vel(0.5, 1.0))
    ///     .vel_xfade(0.2);
    /// ```
    pub fn vel_xfade(mut self, width: f32) -> Sampler {
        self.cfg.vel_xfade = width.clamp(0.0, 1.0);
        self
    }
    /// Mod wheel → filter: the [`Sampler::cutoff`] opens by `octaves` at full mod wheel
    /// (`Track::modwheel`). Needs `.cutoff(..)`; without this the mod wheel is ignored.
    pub fn wheel(mut self, octaves: f32) -> Sampler {
        self.cfg.wheel_octs = octaves;
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
        self.sr = sr;
        if self.preload_all {
            let srcs: Vec<Arc<Src>> = self.zones.iter().map(|z| z.src.clone()).collect();
            load_all(&srcs);
        }
        let sh = Arc::new(Shared::new(self.zones.clone(), self.cfg.clone()));
        self.shared = Some(sh.clone());
        self.poly.proto_mut().shared = Some(sh);
        self.poly.prepare(sr);
    }
    /// Load exactly the zones `notes` can trigger (every round-robin alternate and both layers
    /// of a velocity crossfade), in parallel, and pre-compute their mip-maps.
    fn preload(&mut self, notes: &[crate::clip::Note]) {
        let sh = match &self.shared {
            Some(s) => s.clone(),
            None => Arc::new(Shared::new(self.zones.clone(), self.cfg.clone())),
        };
        let mut seen = std::collections::HashSet::new();
        let mut idx = vec![];
        for n in notes {
            // velocity is quantized to 1/1024 so huge note lists stay cheap
            if seen.insert((n.pitch.to_bits(), (n.vel * 1024.0) as i32)) {
                sh.candidates(n.pitch, n.vel, &mut idx);
            }
        }
        let srcs: Vec<Arc<Src>> = idx.iter().map(|&i| sh.zones[i].src.clone()).collect();
        load_all(&srcs);
        let mut warmed = std::collections::HashSet::new();
        for n in notes {
            if !warmed.insert(n.pitch.to_bits()) {
                continue;
            }
            let mut c = vec![];
            sh.candidates(n.pitch, n.vel, &mut c);
            for i in c {
                let z = &sh.zones[i];
                if let Some(s) = z.src.get() {
                    super::voice::warm(
                        s,
                        2f64.powf(((n.pitch - z.root + z.tune) / 12.0) as f64),
                        self.sr,
                    );
                }
            }
        }
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
                    artic: Default::default(),
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
        let sh = Shared::new(
            vec![Zone::from_sample(a, 60), Zone::from_sample(b, 60)],
            Cfg::default(),
        );
        let picks: Vec<usize> = (0..4).map(|_| sh.select(60.0, 1.0).idx[0]).collect();
        assert_eq!(picks, vec![0, 1, 0, 1]);
    }

    fn layered(xfade: f32) -> Shared {
        let z = |v: (f32, f32)| Zone::from_sample(sine(100.0, 0.1), 60).vel(v.0, v.1);
        let cfg = Cfg {
            vel_xfade: xfade,
            ..Cfg::default()
        };
        // soft layer with two round-robin takes, hard layer with one
        Shared::new(vec![z((0.0, 0.5)), z((0.0, 0.5)), z((0.5, 1.0))], cfg)
    }

    #[test]
    fn velocity_crossfade_is_equal_power_and_keeps_round_robin() {
        let sh = layered(0.2);
        // far from the boundary: one layer at full weight
        let s = sh.select(60.0, 0.1);
        assert_eq!((s.n, s.w[0]), (1, 1.0));
        let s = sh.select(60.0, 0.95);
        assert_eq!((s.n, s.idx[0], s.w[0]), (1, 2, 1.0));
        // on the boundary: both layers, equal power
        let s = sh.select(60.0, 0.5);
        assert_eq!(s.n, 2);
        let p: f32 = s.w[..2].iter().map(|w| w * w).sum();
        assert!(
            (p - 1.0).abs() < 1e-4 && (s.w[0] - s.w[1]).abs() < 1e-4,
            "{s:?}"
        );
        // inside the fade: power stays constant
        for v in [0.42, 0.45, 0.55, 0.58] {
            let s = sh.select(60.0, v);
            let p: f32 = s.w[..s.n].iter().map(|w| w * w).sum();
            assert!((p - 1.0).abs() < 1e-3, "{v}: {s:?}");
        }
        // the soft layer still alternates between its two takes
        let soft: Vec<usize> = (0..4)
            .map(|_| sh.select(60.0, 0.48))
            .map(|s| *s.idx[..s.n].iter().find(|&&i| i < 2).unwrap())
            .collect();
        assert!(soft.contains(&0) && soft.contains(&1), "{soft:?}");
        // without crossfade: hard switch
        let hard = layered(0.0);
        assert_eq!(hard.select(60.0, 0.45).n, 1);
        // preload candidates: every take of both layers
        let mut c = vec![];
        sh.candidates(60.0, 0.5, &mut c);
        c.sort();
        assert_eq!(c, vec![0, 1, 2]);
        let mut c = vec![];
        sh.candidates(60.0, 0.9, &mut c);
        assert_eq!(c, vec![2]);
    }

    #[test]
    fn bend_moves_pitch_smoothly() {
        let mut s = Sampler::new().zone(Zone::from_sample(sine(440.0, 2.0), 69));
        let sr = 48000.0;
        s.prepare(sr);
        let ctx = Ctx::offline(sr);
        s.note_on(
            &NoteOn {
                id: 1,
                pitch: 69.0,
                vel: 1.0,
                artic: Default::default(),
                dur: 1.0,
            },
            &ctx,
        );
        let mut out = vec![Frame::ZERO; 48000];
        for (k, ch) in out.chunks_mut(64).enumerate() {
            let c = Ctx {
                bend: 12.0,
                ..ctx.advanced(k * 64)
            };
            s.render(ch, &c);
        }
        let f = hz(&out[4800..]);
        assert!((f - 880.0).abs() < 8.0, "{f}");
        // no discontinuity at the start of the bend
        let jump = out
            .windows(2)
            .map(|w| (w[1].l - w[0].l).abs())
            .fold(0f32, f32::max);
        assert!(jump < 0.2, "{jump}");
    }

    #[test]
    fn preload_loads_only_needed_zones() {
        let dir = std::env::temp_dir().join(format!("uwu-preload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        for name in ["C3.wav", "C4.wav", "C5.wav"] {
            let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
            for i in 0..1000 {
                w.write_sample(((i as f32 * 0.1).sin() * 10000.0) as i16)
                    .unwrap();
            }
            w.finalize().unwrap();
        }
        let mut s = Sampler::dir(&dir);
        s.prepare(48000.0);
        s.preload(&[crate::clip::Note::new(0, 1, 61.0, 1.0)]);
        let loaded: Vec<bool> = s
            .zones()
            .iter()
            .map(|z| z.src.cell.get().is_some())
            .collect();
        assert_eq!(loaded, vec![false, true, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn velocity_layers_and_key_ranges() {
        let sh = Shared::new(
            vec![
                Zone::from_sample(sine(100.0, 0.1), 60)
                    .keys(0, 63)
                    .vel(0.0, 0.5),
                Zone::from_sample(sine(100.0, 0.1), 60)
                    .keys(0, 63)
                    .vel(0.5, 1.0),
                Zone::from_sample(sine(100.0, 0.1), 70).keys(64, 127),
            ],
            Cfg::default(),
        );
        assert_eq!(sh.select(60.0, 0.2).idx[0], 0);
        assert_eq!(sh.select(60.0, 0.9).idx[0], 1);
        assert_eq!(sh.select(80.0, 0.9).idx[0], 2);
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
                artic: Default::default(),
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
                artic: Default::default(),
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
