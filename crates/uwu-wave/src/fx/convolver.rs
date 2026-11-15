//! Convolution reverb: [`Convolver`] (uniformly-partitioned FFT convolution with a recorded
//! impulse response).

use super::Effect;
use super::fft::{C, Fft};
use super::filter::{SvfMode, SvfStage};
use super::util::{Ring, Smooth, send_mix};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::Param;
use crate::sample::Sample;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_PREDELAY: f32 = 0.5;

/// Stereo uniformly-partitioned overlap-save convolution engine.
///
/// Both channels are transformed with a single complex FFT (left in the real part, right in the
/// imaginary part) and split using conjugate symmetry, so a block costs one forward and one
/// inverse FFT of size `2·b` plus the spectral multiply-accumulate. Latency is exactly `b`.
#[derive(Clone, Debug)]
pub struct PartConv {
    b: usize,
    n: usize,
    parts: usize,
    fft: Fft,
    hl: Vec<C>,
    hr: Vec<C>,
    fdl_l: Vec<C>,
    fdl_r: Vec<C>,
    fdl_pos: usize,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    work: Vec<C>,
    acc_l: Vec<C>,
    acc_r: Vec<C>,
    idx: usize,
}

impl PartConv {
    /// Build from left/right impulse responses with partition (block) size `b` (power of two).
    pub fn new(ir_l: &[f32], ir_r: &[f32], b: usize) -> PartConv {
        let b = b.max(16).next_power_of_two();
        let n = 2 * b;
        let len = ir_l.len().max(ir_r.len()).max(1);
        let parts = len.div_ceil(b);
        let bins = b + 1;
        let fft = Fft::new(n);
        let mut work = vec![C::ZERO; n];
        let mut hl = vec![C::ZERO; parts * bins];
        let mut hr = vec![C::ZERO; parts * bins];
        for p in 0..parts {
            for (k, w) in work.iter_mut().enumerate() {
                let i = p * b + k;
                *w = if k < b {
                    C::new(
                        ir_l.get(i).copied().unwrap_or(0.0),
                        ir_r.get(i).copied().unwrap_or(0.0),
                    )
                } else {
                    C::ZERO
                };
            }
            fft.forward(&mut work);
            split(
                &work,
                &mut hl[p * bins..(p + 1) * bins],
                &mut hr[p * bins..(p + 1) * bins],
            );
        }
        PartConv {
            b,
            n,
            parts,
            fft,
            hl,
            hr,
            fdl_l: vec![C::ZERO; parts * bins],
            fdl_r: vec![C::ZERO; parts * bins],
            fdl_pos: 0,
            in_l: vec![0.0; n],
            in_r: vec![0.0; n],
            out_l: vec![0.0; b],
            out_r: vec![0.0; b],
            work,
            acc_l: vec![C::ZERO; bins],
            acc_r: vec![C::ZERO; bins],
            idx: 0,
        }
    }
    /// Latency in samples (= partition size).
    pub fn latency(&self) -> usize {
        self.b
    }
    /// Process one stereo sample (output is delayed by [`PartConv::latency`]).
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        let i = self.idx;
        let y = (self.out_l[i], self.out_r[i]);
        self.in_l[self.b + i] = l;
        self.in_r[self.b + i] = r;
        self.idx += 1;
        if self.idx == self.b {
            self.idx = 0;
            self.block();
        }
        y
    }
    fn block(&mut self) {
        let (b, n, bins) = (self.b, self.n, self.b + 1);
        for k in 0..n {
            self.work[k] = C::new(self.in_l[k], self.in_r[k]);
        }
        self.fft.forward(&mut self.work);
        let s = self.fdl_pos * bins;
        split(
            &self.work,
            &mut self.fdl_l[s..s + bins],
            &mut self.fdl_r[s..s + bins],
        );
        self.acc_l.iter_mut().for_each(|c| *c = C::ZERO);
        self.acc_r.iter_mut().for_each(|c| *c = C::ZERO);
        for p in 0..self.parts {
            let slot = (self.fdl_pos + self.parts - p) % self.parts;
            let x_l = &self.fdl_l[slot * bins..(slot + 1) * bins];
            let x_r = &self.fdl_r[slot * bins..(slot + 1) * bins];
            let h_l = &self.hl[p * bins..(p + 1) * bins];
            let h_r = &self.hr[p * bins..(p + 1) * bins];
            for k in 0..bins {
                self.acc_l[k] += x_l[k] * h_l[k];
                self.acc_r[k] += x_r[k] * h_r[k];
            }
        }
        for k in 0..bins {
            self.work[k] = self.acc_l[k] + self.acc_r[k].mul_i();
        }
        for k in bins..n {
            self.work[k] = self.acc_l[n - k].conj() + self.acc_r[n - k].conj().mul_i();
        }
        self.fft.inverse(&mut self.work);
        let scale = 1.0 / n as f32;
        for j in 0..b {
            let c = self.work[b + j];
            self.out_l[j] = crate::dsp::undenormal(c.re * scale);
            self.out_r[j] = crate::dsp::undenormal(c.im * scale);
        }
        self.in_l.copy_within(b..n, 0);
        self.in_r.copy_within(b..n, 0);
        self.fdl_pos = (self.fdl_pos + 1) % self.parts;
    }
    pub fn reset(&mut self) {
        for v in [&mut self.fdl_l, &mut self.fdl_r] {
            v.iter_mut().for_each(|c| *c = C::ZERO);
        }
        for v in [
            &mut self.in_l,
            &mut self.in_r,
            &mut self.out_l,
            &mut self.out_r,
        ] {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
        self.idx = 0;
    }
}

/// Split the FFT of `l + i·r` (two real signals) into their individual spectra (bins 0..=n/2).
#[inline]
fn split(z: &[C], l: &mut [C], r: &mut [C]) {
    let n = z.len();
    for k in 0..l.len() {
        let a = z[k];
        let c = z[(n - k) % n].conj();
        l[k] = (a + c).scale(0.5);
        r[k] = (a - c).mul_neg_i().scale(0.5);
    }
}

/// Convolution reverb: places the sound in a real, recorded space.
///
/// Loads a stereo (or mono) impulse-response WAV, resamples it to the song's rate, optionally
/// re-shapes its decay, and convolves with a uniformly-partitioned FFT engine (preallocated, so
/// it's realtime-safe). Latency equals the partition size (default 512 samples) and is reported
/// through [`Effect::latency`] so the engine compensates it; the dry path is delayed to match.
///
/// IRs are found by name in `assets/library/ir/<name>.wav` (or the VPO `IR/` folder until the
/// library exists), or by path.
///
/// ```ignore
/// let hall = song.bus("hall").fx(Convolver::hall().wet());
/// Convolver::church().predelay(25.ms()).low_cut(150.0).mix(0.3)
/// Reverb::ir("studio").decay_scale(0.6)          // same thing, via Reverb
/// Convolver::ir("assets/my_irs/plate.wav").high_cut(9000.0)
/// ```
///
/// Stereo IRs are applied as `L→L` / `R→R` with an adjustable [`crossfeed`](Convolver::crossfeed)
/// of the opposite input channel (default 0.6), so hard-panned sources still excite both sides
/// of the room.
#[derive(Clone)]
pub struct Convolver {
    ir: Arc<Sample>,
    label: String,
    mix: Param,
    predelay: Param,
    low_cut: Param,
    high_cut: Param,
    width: Param,
    gain: Param,
    crossfeed: f32,
    decay_scale: f32,
    max_len: Option<f32>,
    partition: usize,
    normalize: bool,
    // runtime
    conv: Option<PartConv>,
    pre: [Ring; 2],
    sm_pre: Smooth,
    dry: Vec<Frame>,
    dpos: usize,
    hp: SvfStage,
    lp: SvfStage,
    dry_r: Ramp,
    wet_r: Ramp,
    width_r: Ramp,
    gain_r: Ramp,
    sr: f32,
    ir_secs: f32,
}

impl std::fmt::Debug for Convolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Convolver({}, {} frames @ {} Hz)",
            self.label,
            self.ir.frames.len(),
            self.ir.sr
        )
    }
}

impl Convolver {
    /// Load an impulse response by library name (`"church"`, `"ir/studio"`) or file path.
    /// Panics with the list of available IRs if it can't be found; see [`Convolver::try_ir`].
    pub fn ir(name_or_path: &str) -> Convolver {
        match Convolver::try_ir(name_or_path) {
            Ok(c) => c,
            Err(e) => panic!(
                "uwu-wave: Convolver::ir({name_or_path:?}): {e}. Available IRs: {}",
                Convolver::available().join(", ")
            ),
        }
    }
    /// Fallible [`Convolver::ir`].
    pub fn try_ir(name_or_path: &str) -> Result<Convolver, crate::Error> {
        let path = resolve_ir(name_or_path).unwrap_or_else(|| PathBuf::from(name_or_path));
        let s = Sample::load(&path)?;
        if s.frames.is_empty() {
            return Err(crate::Error::Invalid(format!(
                "{} is empty",
                path.display()
            )));
        }
        let label = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| name_or_path.into());
        Ok(Convolver::from_sample(s).named(label))
    }
    /// Use an already loaded sample as the impulse response.
    pub fn from_sample(ir: Arc<Sample>) -> Convolver {
        Convolver {
            ir,
            label: "ir".into(),
            mix: Param::Const(0.3),
            predelay: Param::Const(0.0),
            low_cut: Param::Const(80.0),
            high_cut: Param::Const(16_000.0),
            width: Param::Const(1.0),
            gain: Param::Const(1.0),
            crossfeed: 0.6,
            decay_scale: 1.0,
            max_len: None,
            partition: 512,
            normalize: true,
            conv: None,
            pre: Default::default(),
            sm_pre: Smooth::default(),
            dry: vec![],
            dpos: 0,
            hp: SvfStage::default(),
            lp: SvfStage::default(),
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            width_r: Ramp::default(),
            gain_r: Ramp::default(),
            sr: 48_000.0,
            ir_secs: 0.0,
        }
    }
    /// Use raw frames (at `sr`) as the impulse response.
    pub fn from_frames(frames: Vec<Frame>, sr: f32) -> Convolver {
        Convolver::from_sample(Arc::new(Sample::from_frames(frames, sr)))
    }
    /// Names of the IRs found in the asset folders.
    pub fn available() -> Vec<String> {
        let mut out = vec![];
        for dir in ir_dirs() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))
                        && let Some(s) = p.file_stem()
                    {
                        let s = s.to_string_lossy().to_lowercase();
                        if !out.contains(&s) {
                            out.push(s);
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    // ---- presets
    /// Large stone church (long, ~8 s tail).
    pub fn church() -> Convolver {
        Convolver::ir("church")
            .predelay(0.015)
            .low_cut(120.0)
            .high_cut(12_000.0)
    }
    /// Concert-hall-ish: the church IR with its decay shortened to ~55% and a hall pre-delay.
    pub fn hall() -> Convolver {
        Convolver::ir("church")
            .named("hall")
            .decay_scale(0.55)
            .predelay(0.025)
            .low_cut(100.0)
            .high_cut(11_000.0)
    }
    /// Recording-studio live room — short, natural ambience for close-miked sections.
    pub fn studio() -> Convolver {
        Convolver::ir("studio")
            .predelay(0.004)
            .low_cut(90.0)
            .high_cut(15_000.0)
    }
    /// Concrete garage — a small, reflective room.
    pub fn room() -> Convolver {
        Convolver::ir("garage").named("room").low_cut(120.0)
    }
    /// Spring reverb tank.
    pub fn spring() -> Convolver {
        Convolver::ir("spring")
            .low_cut(200.0)
            .high_cut(7_000.0)
            .crossfeed(0.2)
    }

    // ---- knobs
    /// Dry/wet (send-style law: dry stays at full level up to 0.5).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// 100% wet — for effect-return buses.
    pub fn wet(self) -> Self {
        self.mix(1.0)
    }
    /// Pre-delay before the reverb (seconds, ≤ 0.5).
    pub fn predelay(mut self, secs: impl Into<Param>) -> Self {
        self.predelay = secs.into();
        self
    }
    /// High-pass on the wet signal (Hz; ≤ 10 disables).
    pub fn low_cut(mut self, hz: impl Into<Param>) -> Self {
        self.low_cut = hz.into();
        self
    }
    /// Low-pass on the wet signal (Hz; ≥ 20 kHz disables).
    pub fn high_cut(mut self, hz: impl Into<Param>) -> Self {
        self.high_cut = hz.into();
        self
    }
    /// Stereo width of the wet signal (0 mono, 1 as recorded, 2 extra wide).
    pub fn width(mut self, w: impl Into<Param>) -> Self {
        self.width = w.into();
        self
    }
    /// Wet output gain (linear; `-3.db()`).
    pub fn gain(mut self, g: impl Into<Param>) -> Self {
        self.gain = g.into();
        self
    }
    /// Amount of the opposite input channel fed into each IR channel (0 = strict L→L/R→R,
    /// 1 = mono input).
    pub fn crossfeed(mut self, c: f32) -> Self {
        self.crossfeed = c.clamp(0.0, 1.0);
        self
    }
    /// Scale the IR's decay time (0.05–1.5): `0.5` makes the space decay twice as fast by
    /// applying an exponential envelope computed from the IR's measured RT60.
    pub fn decay_scale(mut self, k: f32) -> Self {
        self.decay_scale = k.clamp(0.05, 1.5);
        self
    }
    /// Truncate the IR to `secs` (with a short fade-out) — saves CPU on very long IRs.
    pub fn length(mut self, secs: f32) -> Self {
        self.max_len = Some(secs.max(0.01));
        self
    }
    /// FFT partition size in samples (power of two, 64–4096; default 512). Smaller = less
    /// latency, more CPU.
    pub fn partition(mut self, samples: usize) -> Self {
        self.partition = samples.clamp(16, 4096).next_power_of_two();
        self
    }
    /// Normalise the IR's energy (default on) so different rooms sit at a similar level.
    pub fn normalize(mut self, on: bool) -> Self {
        self.normalize = on;
        self
    }
    /// Rename (shown in reports).
    pub fn named(mut self, n: impl Into<String>) -> Self {
        self.label = n.into();
        self
    }
    /// IR length in seconds after preparation (resampling, decay shaping, trimming).
    pub fn ir_secs(&self) -> f32 {
        self.ir_secs
    }

    /// Resample, shape, trim and normalise the IR for sample rate `sr`.
    fn build_ir(&self, sr: f32) -> (Vec<f32>, Vec<f32>) {
        let src = &self.ir;
        let ratio = src.sr as f64 / sr as f64;
        let len = ((src.frames.len() as f64) / ratio).floor().max(1.0) as usize;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = if (ratio - 1.0).abs() < 1e-9 {
            src.frames.iter().map(|f| (f.l, f.r)).unzip()
        } else {
            (0..len)
                .map(|i| {
                    let f = src.hermite(i as f64 * ratio);
                    (f.l, f.r)
                })
                .unzip()
        };
        let energy = |l: &[f32], r: &[f32]| -> Vec<f64> {
            // Schroeder backward integral
            let mut edc = vec![0.0f64; l.len() + 1];
            for i in (0..l.len()).rev() {
                edc[i] = edc[i + 1] + (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
            }
            edc
        };
        // decay shaping
        if (self.decay_scale - 1.0).abs() > 1e-4 {
            let edc = energy(&l, &r);
            let e0 = edc[0].max(1e-30);
            let find = |db: f64| {
                edc.iter()
                    .position(|&e| 10.0 * (e / e0).max(1e-30).log10() <= db)
            };
            let rt60 = match (find(-5.0), find(-25.0)) {
                (Some(a), Some(b)) if b > a => 3.0 * (b - a) as f64 / sr as f64,
                _ => l.len() as f64 / sr as f64,
            }
            .max(0.05);
            let extra_db_per_s = 60.0 / rt60 * (1.0 / self.decay_scale as f64 - 1.0);
            let k = 10f64.powf(-extra_db_per_s / 20.0 / sr as f64);
            let mut g = 1.0f64;
            for i in 0..l.len() {
                l[i] *= g as f32;
                r[i] *= g as f32;
                g *= k;
            }
        }
        // trim below -90 dB of total energy and to max length
        let edc = energy(&l, &r);
        let e0 = edc[0].max(1e-30);
        let mut end = edc
            .iter()
            .position(|&e| e < e0 * 1e-9)
            .unwrap_or(l.len())
            .max(1);
        if let Some(m) = self.max_len {
            end = end.min((m * sr) as usize).max(1);
        }
        // fade only when we actually cut the IR short
        let fade = if end < l.len() {
            ((0.05 * sr) as usize).min(end / 4)
        } else {
            0
        };
        for i in 0..fade {
            let g = (i as f32 + 0.5) / fade as f32;
            l[end - 1 - i] *= g;
            r[end - 1 - i] *= g;
        }
        l.truncate(end);
        r.truncate(end);
        if self.normalize {
            let e: f64 = l
                .iter()
                .zip(&r)
                .map(|(a, b)| (*a as f64).powi(2) + (*b as f64).powi(2))
                .sum::<f64>()
                * 0.5;
            let c = self.crossfeed as f64;
            let k = (0.7 / (e.max(1e-20) * (1.0 + c * c)).sqrt()) as f32;
            l.iter_mut().for_each(|x| *x *= k);
            r.iter_mut().for_each(|x| *x *= k);
        }
        (l, r)
    }
}

/// Directories searched for IRs by name.
fn ir_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(a) = crate::sample::assets_dir() {
        v.push(a.join("library").join("ir"));
        v.push(a.join("VirtualPlayingOrchestra-main").join("IR"));
    }
    v
}

fn resolve_ir(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(name);
    if p.is_file() {
        return Some(p);
    }
    let id = name
        .trim_start_matches("ir/")
        .trim_end_matches(".wav")
        .to_lowercase();
    for dir in ir_dirs() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let path = e.path();
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase());
                if path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("wav"))
                    && stem.as_deref() == Some(id.as_str())
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

impl super::Reverb {
    /// Convolution reverb from an impulse response — shorthand for [`Convolver::ir`].
    ///
    /// ```ignore
    /// song.bus("hall").fx(Reverb::ir("church").decay_scale(0.6).wet())
    /// ```
    pub fn ir(name_or_path: &str) -> Convolver {
        Convolver::ir(name_or_path)
    }
}

impl Effect for Convolver {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        let (l, r) = self.build_ir(sr);
        self.ir_secs = l.len() as f32 / sr;
        let conv = PartConv::new(&l, &r, self.partition);
        self.dry = vec![Frame::ZERO; conv.latency() + 1];
        self.dpos = 0;
        self.conv = Some(conv);
        for p in &mut self.pre {
            *p = Ring::with_secs(MAX_PREDELAY + 0.01, sr);
        }
        self.sm_pre.time(0.05, sr);
        self.sm_pre.reset();
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if self.conv.is_none() {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        let (dg, wg) = send_mix(ctx.p(&self.mix));
        self.dry_r.start(dg, n);
        self.wet_r.start(wg, n);
        self.width_r.start(ctx.p(&self.width).clamp(0.0, 2.0), n);
        self.gain_r.start(ctx.p(&self.gain).max(0.0), n);
        self.sm_pre
            .set(ctx.p(&self.predelay).clamp(0.0, MAX_PREDELAY) * sr);
        let lc = ctx.p(&self.low_cut);
        let hc = ctx.p(&self.high_cut);
        let use_hp = lc > 10.0;
        let use_lp = hc < 20_000.0 && hc < sr * 0.45;
        if use_hp {
            self.hp.set(SvfMode::HighPass, lc.min(sr * 0.45), 0.707, sr);
        }
        if use_lp {
            self.lp.set(SvfMode::LowPass, hc.max(200.0), 0.707, sr);
        }
        let c = self.crossfeed;
        let dl = self.dry.len();
        let conv = self.conv.as_mut().expect("prepared");
        for f in buf.iter_mut() {
            let x = *f;
            // dry, delayed to match the convolution latency
            self.dry[self.dpos] = x;
            self.dpos = (self.dpos + 1) % dl;
            let dry = self.dry[self.dpos];
            // wet
            let pd = self.sm_pre.next();
            self.pre[0].push(x.l + c * x.r);
            self.pre[1].push(x.r + c * x.l);
            let (il, ir) = (self.pre[0].read(pd), self.pre[1].read(pd));
            let (yl, yr) = conv.tick(il, ir);
            let mut y = Frame::new(yl, yr);
            if use_hp {
                y = self.hp.frame(y);
            }
            if use_lp {
                y = self.lp.frame(y);
            }
            let w = self.width_r.next();
            let (m, s) = (y.mid(), y.side() * w);
            let wet = Frame::new(m + s, m - s) * self.gain_r.next();
            *f = dry * self.dry_r.next() + wet * self.wet_r.next();
        }
    }
    fn reset(&mut self) {
        if let Some(c) = self.conv.as_mut() {
            c.reset();
        }
        for p in &mut self.pre {
            p.clear();
        }
        self.dry.iter_mut().for_each(|f| *f = Frame::ZERO);
        self.hp.reset();
        self.lp.reset();
    }
    fn tail(&self) -> f32 {
        let pre = self.predelay.as_const().unwrap_or(MAX_PREDELAY);
        let ir = if self.ir_secs > 0.0 {
            self.ir_secs
        } else {
            self.ir.duration()
        };
        pre + ir + self.partition as f32 / self.sr
    }
    fn latency(&self) -> usize {
        self.conv
            .as_ref()
            .map(|c| c.latency())
            .unwrap_or(self.partition)
    }
    fn name(&self) -> String {
        format!("convolver({})", self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Rng;

    fn direct(x: &[f32], h: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|n| (0..h.len().min(n + 1)).map(|k| h[k] * x[n - k]).sum())
            .collect()
    }

    #[test]
    fn partitioned_matches_direct_convolution() {
        let mut rng = Rng::new(3);
        let hl: Vec<f32> = (0..300)
            .map(|i| rng.bipolar() * (-(i as f32) / 80.0).exp())
            .collect();
        let hr: Vec<f32> = (0..237)
            .map(|i| rng.bipolar() * (-(i as f32) / 50.0).exp())
            .collect();
        let xl: Vec<f32> = (0..2000).map(|_| rng.bipolar()).collect();
        let xr: Vec<f32> = (0..2000).map(|_| rng.bipolar()).collect();
        let mut pc = PartConv::new(&hl, &hr, 64);
        assert_eq!(pc.latency(), 64);
        let out: Vec<(f32, f32)> = xl.iter().zip(&xr).map(|(a, b)| pc.tick(*a, *b)).collect();
        let (dl, dr) = (direct(&xl, &hl), direct(&xr, &hr));
        for i in 64..2000 {
            assert!(
                (out[i].0 - dl[i - 64]).abs() < 1e-4,
                "L {i}: {} vs {}",
                out[i].0,
                dl[i - 64]
            );
            assert!((out[i].1 - dr[i - 64]).abs() < 1e-4, "R {i}");
        }
    }

    #[test]
    fn convolver_effect_is_aligned_and_reports_latency() {
        let mut rng = Rng::new(9);
        let ir: Vec<Frame> = (0..500)
            .map(|i| Frame::new(rng.bipolar(), rng.bipolar()) * (-(i as f32) / 100.0).exp())
            .collect();
        let (hl, hr): (Vec<f32>, Vec<f32>) = ir.iter().map(|f| (f.l, f.r)).unzip();
        let mut cv = Convolver::from_frames(ir, 48_000.0)
            .partition(128)
            .normalize(false)
            .crossfeed(0.0)
            .low_cut(0.0)
            .high_cut(24_000.0)
            .wet();
        cv.prepare(48_000.0);
        assert_eq!(Effect::latency(&cv), 128);
        let ctx = Ctx::offline(48_000.0);
        let x: Vec<Frame> = (0..3000)
            .map(|_| Frame::new(rng.bipolar(), rng.bipolar()))
            .collect();
        let mut out = vec![];
        for chunk in x.chunks(64) {
            let mut b = chunk.to_vec();
            cv.process(&mut b, &ctx);
            out.extend(b);
        }
        let xl: Vec<f32> = x.iter().map(|f| f.l).collect();
        let xr: Vec<f32> = x.iter().map(|f| f.r).collect();
        let (dl, dr) = (direct(&xl, &hl), direct(&xr, &hr));
        for i in 128..3000 {
            assert!(
                (out[i].l - dl[i - 128]).abs() < 1e-3 && (out[i].r - dr[i - 128]).abs() < 1e-3,
                "{i}"
            );
        }
        // dry path is delayed by the same latency
        let mut dry = Convolver::from_frames(vec![Frame::mono(1.0); 10], 48_000.0)
            .partition(128)
            .mix(0.0);
        dry.prepare(48_000.0);
        let mut b = x[..256].to_vec();
        dry.process(&mut b[..64], &ctx);
        dry.process(&mut b[64..128], &ctx);
        dry.process(&mut b[128..192], &ctx);
        assert_eq!(b[150], x[150 - 128]);
    }

    #[test]
    fn decay_scale_shortens_and_files_load() {
        // synthetic 2 s exponential noise tail at 44.1 kHz, resampled to 48 kHz
        let mut rng = Rng::new(1);
        let ir: Vec<Frame> = (0..88_200)
            .map(|i| {
                Frame::new(rng.bipolar(), rng.bipolar()) * 10f32.powf(-3.0 * i as f32 / 88_200.0)
            })
            .collect();
        let mut long = Convolver::from_frames(ir.clone(), 44_100.0);
        long.prepare(48_000.0);
        let mut short = Convolver::from_frames(ir, 44_100.0).decay_scale(0.5);
        short.prepare(48_000.0);
        assert!(long.ir_secs() > 1.7, "{}", long.ir_secs());
        assert!(
            // RT60 halves (2 s → 1 s); the IR is trimmed at -90 dB ≈ 1.5 × RT60
            short.ir_secs() < long.ir_secs() * 0.8,
            "{} {}",
            short.ir_secs(),
            long.ir_secs()
        );
        // real IRs, when the assets are present
        if !Convolver::available().is_empty() {
            for name in Convolver::available() {
                let mut c = Convolver::ir(&name).wet();
                c.prepare(48_000.0);
                let ctx = Ctx::offline(48_000.0);
                let mut b = [Frame::ZERO; 64];
                b[0] = Frame::mono(1.0);
                let mut peak = 0f32;
                for _ in 0..200 {
                    c.process(&mut b, &ctx);
                    peak = peak.max(b.iter().fold(0f32, |m, f| m.max(f.peak())));
                    b = [Frame::ZERO; 64];
                }
                assert!(
                    peak.is_finite() && peak > 1e-4 && peak < 2.0,
                    "{name}: {peak}"
                );
            }
        }
    }
}
