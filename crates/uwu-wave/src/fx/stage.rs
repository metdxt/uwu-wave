//! Orchestral seating: [`Stage`] places a (section) track on a virtual concert stage.

use super::Effect;
use super::filter::{SvfMode, SvfStage};
use super::util::{Ring, Smooth};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::Param;
use std::f32::consts::FRAC_PI_4;

/// Early-reflection taps: (milliseconds at depth scale 1, gain, output channel).
const ER_TAPS: [(f32, f32, usize); 8] = [
    (9.1, 0.55, 0),
    (11.3, 0.50, 1),
    (15.7, 0.42, 0),
    (18.9, 0.38, 1),
    (23.3, 0.32, 0),
    (26.1, 0.29, 1),
    (31.9, 0.22, 0),
    (37.7, 0.19, 1),
];
const ER_MAX: f32 = 0.08;
const HAAS_MAX: f32 = 0.03;

/// Seats a track on an orchestral stage.
///
/// `depth` runs from 0 (front of the stage, close to the listener) to 1 (back wall). It drives
/// distance attenuation, a high-frequency roll-off (air absorption), a few milliseconds of
/// Haas/arrival delay and the level of an early-reflection cluster. `pan` positions the section
/// (-1 left … 1 right, constant power) and `width` sets how much of the stage it spans
/// (0 = point source, 1 = keep the full input stereo image).
///
/// Use it as the first insert on a section track; send to a shared hall reverb afterwards.
///
/// ```ignore
/// song.track("vln1", Sampler::library("strings/violins-1/sustain")).fx(Stage::violins_1());
/// song.track("horns", horns).fx(Stage::horns());
/// song.track("x", synth).fx(Stage::seat(-0.3, 0.6).width(0.3));
/// song.track("vla", violas).fx(Stage::at("violas"));
/// ```
///
/// Presets follow the standard American seating (left → right: 1st violins, 2nd violins,
/// violas, cellos, basses on the right; woodwinds centre; horns left-back, trumpets/trombones/
/// tuba right-back; percussion and timpani at the back; harp left).
#[derive(Clone, Debug)]
pub struct Stage {
    pan: Param,
    depth: Param,
    width: Param,
    er: Param,
    distance_db: f32,
    air_db: f32,
    haas: f32,
    label: String,
    // runtime
    sm_depth: Smooth,
    shelf: SvfStage,
    g: [Ramp; 4],
    pre: [Ring; 2],
    er_line: Ring,
    er_lp: [f32; 2],
    er_amt: Ramp,
    sr: f32,
    prepared: bool,
}

/// Seating table: (names, pan, depth, width).
const SEATS: &[(&[&str], f32, f32, f32)] = &[
    (
        &["violins-1", "violins1", "violin-1", "vln1", "first-violins"],
        -0.62,
        0.12,
        0.45,
    ),
    (
        &[
            "violins-2",
            "violins2",
            "violin-2",
            "vln2",
            "second-violins",
        ],
        -0.32,
        0.28,
        0.40,
    ),
    (&["violas", "viola", "vla"], 0.12, 0.30, 0.40),
    (&["cellos", "cello", "vc", "violoncello"], 0.45, 0.22, 0.40),
    (
        &["basses", "bass", "double-bass", "contrabass", "cb"],
        0.70,
        0.45,
        0.30,
    ),
    (&["strings", "strings-ensemble"], 0.0, 0.25, 0.95),
    (&["violin-solo", "solo-violin"], -0.35, 0.02, 0.10),
    (&["viola-solo"], 0.05, 0.05, 0.10),
    (&["cello-solo", "solo-cello"], 0.25, 0.05, 0.10),
    (&["bass-solo"], 0.45, 0.10, 0.10),
    (
        &["woodwinds", "woodwinds-ensemble", "winds"],
        0.0,
        0.55,
        0.45,
    ),
    (&["piccolo"], -0.15, 0.55, 0.10),
    (&["flute", "flutes"], -0.12, 0.55, 0.15),
    (&["oboe", "oboes"], 0.10, 0.55, 0.15),
    (&["cor-anglais", "english-horn"], 0.18, 0.57, 0.10),
    (&["clarinet", "clarinets"], -0.12, 0.65, 0.15),
    (&["bass-clarinet"], -0.22, 0.66, 0.10),
    (&["bassoon", "bassoons"], 0.12, 0.65, 0.15),
    (&["contrabassoon"], 0.22, 0.66, 0.10),
    (&["saxophone", "sax"], 0.25, 0.55, 0.10),
    (
        &["horns", "horn", "french-horn", "french-horns"],
        -0.40,
        0.78,
        0.35,
    ),
    (&["brass", "brass-ensemble"], 0.30, 0.82, 0.50),
    (&["trumpet", "trumpets"], 0.15, 0.80, 0.25),
    (&["trombone", "trombones"], 0.38, 0.82, 0.25),
    (&["tuba"], 0.55, 0.82, 0.10),
    (&["timpani", "timp"], -0.15, 0.90, 0.30),
    (
        &[
            "percussion",
            "bass-drum",
            "snare-drum",
            "clash-cymbals",
            "suspended-cymbal",
            "tam-tam",
            "triangle",
            "drums",
            "tubular-bells",
            "chimes",
        ],
        0.05,
        0.95,
        0.50,
    ),
    (
        &[
            "glockenspiel",
            "xylophone",
            "vibraphone",
            "marimba",
            "mallets",
        ],
        0.25,
        0.88,
        0.25,
    ),
    (&["harp", "harps"], -0.68, 0.40, 0.20),
    (&["celesta", "piano", "keys"], -0.48, 0.42, 0.35),
    (
        &["choir", "mixed", "female", "male", "chorus"],
        0.0,
        1.0,
        0.90,
    ),
    (&["soloist", "solo"], 0.0, 0.0, 0.15),
];

impl Stage {
    /// Seat at `pan` (-1..1) and `depth` (0 front … 1 back). Width defaults to 0.6 narrowing
    /// towards the back.
    pub fn seat(pan: impl Into<Param>, depth: impl Into<Param>) -> Stage {
        Stage {
            pan: pan.into(),
            depth: depth.into(),
            width: Param::Const(0.5),
            er: Param::Const(0.35),
            distance_db: 7.0,
            air_db: 6.0,
            haas: 0.006,
            label: "stage".into(),
            sm_depth: Smooth::default(),
            shelf: SvfStage::default(),
            g: Default::default(),
            pre: Default::default(),
            er_line: Ring::default(),
            er_lp: [0.0; 2],
            er_amt: Ramp::default(),
            sr: 48_000.0,
            prepared: false,
        }
    }
    /// Look up a seat by section/instrument name. Accepts library ids too
    /// (`"strings/violins-1/sustain"` → violins-1). Panics with the list of names if unknown;
    /// see [`Stage::try_at`].
    pub fn at(name: &str) -> Stage {
        Stage::try_at(name).unwrap_or_else(|| {
            panic!(
                "uwu-wave: Stage::at({name:?}): unknown seat. Known: {}",
                Stage::names().join(", ")
            )
        })
    }
    /// Fallible [`Stage::at`].
    pub fn try_at(name: &str) -> Option<Stage> {
        let n = name.trim().to_lowercase().replace(['_', ' '], "-");
        let parts: Vec<&str> = n.split('/').collect();
        // "family/instrument/articulation" → instrument; "family/instrument" → instrument
        let cands: Vec<&str> = match parts.len() {
            1 => vec![parts[0]],
            2 => vec![parts[1], parts[0]],
            _ => vec![parts[1], parts[0]],
        };
        for c in cands {
            for (names, pan, depth, width) in SEATS {
                if names.contains(&c) {
                    return Some(Stage::seat(*pan, *depth).width(*width).named(names[0]));
                }
            }
        }
        None
    }
    /// All seat names understood by [`Stage::at`] (primary names).
    pub fn names() -> Vec<&'static str> {
        SEATS.iter().map(|s| s.0[0]).collect()
    }

    pub fn violins_1() -> Stage {
        Stage::at("violins-1")
    }
    pub fn violins_2() -> Stage {
        Stage::at("violins-2")
    }
    pub fn violas() -> Stage {
        Stage::at("violas")
    }
    pub fn cellos() -> Stage {
        Stage::at("cellos")
    }
    pub fn basses() -> Stage {
        Stage::at("basses")
    }
    pub fn woodwinds() -> Stage {
        Stage::at("woodwinds")
    }
    /// Trumpets, trombones and tuba (right-back).
    pub fn brass() -> Stage {
        Stage::at("brass")
    }
    pub fn horns() -> Stage {
        Stage::at("horns")
    }
    pub fn percussion() -> Stage {
        Stage::at("percussion")
    }
    pub fn timpani() -> Stage {
        Stage::at("timpani")
    }
    pub fn harp() -> Stage {
        Stage::at("harp")
    }
    pub fn choir() -> Stage {
        Stage::at("choir")
    }
    /// Front and centre.
    pub fn soloist() -> Stage {
        Stage::at("soloist")
    }

    /// Pan -1..1 (automatable).
    pub fn pan(mut self, p: impl Into<Param>) -> Self {
        self.pan = p.into();
        self
    }
    /// Depth 0 (front) … 1 (back) (automatable — e.g. an offstage trumpet walking in).
    pub fn depth(mut self, d: impl Into<Param>) -> Self {
        self.depth = d.into();
        self
    }
    /// Stereo spread of the section, 0 (point) … 1 (full input width).
    pub fn width(mut self, w: impl Into<Param>) -> Self {
        self.width = w.into();
        self
    }
    /// Early-reflection amount (0..1, scaled further by depth; default 0.35).
    pub fn er(mut self, amount: impl Into<Param>) -> Self {
        self.er = amount.into();
        self
    }
    /// Attenuation at depth 1, in dB (default 7).
    pub fn distance_db(mut self, db: f32) -> Self {
        self.distance_db = db.max(0.0);
        self
    }
    /// High-shelf loss above ~4 kHz at depth 1, in dB (default 6).
    pub fn air_db(mut self, db: f32) -> Self {
        self.air_db = db.max(0.0);
        self
    }
    /// Arrival delay at depth 1 in seconds (default 6 ms, max 30 ms).
    pub fn haas(mut self, secs: f32) -> Self {
        self.haas = secs.clamp(0.0, HAAS_MAX);
        self
    }
    pub fn named(mut self, n: impl Into<String>) -> Self {
        self.label = n.into();
        self
    }
}

/// Power-normalised pan gains (sum of squares = 1).
#[inline]
fn pg(p: f32) -> (f32, f32) {
    let th = (p.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    (th.cos(), th.sin())
}

impl Effect for Stage {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        for p in &mut self.pre {
            *p = Ring::with_secs(HAAS_MAX + 0.002, sr);
        }
        self.er_line = Ring::with_secs(ER_MAX * 1.5, sr);
        self.sm_depth.time(0.05, sr);
        self.sm_depth.reset();
        self.prepared = true;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if !self.prepared {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        self.sm_depth.set(ctx.p(&self.depth).clamp(0.0, 1.0));
        let d = self.sm_depth.advance(n);
        let pan = ctx.p(&self.pan).clamp(-1.0, 1.0);
        let sigma = ctx.p(&self.width).clamp(0.0, 1.0) * (1.0 - 0.4 * d);
        // L channel sits at pan-σ, R channel at pan+σ (each power-normalised); a narrow image of
        // correlated material sums coherently, so compensate towards 1/√2 as σ → 0.
        let (a, b) = pg(pan - sigma);
        let (c, e) = pg(pan + sigma);
        let norm = 1.0 / (1.0 + (1.0 - sigma) * (std::f32::consts::SQRT_2 - 1.0));
        let dist = crate::units::db_to_gain(-self.distance_db * d);
        let k = norm * dist;
        for (r, v) in self.g.iter_mut().zip([a * k, b * k, c * k, e * k]) {
            r.start(v, n);
        }
        self.shelf
            .set(SvfMode::HighShelf(-self.air_db * d), 4_000.0, 0.6, sr);
        let haas = self.haas * d * sr;
        let er_scale = 0.6 + 0.8 * d;
        let er_amt = ctx.p(&self.er).clamp(0.0, 2.0) * d;
        self.er_amt.start(er_amt, n);
        let er_a = 1.0 - (-std::f32::consts::TAU * 5_000.0 / sr).exp();
        let do_er = er_amt > 0.0 || self.er_amt.value > 0.0;
        for f in buf.iter_mut() {
            let x = self.shelf.frame(*f);
            let (gll, glr, grl, grr) = (
                self.g[0].next(),
                self.g[1].next(),
                self.g[2].next(),
                self.g[3].next(),
            );
            let y = Frame::new(x.l * gll + x.r * grl, x.l * glr + x.r * grr);
            self.pre[0].push(y.l);
            self.pre[1].push(y.r);
            let mut out = Frame::new(self.pre[0].read(haas), self.pre[1].read(haas));
            let amt = self.er_amt.next();
            if do_er {
                self.er_line.push(out.mid());
                let mut er = [0f32; 2];
                for (ms, g, ch) in ER_TAPS {
                    er[ch] += g * self.er_line.read(ms * 0.001 * er_scale * sr);
                }
                for ch in 0..2 {
                    self.er_lp[ch] =
                        crate::dsp::undenormal(self.er_lp[ch] + er_a * (er[ch] - self.er_lp[ch]));
                }
                out += Frame::new(self.er_lp[0], self.er_lp[1]) * amt;
            }
            *f = out;
        }
    }
    fn reset(&mut self) {
        self.shelf.reset();
        for p in &mut self.pre {
            p.clear();
        }
        self.er_line.clear();
        self.er_lp = [0.0; 2];
    }
    fn tail(&self) -> f32 {
        ER_MAX * 1.5 + HAAS_MAX
    }
    fn name(&self) -> String {
        format!("stage({})", self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(mut s: Stage, input: Frame) -> (f32, f32) {
        let sr = 48_000.0;
        s.prepare(sr);
        let ctx = Ctx::offline(sr);
        let (mut el, mut er) = (0.0, 0.0);
        for blk in 0..100 {
            let mut buf = [Frame::ZERO; 64];
            for (i, f) in buf.iter_mut().enumerate() {
                let t = (blk * 64 + i) as f32;
                *f = input * (t * std::f32::consts::TAU * 440.0 / sr).sin();
            }
            s.process(&mut buf, &ctx);
            if blk > 20 {
                for f in &buf {
                    assert!(f.l.is_finite() && f.r.is_finite());
                    el += f.l * f.l;
                    er += f.r * f.r;
                }
            }
        }
        (el, er)
    }

    #[test]
    fn seating_pans_as_expected() {
        let mono = Frame::mono(0.5);
        let (l, r) = energy(Stage::violins_1(), mono);
        assert!(l > 3.0 * r, "violins left {l} {r}");
        let (l, r) = energy(Stage::cellos(), mono);
        assert!(r > 1.5 * l, "cellos right {l} {r}");
        let (l, r) = energy(Stage::basses(), mono);
        assert!(r > 3.0 * l, "basses right {l} {r}");
        let (l, r) = energy(Stage::horns(), mono);
        assert!(l > r, "horns left");
        // centre front ≈ unity power for mono material
        let (l, r) = energy(Stage::seat(0.0, 0.0).width(0.0).er(0.0), mono);
        let (il, _) = energy(Stage::seat(0.0, 0.0).width(1.0).er(0.0), mono);
        // mono material through a point source keeps its level
        assert!(((l + r) / (2.0 * il) - 1.0).abs() < 0.1, "{l} {r} {il}");
    }

    #[test]
    fn depth_attenuates_and_full_width_is_transparent() {
        let mono = Frame::mono(0.5);
        let (fl, fr) = energy(Stage::seat(0.0, 0.0).er(0.0), mono);
        let (bl, br) = energy(Stage::seat(0.0, 1.0).er(0.0), mono);
        let db = 10.0 * ((bl + br) / (fl + fr)).log10();
        assert!(db < -6.0 && db > -12.0, "{db}");
        // pan 0, width 1, depth 0 = identity on stereo input
        let mut s = Stage::seat(0.0, 0.0).width(1.0).er(0.0);
        s.prepare(48_000.0);
        let ctx = Ctx::offline(48_000.0);
        let mut b = [Frame::new(0.3, -0.2); 64];
        s.process(&mut b, &ctx);
        assert!(
            (b[40].l - 0.3).abs() < 1e-3 && (b[40].r + 0.2).abs() < 1e-3,
            "{:?}",
            b[40]
        );
    }

    #[test]
    fn lookup_by_name() {
        assert_eq!(Stage::at("strings/violins-1/sustain").label, "violins-1");
        assert_eq!(Stage::at("brass/horn").label, "horns");
        assert_eq!(Stage::at("Violins 2").label, "violins-2");
        assert!(Stage::try_at("kazoo").is_none());
        for n in Stage::names() {
            assert!(Stage::try_at(n).is_some());
        }
    }
}
