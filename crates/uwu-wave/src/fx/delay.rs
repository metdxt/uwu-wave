//! Echo: [`Delay`] with tempo-synced time, filtered/saturated (tape-ish) feedback and ping-pong.

use super::Effect;
use super::filter::{SvfMode, SvfStage};
use super::util::{DelayTime, Ring, Smooth, send_mix};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp, soft_clip};
use crate::param::Param;
use std::f32::consts::TAU;

/// Stereo delay.
///
/// Time is a [`DelayTime`]: a musical length that follows the tempo (`dotted(eighth())`,
/// `3.beats()`) or seconds (`0.35`, `350.ms()`, an automation…). Time changes glide like a
/// tape machine. The repeats run through a band-limiting filter (`low_cut`/`high_cut`) and a soft
/// saturator (`drive`), so high feedback stays musical.
///
/// `mix` uses a send-style law (dry at full level up to 0.5); use [`Delay::wet`] on a return bus.
///
/// ```ignore
/// Delay::new(dotted(eighth())).feedback(0.45).mix(0.3)
/// Delay::new(quarter()).ping_pong().high_cut(3000.0)
/// Delay::dub(dotted(eighth())).wet()              // dark, saturated, wobbly repeats
/// Delay::new(0.09).feedback(0.1)                  // slapback in seconds
/// ```
#[derive(Clone, Debug)]
pub struct Delay {
    time: DelayTime,
    feedback: Param,
    mix: Param,
    ping_pong: bool,
    low_cut: Param,
    high_cut: Param,
    drive: Param,
    wobble: Param,
    width: Param,
    max_secs: f32,
    // runtime
    lines: [Ring; 2],
    sm_d: Smooth,
    hp: SvfStage,
    lp: SvfStage,
    wow: f32,
    flutter: f32,
    fb_r: Ramp,
    dry_r: Ramp,
    wet_r: Ramp,
    width_r: Ramp,
    sr: f32,
    prepared: bool,
    last_time: f32,
    last_fb: f32,
}

impl Delay {
    /// New delay: 40% feedback, 30% mix, gently band-limited repeats.
    pub fn new(time: impl Into<DelayTime>) -> Delay {
        Delay {
            time: time.into(),
            feedback: Param::Const(0.4),
            mix: Param::Const(0.3),
            ping_pong: false,
            low_cut: Param::Const(80.0),
            high_cut: Param::Const(9_000.0),
            drive: Param::Const(1.0),
            wobble: Param::Const(0.0),
            width: Param::Const(1.0),
            max_secs: 4.0,
            lines: Default::default(),
            sm_d: Smooth::default(),
            hp: SvfStage::default(),
            lp: SvfStage::default(),
            wow: 0.0,
            flutter: 0.0,
            fb_r: Ramp::default(),
            dry_r: Ramp::default(),
            wet_r: Ramp::default(),
            width_r: Ramp::default(),
            sr: 48_000.0,
            prepared: false,
            last_time: 0.5,
            last_fb: 0.4,
        }
    }
    /// Dark, saturated, wobbly dub/tape echo with long feedback.
    pub fn dub(time: impl Into<DelayTime>) -> Delay {
        Delay::new(time)
            .feedback(0.65)
            .high_cut(2_500.0)
            .low_cut(200.0)
            .drive(2.0)
            .wobble(0.25)
    }
    /// Tape echo character on any delay: darker repeats, saturation, wow & flutter.
    pub fn tape(self) -> Delay {
        self.high_cut(4_500.0).low_cut(150.0).drive(1.6).wobble(0.3)
    }
    /// Short single slap (90 ms) — rockabilly vocals, guitars.
    pub fn slapback() -> Delay {
        Delay::new(0.09).feedback(0.12).mix(0.35).high_cut(5_000.0)
    }
    /// Feedback amount 0..1 (repeats).
    pub fn feedback(mut self, fb: impl Into<Param>) -> Self {
        self.feedback = fb.into();
        self
    }
    /// Dry/wet (send-style law).
    pub fn mix(mut self, m: impl Into<Param>) -> Self {
        self.mix = m.into();
        self
    }
    /// 100% wet — for effect-return buses.
    pub fn wet(self) -> Self {
        self.mix(1.0)
    }
    /// Bounce the repeats left ↔ right.
    pub fn ping_pong(mut self) -> Self {
        self.ping_pong = true;
        self
    }
    /// High-pass in the feedback path (Hz).
    pub fn low_cut(mut self, hz: impl Into<Param>) -> Self {
        self.low_cut = hz.into();
        self
    }
    /// Low-pass in the feedback path (Hz).
    pub fn high_cut(mut self, hz: impl Into<Param>) -> Self {
        self.high_cut = hz.into();
        self
    }
    /// Saturation of the repeats (linear gain into a soft clipper; 1 = clean-ish).
    pub fn drive(mut self, d: impl Into<Param>) -> Self {
        self.drive = d.into();
        self
    }
    /// Tape wow & flutter amount 0..1.
    pub fn wobble(mut self, w: impl Into<Param>) -> Self {
        self.wobble = w.into();
        self
    }
    /// Stereo width of the repeats.
    pub fn width(mut self, w: impl Into<Param>) -> Self {
        self.width = w.into();
        self
    }
    /// Change the time.
    pub fn time(mut self, t: impl Into<DelayTime>) -> Self {
        self.time = t.into();
        self
    }
    /// Maximum delay time in seconds (buffer size; default 4 s, raised automatically for long constant times).
    pub fn max_time(mut self, secs: f32) -> Self {
        self.max_secs = secs.max(0.01);
        self
    }
}

impl Effect for Delay {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        // synced constants are given in beats: assume tempos down to 40 BPM
        let need = match &self.time {
            DelayTime::Beats(_) => self.time.const_hint().map(|b| b * 1.5),
            DelayTime::Secs(_) => self.time.const_hint(),
        };
        if let Some(n) = need {
            self.max_secs = self.max_secs.max(n + 0.1);
        }
        for l in &mut self.lines {
            *l = Ring::with_secs(self.max_secs + 0.05, sr);
        }
        self.sm_d.time(0.08, sr);
        self.sm_d.reset();
        self.prepared = true;
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        if !self.prepared {
            self.prepare(ctx.sr);
        }
        let sr = self.sr;
        let n = buf.len();
        let t = self.time.at(ctx).clamp(0.0005, self.max_secs);
        self.last_time = t;
        self.sm_d.set(t * sr);
        let fb = ctx.p(&self.feedback).clamp(0.0, 1.0);
        self.last_fb = fb;
        self.fb_r.start(fb, n);
        let (dg, wg) = send_mix(ctx.p(&self.mix));
        self.dry_r.start(dg, n);
        self.wet_r.start(wg, n);
        self.width_r.start(ctx.p(&self.width).clamp(0.0, 2.0), n);
        self.hp.set(
            SvfMode::HighPass,
            ctx.p(&self.low_cut).clamp(10.0, sr * 0.45),
            0.6,
            sr,
        );
        self.lp.set(
            SvfMode::LowPass,
            ctx.p(&self.high_cut).clamp(100.0, sr * 0.45),
            0.6,
            sr,
        );
        let drive = ctx.p(&self.drive).clamp(0.1, 20.0);
        let inv_drive = 1.0 / drive;
        let wob = ctx.p(&self.wobble).clamp(0.0, 1.0) * 0.0025 * sr;
        let (wow_inc, flut_inc) = (0.6 / sr, 5.7 / sr);
        let max_d = self.lines[0].max_delay();
        for f in buf.iter_mut() {
            let dry = *f;
            self.wow = (self.wow + wow_inc).fract();
            self.flutter = (self.flutter + flut_inc).fract();
            let m = if wob > 0.0 {
                wob * (1.0 + 0.75 * (TAU * self.wow).sin() + 0.25 * (TAU * self.flutter).sin())
            } else {
                0.0
            };
            let d = (self.sm_d.next() + m - 1.0).clamp(1.0, max_d);
            let y = Frame::new(self.lines[0].read_cubic(d), self.lines[1].read_cubic(d));
            let y = self.lp.frame(self.hp.frame(y));
            let y = Frame::new(
                soft_clip(y.l * drive) * inv_drive,
                soft_clip(y.r * drive) * inv_drive,
            );
            let g = self.fb_r.next();
            let (il, ir) = if self.ping_pong {
                (dry.mid() + y.r * g, y.l * g)
            } else {
                (dry.l + y.l * g, dry.r + y.r * g)
            };
            self.lines[0].push(crate::dsp::undenormal(il));
            self.lines[1].push(crate::dsp::undenormal(ir));
            let w = self.width_r.next();
            let (mm, ss) = (y.mid(), y.side() * w);
            *f = dry * self.dry_r.next() + Frame::new(mm + ss, mm - ss) * self.wet_r.next();
        }
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.hp.reset();
        self.lp.reset();
        self.sm_d.reset();
    }
    fn tail(&self) -> f32 {
        let fb = self
            .feedback
            .as_const()
            .unwrap_or(self.last_fb)
            .clamp(0.0, 0.999);
        let repeats = if fb < 1e-3 {
            1.0
        } else {
            (0.001f32.ln() / fb.ln()).min(200.0) + 1.0
        };
        (self.last_time * repeats).min(60.0)
    }
    fn name(&self) -> String {
        "delay".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{dotted, eighth};

    fn run(d: &mut Delay, n: usize) -> Vec<Frame> {
        let sr = 48_000.0;
        d.prepare(sr);
        let ctx = Ctx::offline(sr); // 120 BPM
        let mut out = vec![];
        for b in 0..n / 64 {
            let mut buf = [Frame::ZERO; 64];
            if b == 0 {
                buf[0] = Frame::new(1.0, 1.0);
            }
            d.process(&mut buf, &ctx);
            out.extend_from_slice(&buf);
        }
        out
    }

    #[test]
    fn synced_echo_lands_on_time() {
        // dotted eighth at 120 BPM = 0.375 s = 18000 samples
        let mut d = Delay::new(dotted(eighth()))
            .wet()
            .high_cut(20_000.0)
            .low_cut(10.0);
        let out = run(&mut d, 48_000);
        let (idx, _) = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.l.abs().total_cmp(&b.1.l.abs()))
            .unwrap();
        assert!((idx as i64 - 18_000).abs() <= 3, "{idx}");
        assert!(
            out[36_000 - 50..36_000 + 50]
                .iter()
                .any(|f| f.l.abs() > 0.1)
        );
        assert!((d.tail() - 0.375 * 8.5).abs() < 0.5, "{}", d.tail());
    }

    #[test]
    fn ping_pong_alternates() {
        let mut d = Delay::new(0.1).ping_pong().wet().feedback(0.8);
        let out = run(&mut d, 24_000);
        let e = |a: usize, ch: usize| {
            out[a - 200..a + 200]
                .iter()
                .map(|f| if ch == 0 { f.l.abs() } else { f.r.abs() })
                .fold(0f32, f32::max)
        };
        assert!(e(4800, 0) > 0.1 && e(4800, 1) < 1e-3);
        assert!(e(9600, 1) > 0.05 && e(9600, 0) < 1e-3);
    }

    #[test]
    fn full_feedback_stays_bounded() {
        let mut d = Delay::dub(0.05).feedback(1.0).wet();
        let out = run(&mut d, 96_000);
        assert!(out.iter().all(|f| f.l.is_finite() && f.l.abs() < 2.0));
    }
}
