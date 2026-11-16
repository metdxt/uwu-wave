//! The articulated sample engine: articulation switching, CC1 dynamic-layer crossfades,
//! a legato (slur) engine with portamento, delayed vibrato, humanize and round robins.

use super::patch::{ArticPatch, instrument_dirs, load_dir};
use crate::clip::Artic;
use crate::ctx::Ctx;
use crate::dsp::{Frame, Rng, TAU};
use crate::instrument::{Instrument, NoteOn};
use std::sync::Arc;

/// Maximum dynamic layers crossfaded per voice.
pub const MAX_LAYERS: usize = 6;
/// Maximum sample heads per voice (dynamic layers × stacks).
const MAX_HEADS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Delay,
    Attack,
    Hold,
    Release,
    /// Legato hand-off: fading out.
    FadeOut,
}

#[derive(Clone)]
struct Voice {
    on: bool,
    id: u64,
    held: bool,
    oneshot: bool,
    patch: usize,
    zone: [usize; MAX_HEADS],
    pos: [f64; MAX_HEADS],
    /// Dynamic-layer centre of each head.
    hc: [f32; MAX_HEADS],
    /// Stack group of each head (heads of a group are contiguous and sorted soft → loud).
    hg: [u8; MAX_HEADS],
    nh: usize,
    multi: bool,
    pitch: f32,
    glide: f32,
    glide_k: f32,
    /// Live dynamics (follow CC1) or fixed (velocity).
    live: bool,
    w: f32,
    w_fixed: f32,
    stage: Stage,
    env: f32,
    att_inc: f32,
    rel_k: f32,
    delay_left: u32,
    xf: f32,
    xf_inc: f32,
    accent: f32,
    accent_k: f32,
    lp: [f32; 2],
    vib_ph: f32,
    vib_t: f32,
    vib_rate: f32,
    vib_depth_j: f32,
    hum: f32,
    age: u64,
    released_at: u64,
}

impl Voice {
    fn idle() -> Voice {
        Voice {
            on: false,
            id: u64::MAX,
            held: false,
            oneshot: false,
            patch: 0,
            zone: [usize::MAX; MAX_HEADS],
            pos: [0.0; MAX_HEADS],
            hc: [0.5; MAX_HEADS],
            hg: [0; MAX_HEADS],
            nh: 0,
            multi: false,
            pitch: 60.0,
            glide: 0.0,
            glide_k: 0.0,
            live: false,
            w: 0.5,
            w_fixed: 0.5,
            stage: Stage::Release,
            env: 0.0,
            att_inc: 1.0,
            rel_k: 0.0,
            delay_left: 0,
            xf: 1.0,
            xf_inc: 0.0,
            accent: 0.0,
            accent_k: 0.0,
            lp: [0.0; 2],
            vib_ph: 0.0,
            vib_t: 0.0,
            vib_rate: 5.3,
            vib_depth_j: 1.0,
            hum: 0.0,
            age: 0,
            released_at: 0,
        }
    }
}

/// Equal-power crossfade weights for heads grouped by stack (each group sorted by centre).
fn head_weights(hc: &[f32], hg: &[u8], w: f32, out: &mut [f32; MAX_HEADS]) {
    let n = hc.len();
    let mut a = 0;
    while a < n {
        let mut b = a + 1;
        while b < n && hg[b] == hg[a] {
            b += 1;
        }
        let c = &hc[a..b];
        let k = c.len();
        for o in out[a..b].iter_mut() {
            *o = 0.0;
        }
        if k == 1 || w <= c[0] {
            out[a] = 1.0;
        } else if w >= c[k - 1] {
            out[b - 1] = 1.0;
        } else {
            for i in 0..k - 1 {
                if w >= c[i] && w <= c[i + 1] {
                    let t = (w - c[i]) / (c[i + 1] - c[i]).max(1e-6);
                    let ang = t * std::f32::consts::FRAC_PI_2;
                    out[a + i] = ang.cos();
                    out[a + i + 1] = ang.sin();
                    break;
                }
            }
        }
        a = b;
    }
}

/// Articulation fallback order (the requested one first). `Normal` is resolved by the caller.
pub fn fallback_chain(a: Artic) -> &'static [Artic] {
    use Artic::*;
    match a {
        Staccatissimo => &[Staccatissimo, Staccato, Spiccato],
        Staccato => &[Staccato, Spiccato, Staccatissimo],
        Spiccato => &[Spiccato, Staccato, Staccatissimo],
        Pizzicato => &[Pizzicato],
        Legato => &[Legato, Sustain, Normal],
        Accent => &[Accent, Marcato, Sustain, Normal],
        Marcato => &[Marcato, Accent, Sustain, Normal],
        Tenuto => &[Tenuto, Sustain, Normal],
        Tremolo => &[Tremolo, Sustain, Normal],
        Trill => &[Trill, Tremolo, Sustain, Normal],
        Harmonic => &[Harmonic, Sustain, Normal],
        Muted => &[Muted, Sustain, Normal],
        Sustain => &[Sustain, Normal, Legato],
        Normal => &[Normal, Sustain],
        Custom(_) => &[],
    }
}

/// An articulated, dynamics-crossfading sampled instrument (strings, winds, brass, choir,
/// percussion...). Usually created through [`Orchestra`](super::Orchestra) presets.
///
/// * **Articulations** — each note's [`Artic`] selects a patch, with musical fallbacks
///   (staccatissimo → staccato → spiccato, legato/tenuto → sustain, accent/marcato → sustain
///   with an attack boost…). `Normal` plays the instrument's default articulation; with
///   [`Articulated::auto_short`] short `Normal` notes (< 0.18 s) use the staccato patch.
/// * **Dynamics** — CC1 ([`Track::dynamics`](crate::song::Track::dynamics)) crossfades the dynamic
///   layers *during* sustained notes (equal power), and drives a brightness filter and gain.
///   Short notes use velocity (blended with CC1 when the track has a dynamics lane). Tracks
///   without CC1 (value 0) use velocity throughout, so plain patterns just work. CC11
///   expression is a smooth gain on top.
/// * **Legato** — on legato instruments (sections, solo winds/brass), overlapping notes are
///   slurred: no new attack, the new sample starts past its attack and crossfades in while
///   the pitch glides from the old note ([`Articulated::legato_speed`],
///   [`Articulated::portamento`]). `Artic::Legato` or CC68 ≥ 0.5 slur on any instrument (also
///   across tiny gaps). Chords move voice-by-voice to the nearest held note.
/// * **Vibrato** — CC21 adds delayed vibrato (up to 0.35 st) with drifting rate; the default
///   [`Articulated::vibrato`] depth is 0 because sustain samples already contain vibrato.
/// * Pitch bend, release tails, round robins, ±3 cent / 0–6 ms humanize (seeded).
///
/// ```no_run
/// use uwu_wave::prelude::*;
/// let song = Song::new("adagio").bpm(64);
/// let vln = song.track("vln1", Orchestra::violins_1().legato_speed(0.11));
/// vln.dynamics(auto![0 => Dyn::P, bar(4) => Dyn::F => smooth]);
/// ```
pub struct Articulated {
    patches: Vec<(Artic, String, Arc<ArticPatch>)>,
    default: Artic,
    voices: Vec<Voice>,
    max_voices: usize,
    legato: bool,
    auto_short: bool,
    legato_speed: f32,
    portamento: f32,
    legato_skip: f32,
    vib_default: f32,
    hum_cents: f32,
    hum_ms: f32,
    gain: f32,
    oneshot_all: bool,
    seat: (f32, f32),
    name: String,
    sr: f32,
    rng: Rng,
    rr: u32,
    clock: u64,
    bend: f32,
    expr: f32,
    started: bool,
}

impl Clone for Articulated {
    fn clone(&self) -> Articulated {
        Articulated {
            voices: vec![],
            started: false,
            ..self.shallow()
        }
    }
}

impl Articulated {
    fn shallow(&self) -> Articulated {
        Articulated {
            patches: self.patches.clone(),
            default: self.default,
            voices: vec![],
            max_voices: self.max_voices,
            legato: self.legato,
            auto_short: self.auto_short,
            legato_speed: self.legato_speed,
            portamento: self.portamento,
            legato_skip: self.legato_skip,
            vib_default: self.vib_default,
            hum_cents: self.hum_cents,
            hum_ms: self.hum_ms,
            gain: self.gain,
            oneshot_all: self.oneshot_all,
            seat: self.seat,
            name: self.name.clone(),
            sr: self.sr,
            rng: self.rng.clone(),
            rr: 0,
            clock: 0,
            bend: 0.0,
            expr: 1.0,
            started: false,
        }
    }

    /// An empty instrument; add patches with [`Articulated::artic`].
    pub fn new(name: impl Into<String>) -> Articulated {
        Articulated {
            patches: vec![],
            default: Artic::Normal,
            voices: vec![],
            max_voices: 24,
            legato: false,
            auto_short: false,
            legato_speed: 0.09,
            portamento: 0.06,
            legato_skip: 0.1,
            vib_default: 0.0,
            hum_cents: 3.0,
            hum_ms: 6.0,
            gain: 1.0,
            oneshot_all: false,
            seat: (0.0, 0.5),
            name: name.into(),
            sr: 48_000.0,
            rng: Rng::new(0x0dc4_e57a),
            rr: 0,
            clock: 0,
            bend: 0.0,
            expr: 1.0,
            started: false,
        }
    }

    /// Load every articulation of a library instrument (`"strings/violins-1"`): one sub-dir per
    /// articulation, named like the [`Artic`] (`staccato`, `pizzicato` …; unknown names become
    /// `Artic::Custom(n)` in alphabetical order).
    pub fn load(instrument: &str) -> Result<Articulated, String> {
        let dirs = instrument_dirs(instrument);
        if dirs.is_empty() {
            return Err(format!(
                "uwu-wave: no articulations found for {instrument:?} under {} — available: {}",
                super::patch::library_root().display(),
                super::presets::available().join(", ")
            ));
        }
        let mut a = Articulated::new(instrument.rsplit('/').next().unwrap_or(instrument));
        let mut custom = 0u8;
        for (name, dir) in dirs {
            let artic = Artic::parse(&name).unwrap_or_else(|| {
                custom += 1;
                Artic::Custom(custom - 1)
            });
            match load_dir(&dir, &format!("{instrument}/{name}")) {
                Ok(p) => a = a.artic_named(artic, &name, p),
                Err(e) => eprintln!("uwu-wave: skipping {e}"),
            }
        }
        if a.patches.is_empty() {
            return Err(format!(
                "uwu-wave: {instrument:?}: no loadable articulations"
            ));
        }
        a.default = a.pick_default();
        Ok(a)
    }

    fn pick_default(&self) -> Artic {
        for c in [Artic::Sustain, Artic::Normal, Artic::Legato] {
            if self.has(c) {
                return c;
            }
        }
        self.patches.first().map(|p| p.0).unwrap_or(Artic::Normal)
    }

    /// Add an articulation patch.
    pub fn artic(self, a: Artic, patch: ArticPatch) -> Articulated {
        let name = format!("{a:?}").to_lowercase();
        self.artic_named(a, &name, patch)
    }
    fn artic_named(mut self, a: Artic, name: &str, patch: ArticPatch) -> Articulated {
        let patch = if patch.layers.len() == 1 && patch.zones.iter().any(|z| z.layer != 0) {
            patch.finish()
        } else {
            patch
        };
        self.patches.retain(|p| p.0 != a);
        self.patches.push((a, name.to_string(), Arc::new(patch)));
        if self.patches.len() == 1 || !self.has(self.default) {
            self.default = self.pick_default();
        }
        self
    }
    /// Does the instrument have a patch for exactly this articulation?
    pub fn has(&self, a: Artic) -> bool {
        self.patches.iter().any(|p| p.0 == a)
    }
    /// Playable key range of the default articulation.
    pub fn range(&self) -> (f32, f32) {
        self.patches
            .iter()
            .find(|p| p.0 == self.default)
            .or(self.patches.first())
            .map(|p| p.2.range())
            .unwrap_or((60.0, 60.0))
    }
    /// Articulation names available.
    pub fn articulations(&self) -> Vec<String> {
        self.patches.iter().map(|p| p.1.clone()).collect()
    }
    /// Look up a custom articulation by its directory name (e.g. `"col-legno"`).
    pub fn custom(&self, name: &str) -> Option<Artic> {
        self.patches.iter().find(|p| p.1 == name).map(|p| p.0)
    }
    /// The articulation `Normal` notes play.
    pub fn default_artic(mut self, a: Artic) -> Articulated {
        if self.has(a) {
            self.default = a;
        }
        self
    }
    /// Legato engine for overlapping notes (default on for sections and solo winds/brass).
    pub fn legato(mut self, on: bool) -> Articulated {
        self.legato = on;
        self
    }
    /// Short `Normal` notes (< 0.18 s) play the staccato patch when available.
    pub fn auto_short(mut self, on: bool) -> Articulated {
        self.auto_short = on;
        self
    }
    /// Legato crossfade time in seconds (default 0.09; slower = more singing).
    pub fn legato_speed(mut self, secs: f32) -> Articulated {
        self.legato_speed = secs.max(0.005);
        self
    }
    /// Portamento base time in seconds for slurs (scaled by interval; default 0.06).
    pub fn portamento(mut self, secs: f32) -> Articulated {
        self.portamento = secs.max(0.0);
        self
    }
    /// How far into the new note's sample a slur starts (skips its attack; default 0.1 s).
    pub fn legato_skip(mut self, secs: f32) -> Articulated {
        self.legato_skip = secs.max(0.0);
        self
    }
    /// Default vibrato depth in semitones (added to CC21 × 0.35 st). Default 0.
    pub fn vibrato(mut self, semitones: f32) -> Articulated {
        self.vib_default = semitones.max(0.0);
        self
    }
    /// Humanize: random tuning ±`cents` and start delay 0..`ms` per note (seeded).
    pub fn humanize(mut self, cents: f32, ms: f32) -> Articulated {
        self.hum_cents = cents.max(0.0);
        self.hum_ms = ms.max(0.0);
        self
    }
    /// Random seed for humanize / vibrato drift.
    pub fn seed(mut self, s: u64) -> Articulated {
        self.rng = Rng::new(s);
        self
    }
    /// Output gain (linear or `-3.db()`).
    pub fn gain(mut self, g: impl crate::units::Val) -> Articulated {
        self.gain = g.val();
        self
    }
    /// Maximum polyphony (default 24).
    pub fn voices(mut self, n: usize) -> Articulated {
        self.max_voices = n.max(1);
        self
    }
    /// Treat every note as a one-shot (percussion: note-offs are ignored).
    pub fn one_shot(mut self, on: bool) -> Articulated {
        self.oneshot_all = on;
        self
    }
    /// Release time override for every articulation (seconds).
    pub fn release(mut self, secs: f32) -> Articulated {
        for p in &mut self.patches {
            Arc::make_mut(&mut p.2).release = secs;
        }
        self
    }
    /// Default stage position (pan −1..1, depth 0 = front .. 1 = back).
    pub fn seat(&self) -> (f32, f32) {
        self.seat
    }
    /// Change the stage position hint.
    pub fn place(mut self, pan: f32, depth: f32) -> Articulated {
        self.seat = (pan.clamp(-1.0, 1.0), depth.clamp(0.0, 1.0));
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> Articulated {
        self.name = name.into();
        self
    }

    /// Resolve which patch a note uses (index into the patch list) and whether an accent
    /// boost should be applied (accent fell back to a sustain patch).
    pub fn resolve(&self, artic: Artic, dur: f32) -> Option<(usize, bool)> {
        let find = |a: Artic| self.patches.iter().position(|p| p.0 == a);
        let mut req = artic;
        if req == Artic::Normal {
            if self.auto_short && dur > 0.0 && dur < 0.18 && self.has(Artic::Staccato) {
                req = Artic::Staccato;
            } else {
                req = self.default;
            }
        }
        if let Artic::Custom(_) = req
            && let Some(i) = find(req)
        {
            return Some((i, false));
        }
        for &a in fallback_chain(req) {
            if let Some(i) = find(a) {
                let boost = matches!(artic, Artic::Accent | Artic::Marcato)
                    && !matches!(a, Artic::Accent | Artic::Marcato);
                return Some((i, boost));
            }
        }
        // short requests fall back to the default but get cut short by the caller's dur
        find(self.default)
            .or(if self.patches.is_empty() {
                None
            } else {
                Some(0)
            })
            .map(|i| (i, false))
    }

    /// The articulation that a request resolves to.
    pub fn resolved_artic(&self, artic: Artic, dur: f32) -> Option<Artic> {
        self.resolve(artic, dur).map(|(i, _)| self.patches[i].0)
    }

    fn is_oneshot(&self, pi: usize) -> bool {
        self.oneshot_all || self.patches[pi].0.is_short() || self.patches[pi].2.oneshot
    }

    fn alloc(&mut self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.on) {
            return i;
        }
        if self.voices.len() < self.max_voices {
            self.voices.push(Voice::idle());
            return self.voices.len() - 1;
        }
        // steal the quietest released voice, else the oldest
        let mut best = 0;
        let mut score = f32::MAX;
        for (i, v) in self.voices.iter().enumerate() {
            let s = if v.held {
                10.0 + v.age as f32 * 1e-9
            } else {
                v.env * v.xf
            } - (self.clock - v.age) as f32 * 1e-12;
            if s < score {
                score = s;
                best = i;
            }
        }
        best
    }

    fn start_voice(
        &mut self,
        vi: usize,
        note: &NoteOn,
        ctx: &Ctx,
        pi: usize,
        boost: bool,
        legato_from: Option<usize>,
    ) {
        let sr = self.sr;
        let patch = self.patches[pi].2.clone();
        self.rr = self.rr.wrapping_add(1);
        let cc1 = ctx.dynamics();
        let oneshot = self.is_oneshot(pi);
        let live = !oneshot && cc1 > 0.0;
        let w_fixed = if oneshot && cc1 > 0.0 {
            0.5 * note.vel + 0.5 * cc1
        } else {
            note.vel
        };
        let hum = self.rng.bipolar() * self.hum_cents / 100.0;
        let delay = if legato_from.is_some() {
            0
        } else {
            (self.rng.f32() * self.hum_ms * 0.001 * sr) as u32
        };
        let vib_rate = 5.0 + self.rng.range(-0.4, 0.5);
        let vib_j = self.rng.range(0.85, 1.15);
        let rr = self.rr;
        let mut v = Voice::idle();
        v.on = true;
        v.id = note.id;
        v.held = true;
        v.oneshot = oneshot;
        v.patch = pi;
        let mut h = 0;
        for (g, st) in patch.stacks().into_iter().enumerate() {
            for (c, z) in patch.note_layers(note.pitch, st, rr) {
                if h >= MAX_HEADS {
                    break;
                }
                let zn = &patch.zones[z];
                let s = &zn.sample;
                let mut p = if zn.start > 0 { zn.start } else { s.onset() } as f64;
                if legato_from.is_some() {
                    p += (self.legato_skip * s.sr) as f64;
                    p = p.min(zn.start as f64 + (zn.end - zn.start) as f64 * 0.5);
                }
                v.zone[h] = z;
                v.hc[h] = c;
                v.hg[h] = g as u8;
                v.pos[h] = p;
                h += 1;
            }
        }
        v.nh = h;
        v.multi = (1..h).any(|i| v.hg[i] == v.hg[i - 1]);
        v.pitch = note.pitch;
        v.live = live;
        v.w_fixed = w_fixed.clamp(0.0, 1.0);
        v.w = if live { cc1 } else { v.w_fixed };
        v.att_inc = 1.0 / (patch.attack.max(0.001) * sr);
        v.rel_k = (-6.9 / (patch.release.max(0.01) * sr)).exp();
        v.hum = hum;
        v.vib_rate = vib_rate;
        v.vib_depth_j = vib_j;
        v.age = self.clock;
        v.delay_left = delay;
        v.stage = if delay > 0 {
            Stage::Delay
        } else {
            Stage::Attack
        };
        if boost {
            v.accent = 0.9;
            v.accent_k = (-1.0 / (0.12 * sr)).exp();
        }
        if let Some(si) = legato_from {
            let src = self.voices[si].clone();
            let from = src.pitch + src.glide;
            let interval = (note.pitch - from).abs();
            let port = (self.portamento * (0.5 + interval / 12.0)).clamp(0.0, 0.35);
            let gk = if port <= 0.0 {
                0.0
            } else {
                (-3.0 / (port * sr)).exp()
            };
            v.glide = from - note.pitch;
            v.glide_k = gk;
            v.env = 1.0;
            v.stage = Stage::Hold;
            v.xf = 0.0;
            v.xf_inc = 1.0 / (self.legato_speed * sr);
            v.w = src.w;
            v.lp = src.lp;
            v.vib_ph = src.vib_ph;
            v.vib_t = src.vib_t.max(0.5);
            v.vib_rate = src.vib_rate;
            // the outgoing note slides toward the new pitch while it fades
            let s = &mut self.voices[si];
            s.stage = Stage::FadeOut;
            s.held = false;
            s.id = u64::MAX;
            s.xf_inc = -1.0 / (self.legato_speed * sr);
            s.glide_k = gk;
            s.pitch = note.pitch;
            s.glide = from - note.pitch;
        }
        self.voices[vi] = v;
    }

    /// Find a voice to slur from (nearest pitch), if this note should be played legato.
    fn legato_source(&self, note: &NoteOn, ctx: &Ctx, pi: usize) -> Option<usize> {
        if self.is_oneshot(pi) {
            return None;
        }
        let forced = note.artic == Artic::Legato || ctx.cc(68) >= 0.5;
        if !(self.legato || forced) {
            return None;
        }
        let recent = (0.08 * self.sr) as u64;
        let fresh = (0.012 * self.sr) as u64;
        let mut best = None;
        let mut bd = f32::MAX;
        for (i, v) in self.voices.iter().enumerate() {
            if !v.on || v.oneshot || v.stage == Stage::FadeOut || self.clock - v.age < fresh {
                continue;
            }
            let ok = v.held
                || (forced && v.stage == Stage::Release && self.clock - v.released_at < recent);
            if !ok {
                continue;
            }
            let d = (v.pitch - note.pitch).abs();
            if d < bd {
                bd = d;
                best = Some(i);
            }
        }
        best
    }

    fn render_voices(&mut self, out: &mut [Frame], ctx: &Ctx) {
        let n = out.len();
        if n == 0 {
            return;
        }
        let sr = self.sr;
        let nf = n as f32;
        let b0 = self.bend;
        let db = (ctx.bend - b0) / nf;
        let e0 = self.expr;
        let de = (ctx.expression().clamp(0.0, 1.0) - e0) / nf;
        let cc1 = ctx.dynamics().clamp(0.0, 1.0);
        let vib_depth_cc = ctx.cc(21).clamp(0.0, 1.0) * 0.35 + self.vib_default;
        let wk = 1.0 - (-1.0 / (0.03 * sr)).exp();
        let gain = self.gain;
        for v in self.voices.iter_mut().filter(|v| v.on) {
            let patch = &self.patches[v.patch].2;
            let multi = v.multi;
            let w_target = if v.live && cc1 > 0.0 { cc1 } else { v.w_fixed };
            let mut bend = b0;
            let mut expr = e0;
            let mut ended = false;
            for f in out.iter_mut() {
                bend += db;
                expr += de;
                match v.stage {
                    Stage::Delay => {
                        if v.delay_left > 0 {
                            v.delay_left -= 1;
                            continue;
                        }
                        v.stage = Stage::Attack;
                    }
                    Stage::Attack => {
                        v.env += v.att_inc;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Hold;
                        }
                    }
                    Stage::Hold => {}
                    Stage::Release => {
                        v.env *= v.rel_k;
                        if v.env < 1e-4 {
                            ended = true;
                            break;
                        }
                    }
                    Stage::FadeOut => {}
                }
                if v.xf_inc != 0.0 {
                    v.xf += v.xf_inc;
                    if v.xf >= 1.0 {
                        v.xf = 1.0;
                        v.xf_inc = 0.0;
                    } else if v.xf <= 0.0 {
                        ended = true;
                        break;
                    }
                }
                v.w += (w_target - v.w) * wk;
                v.glide *= v.glide_k;
                // vibrato: fades in after ~0.3 s, slowly drifting rate
                let mut vib = 0.0;
                if vib_depth_cc > 0.0 {
                    v.vib_t += 1.0 / sr;
                    let fade = ((v.vib_t - 0.3) / 0.4).clamp(0.0, 1.0);
                    let rate = v.vib_rate * (1.0 + 0.06 * (v.vib_t * 0.7 + v.hum * 50.0).sin());
                    v.vib_ph += rate / sr;
                    if v.vib_ph >= 1.0 {
                        v.vib_ph -= 1.0;
                    }
                    vib = (TAU * v.vib_ph).sin() * vib_depth_cc * v.vib_depth_j * fade;
                }
                let semis = v.pitch + v.glide + bend + v.hum + vib + patch.tune;
                let mut ws = [0.0f32; MAX_HEADS];
                head_weights(&v.hc[..v.nh], &v.hg[..v.nh], v.w, &mut ws);
                let mut acc = Frame::ZERO;
                let mut alive = false;
                for hd in 0..v.nh {
                    let z = v.zone[hd];
                    let wl = ws[hd];
                    let zone = &patch.zones[z];
                    let s = &zone.sample;
                    let rate =
                        ((semis - zone.root + zone.tune) / 12.0).exp2() as f64 * (s.sr / sr) as f64;
                    let len = zone.end as f64;
                    if wl > 1e-4 {
                        acc += s.hermite(v.pos[hd]) * (wl * zone.gain);
                    }
                    v.pos[hd] += rate;
                    if let Some((a, b)) = zone.loop_pts
                        && v.pos[hd] >= b as f64
                    {
                        v.pos[hd] -= (b - a) as f64;
                    }
                    if wl > 1e-4 && v.pos[hd] < len - 2.0 {
                        alive = true;
                    }
                }
                if !alive {
                    ended = true;
                    break;
                }
                // dynamics → brightness and gain (gentler when real layers exist)
                let (lo_hz, dgain) = if multi {
                    (2500.0f32, 0.6 + 0.4 * v.w)
                } else {
                    (900.0f32, 0.18 + 0.82 * v.w.powf(1.3))
                };
                let cut = (lo_hz.log2() + (18_000.0f32 / lo_hz).log2() * v.w).exp2();
                let a = 1.0 - (-TAU * cut.min(sr * 0.45) / sr).exp();
                v.lp[0] += a * (acc.l - v.lp[0]);
                v.lp[1] += a * (acc.r - v.lp[1]);
                let mut g = v.env * v.xf * expr * dgain * patch.norm * patch.gain * gain;
                if v.accent > 1e-4 {
                    g *= 1.0 + v.accent;
                    v.accent *= v.accent_k;
                }
                // equal-power legato crossfade
                g *= (v.xf * std::f32::consts::FRAC_PI_2).sin() / v.xf.max(1e-6);
                *f += Frame::new(v.lp[0] * g, v.lp[1] * g);
            }
            if ended {
                v.on = false;
            }
        }
        self.bend = ctx.bend;
        self.expr = ctx.expression().clamp(0.0, 1.0);
        self.clock += n as u64;
    }
}

impl Instrument for Articulated {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.voices = Vec::with_capacity(self.max_voices);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        if !self.started {
            self.started = true;
            self.bend = ctx.bend;
            self.expr = ctx.expression().clamp(0.0, 1.0);
        }
        let Some((pi, boost)) = self.resolve(note.artic, note.dur) else {
            return;
        };
        let src = self.legato_source(note, ctx, pi);
        let vi = self.alloc();
        // the allocator may have stolen the legato source itself
        let src = src.filter(|&s| s != vi);
        self.start_voice(vi, note, ctx, pi, boost, src);
    }
    fn note_off(&mut self, id: u64, _ctx: &Ctx) {
        let clock = self.clock;
        for v in self.voices.iter_mut() {
            if v.on && v.id == id && v.held {
                v.held = false;
                if !v.oneshot {
                    v.stage = Stage::Release;
                    v.released_at = clock;
                }
            }
        }
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        if !self.started {
            self.bend = ctx.bend;
            self.expr = ctx.expression().clamp(0.0, 1.0);
        }
        self.render_voices(out, ctx);
    }
    fn active(&self) -> bool {
        self.voices.iter().any(|v| v.on)
    }
    fn panic(&mut self) {
        self.voices.iter_mut().for_each(|v| v.on = false);
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::patch::{ArticZone, synth_sample};

    fn peak(b: &[Frame]) -> f32 {
        b.iter().fold(0.0, |m, f| m.max(f.peak()))
    }
    fn rms(b: &[Frame]) -> f32 {
        (b.iter().map(|f| f.l * f.l).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }
    fn zc_hz(b: &[Frame]) -> f32 {
        let z = b
            .windows(2)
            .filter(|w| (w[0].l <= 0.0) != (w[1].l <= 0.0))
            .count();
        z as f32 / 2.0 / (b.len() as f32 / 48_000.0)
    }

    fn sustain_patch(burst: bool) -> ArticPatch {
        let soft = Arc::new(synth_sample(220.0, 0.15, 3.0, burst));
        let loud = Arc::new(synth_sample(220.0, 0.6, 3.0, burst));
        ArticPatch::new("sus")
            .zone(
                ArticZone::new(soft, 57.0)
                    .keys(40.0, 80.0)
                    .layer(0.0, 0.5)
                    .looped(48_000, 120_000),
            )
            .zone(
                ArticZone::new(loud, 57.0)
                    .keys(40.0, 80.0)
                    .layer(0.5, 1.0)
                    .looped(48_000, 120_000),
            )
            .attack(0.01)
            .release(0.2)
            .finish()
    }
    fn stacc_patch() -> ArticPatch {
        ArticPatch::new("stacc")
            .zone(ArticZone::new(synth_sample(440.0, 0.5, 0.25, false), 57.0).keys(40.0, 80.0))
            .finish()
    }
    fn inst() -> Articulated {
        Articulated::new("test")
            .artic(Artic::Sustain, sustain_patch(false))
            .artic(Artic::Staccato, stacc_patch())
            .humanize(0.0, 0.0)
    }

    /// Render notes (start_s, pitch, vel, dur_s, artic) with a per-block ctx mutator.
    fn render(
        a: &mut Articulated,
        notes: &[(f32, f32, f32, f32, Artic)],
        secs: f32,
        mut cc: impl FnMut(f32, &mut [f32; 128]),
    ) -> Vec<Frame> {
        let sr = 48_000.0;
        a.prepare(sr);
        let total = (secs * sr) as usize;
        let mut out = vec![Frame::ZERO; total];
        let mut ccs = crate::ctx::DEFAULT_CC;
        let mut evs: Vec<(usize, bool, usize)> = vec![];
        for (i, n) in notes.iter().enumerate() {
            evs.push(((n.0 * sr) as usize, true, i));
            evs.push((((n.0 + n.3) * sr) as usize, false, i));
        }
        evs.sort_by_key(|e| (e.0, e.1));
        let mut ei = 0;
        let mut pos = 0;
        while pos < total {
            cc(pos as f32 / sr, &mut ccs);
            let ctx = Ctx {
                cc: &ccs,
                modwheel: ccs[1],
                ..Ctx::offline(sr)
            }
            .advanced(pos);
            while ei < evs.len() && evs[ei].0 <= pos {
                let (_, on, i) = evs[ei];
                let n = notes[i];
                if on {
                    a.note_on(
                        &NoteOn {
                            id: i as u64 + 1,
                            pitch: n.1,
                            vel: n.2,
                            dur: n.3,
                            artic: n.4,
                        },
                        &ctx,
                    );
                } else {
                    a.note_off(i as u64 + 1, &ctx);
                }
                ei += 1;
            }
            let mut m = 64.min(total - pos);
            if ei < evs.len() {
                m = m.min(evs[ei].0.saturating_sub(pos).max(1));
            }
            a.render(&mut out[pos..pos + m], &ctx);
            pos += m;
        }
        out
    }

    #[test]
    fn articulation_selection_and_fallback() {
        let a = inst().auto_short(true);
        assert_eq!(a.resolved_artic(Artic::Normal, 1.0), Some(Artic::Sustain));
        assert_eq!(a.resolved_artic(Artic::Normal, 0.1), Some(Artic::Staccato));
        assert_eq!(
            a.resolved_artic(Artic::Staccatissimo, 0.1),
            Some(Artic::Staccato)
        );
        assert_eq!(
            a.resolved_artic(Artic::Spiccato, 0.1),
            Some(Artic::Staccato)
        );
        assert_eq!(a.resolved_artic(Artic::Legato, 1.0), Some(Artic::Sustain));
        assert_eq!(a.resolved_artic(Artic::Tenuto, 1.0), Some(Artic::Sustain));
        assert_eq!(a.resolve(Artic::Accent, 1.0), Some((0, true)));
        assert_eq!(
            a.resolved_artic(Artic::Pizzicato, 0.2),
            Some(Artic::Sustain)
        );
        let b = inst().auto_short(false);
        assert_eq!(b.resolved_artic(Artic::Normal, 0.1), Some(Artic::Sustain));
        // the staccato patch actually plays (440 Hz sample, 1 octave above the sustain one)
        let mut a = inst();
        let out = render(
            &mut a,
            &[(0.0, 57.0, 0.8, 0.1, Artic::Staccato)],
            0.4,
            |_, _| {},
        );
        assert!((zc_hz(&out[2400..9600]) - 440.0).abs() < 10.0);
        let mut a = inst();
        let out = render(
            &mut a,
            &[(0.0, 57.0, 0.8, 0.5, Artic::Normal)],
            0.4,
            |_, _| {},
        );
        assert!((zc_hz(&out[2400..9600]) - 220.0).abs() < 6.0);
    }

    #[test]
    fn cc1_crossfade_is_continuous_and_louder() {
        let mut a = inst();
        let out = render(
            &mut a,
            &[(0.0, 57.0, 0.5, 2.0, Artic::Normal)],
            2.0,
            |t, cc| cc[1] = (0.1 + 0.9 * t / 1.5).min(1.0),
        );
        assert!(out.iter().all(|f| f.l.is_finite()));
        let p = peak(&out);
        let max_step = out
            .windows(2)
            .map(|w| (w[1].l - w[0].l).abs())
            .fold(0.0, f32::max);
        // a 220 Hz sine moves at most 2π·220/48000 ≈ 0.029 of its amplitude per sample
        assert!(max_step < 0.05 * p, "step {max_step} peak {p}");
        let soft = rms(&out[4800..14_400]);
        let loud = rms(&out[80_000..90_000]);
        assert!(loud > soft * 3.0, "{soft} → {loud}");
    }

    #[test]
    fn velocity_is_dynamics_without_cc1() {
        let mut a = inst();
        let soft = rms(&render(
            &mut a,
            &[(0.0, 57.0, 0.2, 0.5, Artic::Normal)],
            0.5,
            |_, _| {},
        )[4800..]);
        let mut a = inst();
        let loud = rms(&render(
            &mut a,
            &[(0.0, 57.0, 1.0, 0.5, Artic::Normal)],
            0.5,
            |_, _| {},
        )[4800..]);
        assert!(loud > soft * 2.0);
    }

    #[test]
    fn legato_has_no_new_attack_and_glides() {
        let mk = |leg: bool| {
            Articulated::new("t")
                .artic(Artic::Sustain, sustain_patch(true))
                .humanize(0.0, 0.0)
                .legato(leg)
        };
        let notes = [
            (0.0, 57.0, 0.8, 0.6, Artic::Normal),
            (0.5, 60.0, 0.8, 0.6, Artic::Normal),
        ];
        let poly = render(&mut mk(false), &notes, 1.2, |_, _| {});
        let leg = render(&mut mk(true), &notes, 1.2, |_, _| {});
        let w = |o: &[Frame]| peak(&o[24_000..24_000 + 2400]);
        // the re-attacked note includes the loud burst; the slurred one doesn't
        assert!(
            w(&leg) < 0.5 * w(&poly),
            "legato {} vs poly {}",
            w(&leg),
            w(&poly)
        );
        // no gap: level stays up through the transition
        let env: Vec<f32> = leg[22_000..30_000].chunks(480).map(peak).collect();
        let lo = env.iter().cloned().fold(f32::MAX, f32::min);
        assert!(lo > 0.4 * peak(&leg[10_000..20_000]), "dip {lo}");
        // ends on the new pitch (C4 = 261.6 Hz)
        assert!((zc_hz(&leg[36_000..50_000]) - 261.6).abs() < 6.0);
        assert!(leg.iter().all(|f| f.l.is_finite()));
    }

    #[test]
    fn cc68_and_artic_legato_slur_without_legato_mode() {
        let mk = || {
            Articulated::new("t")
                .artic(Artic::Sustain, sustain_patch(true))
                .humanize(0.0, 0.0)
        };
        let notes = [
            (0.0, 57.0, 0.8, 0.6, Artic::Normal),
            (0.5, 60.0, 0.8, 0.6, Artic::Legato),
        ];
        let leg = render(&mut mk(), &notes, 1.2, |_, _| {});
        let poly = render(
            &mut mk(),
            &[
                (0.0, 57.0, 0.8, 0.6, Artic::Normal),
                (0.5, 60.0, 0.8, 0.6, Artic::Normal),
            ],
            1.2,
            |_, _| {},
        );
        let w = |o: &[Frame]| peak(&o[24_000..24_000 + 2400]);
        assert!(w(&leg) < 0.5 * w(&poly));
        let cc = render(
            &mut mk(),
            &[
                (0.0, 57.0, 0.8, 0.6, Artic::Normal),
                (0.5, 60.0, 0.8, 0.6, Artic::Normal),
            ],
            1.2,
            |_, c| c[68] = 1.0,
        );
        assert!(w(&cc) < 0.5 * w(&poly));
    }

    #[test]
    fn release_tail_ends_and_bend_applies() {
        let mut a = inst();
        let out = render(
            &mut a,
            &[(0.0, 57.0, 0.8, 0.3, Artic::Normal)],
            1.5,
            |_, _| {},
        );
        assert!(!a.active());
        assert!(peak(&out[40_000..]) < 1e-3);
        let sr = 48_000.0;
        let mut a = inst();
        a.prepare(sr);
        let mut ctx = Ctx::offline(sr);
        ctx.bend = 12.0;
        a.note_on(&NoteOn::new(1, 57.0, 0.8, 1.0), &ctx);
        let mut o = vec![Frame::ZERO; 24_000];
        for c in o.chunks_mut(64) {
            a.render(c, &ctx);
        }
        assert!((zc_hz(&o[4800..]) - 440.0).abs() < 8.0);
    }

    #[test]
    fn humanize_is_seeded() {
        let a1 = render(
            &mut inst().humanize(3.0, 6.0).seed(9),
            &[(0.0, 57.0, 0.8, 0.3, Artic::Normal)],
            0.3,
            |_, _| {},
        );
        let a2 = render(
            &mut inst().humanize(3.0, 6.0).seed(9),
            &[(0.0, 57.0, 0.8, 0.3, Artic::Normal)],
            0.3,
            |_, _| {},
        );
        assert_eq!(a1, a2);
    }
}
