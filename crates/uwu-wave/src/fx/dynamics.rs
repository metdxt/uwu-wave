//! Dynamics: [`Compressor`], [`Limiter`], [`Gate`], [`Ducker`], [`TransientShaper`].
//!
//! Level knobs (thresholds, ceilings, ranges) are linear gains written with `.db()`; plain
//! non-positive numbers are read as dB (`-18.0` = -18 dB). Times are seconds (`10.ms()`).

use super::Effect;
use super::filter::{SvfMode, SvfStage};
use super::util::{amp_db, coef, db_amp, level, level_db};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::Param;

#[inline]
fn follow(env: f32, x: f32, att: f32, rel: f32) -> f32 {
    let c = if x > env { att } else { rel };
    crate::dsp::undenormal(x + (env - x) * c)
}

/// Optional sidechain high-pass so bass doesn't dominate the detector.
#[derive(Clone, Copy, Debug, Default)]
struct ScFilter {
    hz: f32,
    st: SvfStage,
}
impl ScFilter {
    fn prepare(&mut self, sr: f32) {
        if self.hz > 0.0 {
            self.st.set(SvfMode::HighPass, self.hz, 0.707, sr);
        }
    }
    #[inline]
    fn tick(&mut self, f: Frame) -> Frame {
        if self.hz > 0.0 { self.st.frame(f) } else { f }
    }
}

// ---- Compressor --------------------------------------------------------------------------------

/// Feed-forward compressor with soft knee, stereo-linked peak detection, makeup gain, parallel
/// mix and an optional external sidechain.
///
/// ```ignore
/// Compressor::new().threshold(-18.db()).ratio(4.0).attack(10.ms()).release(120.ms())
/// Compressor::glue()                               // gentle bus glue
/// Compressor::pump().sidechain("kick")             // classic EDM pumping
/// Compressor::new().ratio(8.0).mix(0.4)             // parallel ("New York") compression
/// ```
#[derive(Clone, Debug)]
pub struct Compressor {
    threshold: Param,
    ratio: Param,
    attack: Param,
    release: Param,
    knee: f32,
    makeup: Param,
    auto_makeup: bool,
    mix: Param,
    source: Option<String>,
    sc: ScFilter,
    env: f32,
    mix_ramp: Ramp,
    mk_ramp: Ramp,
    sr: f32,
}

impl Compressor {
    /// -18 dB threshold, 4:1, 10 ms attack, 120 ms release, 6 dB knee.
    pub fn new() -> Compressor {
        Compressor {
            threshold: Param::Const(db_amp(-18.0)),
            ratio: Param::Const(4.0),
            attack: Param::Const(0.010),
            release: Param::Const(0.120),
            knee: 6.0,
            makeup: Param::Const(1.0),
            auto_makeup: false,
            mix: Param::Const(1.0),
            source: None,
            sc: ScFilter::default(),
            env: 0.0,
            mix_ramp: Ramp::default(),
            mk_ramp: Ramp::default(),
            sr: 48_000.0,
        }
    }
    /// Gentle 2:1 bus "glue": slow attack lets transients through, auto makeup.
    pub fn glue() -> Compressor {
        Compressor::new()
            .threshold(db_amp(-14.0))
            .ratio(2.0)
            .attack(0.030)
            .release(0.200)
            .knee(8.0)
            .auto_makeup()
    }
    /// Punchy drum compression: 30 ms attack keeps the snap, fast release.
    pub fn punch() -> Compressor {
        Compressor::new()
            .threshold(db_amp(-20.0))
            .ratio(4.0)
            .attack(0.030)
            .release(0.080)
            .knee(4.0)
            .auto_makeup()
    }
    /// Heavy, fast settings for sidechain pumping (combine with [`Compressor::sidechain`]).
    pub fn pump() -> Compressor {
        Compressor::new()
            .threshold(db_amp(-24.0))
            .ratio(6.0)
            .attack(0.001)
            .release(0.160)
            .knee(3.0)
    }
    /// Level above which compression starts (`-18.db()`).
    pub fn threshold(mut self, t: impl Into<Param>) -> Self {
        self.threshold = t.into();
        self
    }
    /// Compression ratio (`4.0` = 4:1; `20.0`+ ≈ limiting).
    pub fn ratio(mut self, r: impl Into<Param>) -> Self {
        self.ratio = r.into();
        self
    }
    /// Attack time in seconds.
    pub fn attack(mut self, secs: impl Into<Param>) -> Self {
        self.attack = secs.into();
        self
    }
    /// Release time in seconds.
    pub fn release(mut self, secs: impl Into<Param>) -> Self {
        self.release = secs.into();
        self
    }
    /// Soft-knee width in dB (0 = hard knee).
    pub fn knee(mut self, db: f32) -> Self {
        self.knee = db.max(0.0);
        self
    }
    /// Output makeup gain (`3.db()`).
    pub fn makeup(mut self, g: impl Into<Param>) -> Self {
        self.makeup = g.into();
        self
    }
    /// Add makeup gain automatically from threshold and ratio.
    pub fn auto_makeup(mut self) -> Self {
        self.auto_makeup = true;
        self
    }
    /// Dry/wet for parallel compression (1 = fully compressed).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// Use another track/bus as the detector input (e.g. `"kick"` for pumping).
    pub fn sidechain(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
    /// High-pass the detector signal (Hz) so lows don't trigger compression.
    pub fn sc_hpf(mut self, hz: f32) -> Self {
        self.sc.hz = hz;
        self
    }
    /// Current gain reduction in dB (positive number).
    pub fn gain_reduction_db(&self) -> f32 {
        self.env
    }
}

impl Effect for Compressor {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.sc.prepare(sr);
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        let t = level_db(ctx.p(&self.threshold));
        let r = ctx.p(&self.ratio).max(1.0);
        let slope = 1.0 / r - 1.0;
        let w = self.knee;
        let att = coef(ctx.p(&self.attack).max(1e-5), sr);
        let rel = coef(ctx.p(&self.release).max(1e-4), sr);
        let mut mk = level_db(ctx.p(&self.makeup));
        if self.auto_makeup {
            mk += -t * (1.0 - 1.0 / r) * 0.5;
        }
        self.mk_ramp.start(db_amp(mk), buf.len());
        self.mix_ramp
            .start(ctx.p(&self.mix).clamp(0.0, 1.0), buf.len());
        let sc = ctx.sidechain.filter(|s| s.len() >= buf.len());
        for (i, f) in buf.iter_mut().enumerate() {
            let det = match sc {
                Some(s) => s[i],
                None => *f,
            };
            let det = self.sc.tick(det);
            let x = amp_db(det.peak());
            let over = x - t;
            let gr = if 2.0 * over < -w {
                0.0
            } else if w > 0.0 && 2.0 * over.abs() <= w {
                -slope * (over + 0.5 * w) * (over + 0.5 * w) / (2.0 * w)
            } else {
                -slope * over
            };
            self.env = follow(self.env, gr, att, rel);
            let g = db_amp(-self.env) * self.mk_ramp.next();
            let m = self.mix_ramp.next();
            *f *= 1.0 - m + m * g;
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.sc.st.reset();
    }
    fn tail(&self) -> f32 {
        self.release.as_const().unwrap_or(0.2) * 3.0
    }
    fn sidechain(&self) -> Option<&str> {
        self.source.as_deref()
    }
    fn name(&self) -> String {
        "compressor".into()
    }
}

// ---- Limiter -----------------------------------------------------------------------------------

/// Sliding-window minimum (monotonic deque) over the last `len` values.
#[derive(Clone, Debug, Default)]
struct MinWindow {
    vals: Vec<f32>,
    idx: Vec<u64>,
    head: usize,
    count: usize,
    len: u64,
}
impl MinWindow {
    fn new(len: usize) -> MinWindow {
        let cap = len + 2;
        MinWindow {
            vals: vec![0.0; cap],
            idx: vec![0; cap],
            head: 0,
            count: 0,
            len: len as u64,
        }
    }
    #[inline]
    fn push(&mut self, i: u64, v: f32) -> f32 {
        let cap = self.vals.len();
        while self.count > 0 {
            let back = (self.head + self.count - 1) % cap;
            if self.vals[back] >= v {
                self.count -= 1;
            } else {
                break;
            }
        }
        let pos = (self.head + self.count) % cap;
        self.vals[pos] = v;
        self.idx[pos] = i;
        self.count += 1;
        while self.idx[self.head] + self.len <= i {
            self.head = (self.head + 1) % cap;
            self.count -= 1;
        }
        self.vals[self.head]
    }
    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }
}

/// Look-ahead brickwall limiter — the go-to last effect on the master.
///
/// Gain reduction is computed ahead of time (default 3 ms look-ahead) and applied with a smooth
/// attack ramp so peaks never exceed the ceiling, then released with `release`.
///
/// ```ignore
/// song.master().fx(Limiter::new().ceiling(-1.db()).gain(4.db()))
/// ```
#[derive(Clone, Debug)]
pub struct Limiter {
    ceiling: Param,
    release: Param,
    gain: Param,
    lookahead: f32,
    la: usize,
    delay: Vec<Frame>,
    dpos: usize,
    minw: MinWindow,
    boxv: Vec<f32>,
    box_sum: f64,
    bpos: usize,
    s: f32,
    i: u64,
    g_ramp: Ramp,
    c_ramp: Ramp,
    sr: f32,
}

impl Limiter {
    /// Ceiling -1 dB, release 100 ms, 3 ms look-ahead.
    pub fn new() -> Limiter {
        Limiter {
            ceiling: Param::Const(db_amp(-1.0)),
            release: Param::Const(0.1),
            gain: Param::Const(1.0),
            lookahead: 0.003,
            la: 1,
            delay: vec![],
            dpos: 0,
            minw: MinWindow::default(),
            boxv: vec![],
            box_sum: 0.0,
            bpos: 0,
            s: 1.0,
            i: 0,
            g_ramp: Ramp::default(),
            c_ramp: Ramp::default(),
            sr: 48_000.0,
        }
    }
    /// Alias of [`Limiter::new`] — reads well on the master bus.
    pub fn master() -> Limiter {
        Limiter::new()
    }
    /// Maximum output level (`-1.db()`, `-0.3.db()`).
    pub fn ceiling(mut self, c: impl Into<Param>) -> Self {
        self.ceiling = c.into();
        self
    }
    /// Release time in seconds.
    pub fn release(mut self, secs: impl Into<Param>) -> Self {
        self.release = secs.into();
        self
    }
    /// Input gain driving the limiter (`4.db()` = louder master).
    pub fn gain(mut self, g: impl Into<Param>) -> Self {
        self.gain = g.into();
        self
    }
    /// Alias of [`Limiter::gain`].
    pub fn drive(self, g: impl Into<Param>) -> Self {
        self.gain(g)
    }
    /// Look-ahead in seconds (0.1-20 ms; default 3 ms). Adds the same latency.
    pub fn lookahead(mut self, secs: f32) -> Self {
        self.lookahead = secs.clamp(0.0001, 0.02);
        self
    }
}

impl Effect for Limiter {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.la = ((self.lookahead * sr).round() as usize).max(1);
        self.delay = vec![Frame::ZERO; self.la];
        self.boxv = vec![1.0; self.la];
        self.minw = MinWindow::new(self.la);
        self.reset();
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if self.delay.is_empty() {
            self.prepare(ctx.sr);
        }
        let rel = coef(ctx.p(&self.release).max(0.001), self.sr);
        self.g_ramp.start(ctx.p(&self.gain).max(0.0), buf.len());
        self.c_ramp
            .start(level(ctx.p(&self.ceiling)).clamp(1e-4, 4.0), buf.len());
        let la = self.la;
        let inv = 1.0 / la as f64;
        for f in buf.iter_mut() {
            let x = *f * self.g_ramp.next();
            let ceil = self.c_ramp.next();
            let pk = x.peak();
            let r = if pk > ceil { ceil / pk } else { 1.0 };
            self.i += 1;
            let h = self.minw.push(self.i, r);
            self.s = if h < self.s {
                h
            } else {
                h + (self.s - h) * rel
            };
            self.box_sum += (self.s - self.boxv[self.bpos]) as f64;
            self.boxv[self.bpos] = self.s;
            self.bpos = (self.bpos + 1) % la;
            let g = (self.box_sum * inv) as f32;
            self.delay[self.dpos] = x;
            self.dpos = (self.dpos + 1) % la;
            let d = self.delay[self.dpos % la];
            let y = d * g.min(1.0);
            *f = Frame::new(y.l.clamp(-ceil, ceil), y.r.clamp(-ceil, ceil));
        }
    }
    fn reset(&mut self) {
        self.delay.iter_mut().for_each(|f| *f = Frame::ZERO);
        self.boxv.iter_mut().for_each(|v| *v = 1.0);
        self.box_sum = self.la as f64;
        self.minw.clear();
        self.s = 1.0;
        self.dpos = 0;
        self.bpos = 0;
    }
    fn tail(&self) -> f32 {
        self.lookahead + 0.05
    }
    fn name(&self) -> String {
        "limiter".into()
    }
}

// ---- Gate --------------------------------------------------------------------------------------

/// Noise gate / expander with hold and hysteresis; optionally keyed from another track.
///
/// ```ignore
/// Gate::new().threshold(-40.db()).release(80.ms())
/// Gate::new().range(-12.db())              // gentle expander instead of hard silence
/// Gate::new().sidechain("kick")            // rhythmic gating keyed by the kick
/// ```
#[derive(Clone, Debug)]
pub struct Gate {
    threshold: Param,
    attack: Param,
    hold: Param,
    release: Param,
    range: Param,
    source: Option<String>,
    env: f32,
    gain: f32,
    open: bool,
    hold_left: f32,
    sr: f32,
}

impl Gate {
    pub fn new() -> Gate {
        Gate {
            threshold: Param::Const(db_amp(-40.0)),
            attack: Param::Const(0.0005),
            hold: Param::Const(0.02),
            release: Param::Const(0.08),
            range: Param::Const(0.0),
            source: None,
            env: 0.0,
            gain: 0.0,
            open: false,
            hold_left: 0.0,
            sr: 48_000.0,
        }
    }
    /// Opening level (`-40.db()`). Closes 4 dB lower (hysteresis).
    pub fn threshold(mut self, t: impl Into<Param>) -> Self {
        self.threshold = t.into();
        self
    }
    pub fn attack(mut self, secs: impl Into<Param>) -> Self {
        self.attack = secs.into();
        self
    }
    /// Time the gate stays open after the signal drops (seconds).
    pub fn hold(mut self, secs: impl Into<Param>) -> Self {
        self.hold = secs.into();
        self
    }
    pub fn release(mut self, secs: impl Into<Param>) -> Self {
        self.release = secs.into();
        self
    }
    /// Gain while closed (default silence; `-12.db()` = gentle expander). Plain `0.0` = silence.
    pub fn range(mut self, g: impl Into<Param>) -> Self {
        self.range = g.into();
        self
    }
    /// Key the gate from another track/bus.
    pub fn sidechain(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

impl Effect for Gate {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        let open_lv = level(ctx.p(&self.threshold));
        let close_lv = open_lv * db_amp(-4.0);
        let att = coef(ctx.p(&self.attack).max(1e-5), sr);
        let rel = coef(ctx.p(&self.release).max(1e-4), sr);
        let hold = ctx.p(&self.hold).max(0.0) * sr;
        let floor = ctx.p(&self.range).clamp(0.0, 1.0);
        let det_rel = coef(0.003, sr);
        let sc = ctx.sidechain.filter(|s| s.len() >= buf.len());
        for (i, f) in buf.iter_mut().enumerate() {
            let det = match sc {
                Some(s) => s[i].peak(),
                None => f.peak(),
            };
            self.env = if det > self.env {
                det
            } else {
                crate::dsp::undenormal(det + (self.env - det) * det_rel)
            };
            if self.env >= open_lv {
                self.open = true;
                self.hold_left = hold;
            } else if self.env < close_lv {
                if self.hold_left > 0.0 {
                    self.hold_left -= 1.0;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { floor };
            self.gain = follow(self.gain, target, att, rel);
            *f *= self.gain;
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 0.0;
        self.open = false;
    }
    fn tail(&self) -> f32 {
        self.release.as_const().unwrap_or(0.1) * 3.0
    }
    fn sidechain(&self) -> Option<&str> {
        self.source.as_deref()
    }
    fn name(&self) -> String {
        "gate".into()
    }
}

// ---- Ducker ------------------------------------------------------------------------------------

/// The simplest sidechain pump: duck this track whenever `source` plays.
///
/// ```ignore
/// let bass = song.track("bass", bass_synth).fx(Ducker::new("kick").amount(0.8).release(150.ms()));
/// ```
#[derive(Clone, Debug)]
pub struct Ducker {
    source: String,
    amount: Param,
    attack: Param,
    release: Param,
    threshold: Param,
    env: f32,
    gain: f32,
    sr: f32,
}

impl Ducker {
    /// Duck when the track/bus named `source` is loud. Default: 70% duck, 2 ms attack, 140 ms release.
    pub fn new(source: impl Into<String>) -> Ducker {
        Ducker {
            source: source.into(),
            amount: Param::Const(0.7),
            attack: Param::Const(0.002),
            release: Param::Const(0.14),
            threshold: Param::Const(db_amp(-30.0)),
            env: 0.0,
            gain: 1.0,
            sr: 48_000.0,
        }
    }
    /// How far to duck: 0 = nothing, 1 = silence. Automatable.
    pub fn amount(mut self, a: impl Into<Param>) -> Self {
        self.amount = a.into();
        self
    }
    /// Alias of [`Ducker::amount`].
    pub fn depth(self, a: impl Into<Param>) -> Self {
        self.amount(a)
    }
    pub fn attack(mut self, secs: impl Into<Param>) -> Self {
        self.attack = secs.into();
        self
    }
    /// How long the level takes to come back (seconds) — the "pump" length.
    pub fn release(mut self, secs: impl Into<Param>) -> Self {
        self.release = secs.into();
        self
    }
    /// Sidechain level where ducking starts; full duck is reached 12 dB above (`-30.db()` default).
    pub fn threshold(mut self, t: impl Into<Param>) -> Self {
        self.threshold = t.into();
        self
    }
}

impl Effect for Ducker {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let Some(sc) = ctx.sidechain.filter(|s| s.len() >= buf.len()) else {
            // no key signal: just recover
            let rel = coef(ctx.p(&self.release).max(1e-4), self.sr);
            for f in buf.iter_mut() {
                self.gain = 1.0 + (self.gain - 1.0) * rel;
                *f *= self.gain;
            }
            return;
        };
        let sr = self.sr;
        let amount = ctx.p(&self.amount).clamp(0.0, 1.0);
        let att = coef(ctx.p(&self.attack).max(1e-5), sr);
        let rel = coef(ctx.p(&self.release).max(1e-4), sr);
        let thr = level_db(ctx.p(&self.threshold));
        let det_rel = coef(0.01, sr);
        for (i, f) in buf.iter_mut().enumerate() {
            let x = sc[i].peak();
            self.env = if x > self.env {
                x
            } else {
                crate::dsp::undenormal(x + (self.env - x) * det_rel)
            };
            let key = ((amp_db(self.env) - thr) / 12.0).clamp(0.0, 1.0);
            let key = key * key * (3.0 - 2.0 * key);
            let target = 1.0 - amount * key;
            self.gain = follow(self.gain, target, rel, att); // falling = attack, rising = release
            *f *= self.gain;
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 1.0;
    }
    fn tail(&self) -> f32 {
        0.0
    }
    fn sidechain(&self) -> Option<&str> {
        Some(&self.source)
    }
    fn name(&self) -> String {
        format!("ducker({})", self.source)
    }
}

// ---- TransientShaper ---------------------------------------------------------------------------

/// Boost or tame attacks and sustain independently of level.
///
/// ```ignore
/// TransientShaper::new().attack(0.6).sustain(-0.3)   // snappier, tighter drums
/// ```
#[derive(Clone, Debug)]
pub struct TransientShaper {
    attack: Param,
    sustain: Param,
    output: Param,
    fast: f32,
    slow: f32,
    long: f32,
    gain_db: f32,
    out_ramp: Ramp,
    sr: f32,
}

impl TransientShaper {
    pub fn new() -> TransientShaper {
        TransientShaper {
            attack: Param::Const(0.0),
            sustain: Param::Const(0.0),
            output: Param::Const(1.0),
            fast: 0.0,
            slow: 0.0,
            long: 0.0,
            gain_db: 0.0,
            out_ramp: Ramp::default(),
            sr: 48_000.0,
        }
    }
    /// Attack emphasis -1 (soften) .. 1 (punch).
    pub fn attack(mut self, a: impl Into<Param>) -> Self {
        self.attack = a.into();
        self
    }
    /// Sustain emphasis -1 (tighter) .. 1 (fuller, more room).
    pub fn sustain(mut self, s: impl Into<Param>) -> Self {
        self.sustain = s.into();
        self
    }
    pub fn output(mut self, g: impl Into<Param>) -> Self {
        self.output = g.into();
        self
    }
}

impl Effect for TransientShaper {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let sr = self.sr;
        let a = ctx.p(&self.attack).clamp(-1.0, 1.0);
        let s = ctx.p(&self.sustain).clamp(-1.0, 1.0);
        let (fa, fr) = (coef(0.0005, sr), coef(0.04, sr));
        let sa = coef(0.02, sr);
        let lr = coef(0.3, sr);
        let gs = coef(0.001, sr);
        self.out_ramp.start(ctx.p(&self.output), buf.len());
        for f in buf.iter_mut() {
            let x = f.peak();
            self.fast = follow(self.fast, x, fa, fr);
            self.slow = follow(self.slow, x, sa, fr);
            self.long = follow(self.long, x, fa, lr);
            let tr = (amp_db(self.fast) - amp_db(self.slow)).max(0.0);
            let su = (amp_db(self.long) - amp_db(self.fast)).max(0.0);
            let target = (a * tr.min(24.0) * 0.5 + s * su.min(24.0) * 0.5).clamp(-24.0, 12.0);
            self.gain_db = crate::dsp::undenormal(target + (self.gain_db - target) * gs);
            *f *= db_amp(self.gain_db) * self.out_ramp.next();
        }
    }
    fn reset(&mut self) {
        self.fast = 0.0;
        self.slow = 0.0;
        self.long = 0.0;
        self.gain_db = 0.0;
    }
    fn name(&self) -> String {
        "transient".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Units;

    fn run(
        fx: &mut dyn Effect,
        input: impl Fn(usize) -> Frame,
        sc: Option<&dyn Fn(usize) -> Frame>,
        n: usize,
    ) -> Vec<Frame> {
        let sr = 48_000.0;
        fx.prepare(sr);
        let mut out = vec![];
        let mut t = 0;
        while t < n {
            let mut buf = [Frame::ZERO; 64];
            let mut sbuf = [Frame::ZERO; 64];
            for k in 0..64 {
                buf[k] = input(t + k);
                if let Some(s) = sc {
                    sbuf[k] = s(t + k);
                }
            }
            let mut ctx = Ctx::offline(sr);
            if sc.is_some() {
                ctx.sidechain = Some(&sbuf);
            }
            fx.process(&mut buf, &ctx);
            out.extend_from_slice(&buf);
            t += 64;
        }
        out
    }
    fn sine(amp: f32) -> impl Fn(usize) -> Frame {
        move |t| Frame::mono(amp * (t as f32 * std::f32::consts::TAU * 220.0 / 48000.0).sin())
    }
    fn peak(v: &[Frame]) -> f32 {
        v.iter().fold(0f32, |m, f| m.max(f.peak()))
    }

    #[test]
    fn compressor_reduces_loud_signal() {
        let mut c = Compressor::new()
            .threshold(-20.db())
            .ratio(4.0)
            .knee(0.0)
            .attack(0.001);
        let out = run(&mut c, sine(1.0), None, 48_000);
        let p = peak(&out[24_000..]);
        // 0 dBFS in, 20 dB over → 5 dB over → -15 dB
        let db = 20.0 * p.log10();
        assert!((db + 15.0).abs() < 1.5, "{db}");
    }

    #[test]
    fn compressor_sidechain_keys_from_ctx() {
        let mut c = Compressor::pump().sidechain("kick");
        assert_eq!(Effect::sidechain(&c), Some("kick"));
        let key = |t: usize| {
            if (t / 12_000) % 2 == 0 {
                Frame::mono(1.0)
            } else {
                Frame::ZERO
            }
        };
        let out = run(&mut c, sine(0.1), Some(&key), 48_000);
        let ducked = peak(&out[6_000..12_000]);
        let free = peak(&out[20_000..24_000]);
        assert!(ducked < free * 0.3, "{ducked} {free}");
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let mut l = Limiter::new().ceiling(-1.db()).gain(12.db());
        let out = run(
            &mut l,
            |t| Frame::new(((t * 7919) % 1000) as f32 / 500.0 - 1.0, sine(1.0)(t).l),
            None,
            96_000,
        );
        let p = peak(&out);
        assert!(p <= db_amp(-1.0) + 1e-6, "{p}");
        assert!(p > db_amp(-1.5));
        assert!(out.iter().all(|f| f.l.is_finite()));
    }

    #[test]
    fn limiter_transparent_below_ceiling() {
        let mut l = Limiter::new().lookahead(0.002);
        let out = run(&mut l, sine(0.5), None, 9600);
        let d = 96; // 2 ms at 48k → latency la-1
        for t in 1000..9000 {
            assert!((out[t].l - sine(0.5)(t - (d - 1)).l).abs() < 1e-4);
        }
    }

    #[test]
    fn gate_and_ducker() {
        let mut g = Gate::new().threshold(-30.db());
        let out = run(
            &mut g,
            |t| {
                if t < 24_000 {
                    sine(0.5)(t)
                } else {
                    sine(0.001)(t)
                }
            },
            None,
            48_000,
        );
        assert!(peak(&out[10_000..20_000]) > 0.45);
        assert!(peak(&out[40_000..]) < 1e-4);

        let mut d = Ducker::new("kick").amount(0.8).release(150.ms());
        let key = |t: usize| {
            if t % 24_000 < 2_000 {
                Frame::mono(0.8)
            } else {
                Frame::ZERO
            }
        };
        let out = run(&mut d, |_| Frame::mono(0.5), Some(&key), 48_000);
        assert!((out[1_500].l - 0.1).abs() < 0.02, "{}", out[1_500].l);
        assert!(out[23_000].l > 0.45);
    }

    #[test]
    fn transient_shaper_is_finite() {
        let mut t = TransientShaper::new().attack(1.0).sustain(-1.0);
        let out = run(
            &mut t,
            |t| {
                if t % 12_000 < 3_000 {
                    sine(0.5)(t)
                } else {
                    Frame::ZERO
                }
            },
            None,
            48_000,
        );
        assert!(out.iter().all(|f| f.l.is_finite() && f.l.abs() < 5.0));
    }
}
