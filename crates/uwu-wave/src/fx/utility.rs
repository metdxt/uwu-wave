//! Utilities: [`Utility`] (gain/pan/mono/invert/swap) and [`Width`] (mid/side stereo width).

use super::Effect;
use super::filter::{SvfMode, SvfStage};
use crate::ctx::Ctx;
use crate::dsp::{Frame, Ramp};
use crate::param::Param;

/// Stereo-preserving pan (-1..1): the far channel is folded into the near one instead of being
/// discarded, keeping mono material at roughly constant power.
#[inline]
pub fn stereo_pan(f: Frame, p: f32) -> Frame {
    const K: f32 = std::f32::consts::SQRT_2 - 1.0;
    let p = p.clamp(-1.0, 1.0);
    if p < 0.0 {
        let q = -p;
        Frame::new(f.l + f.r * q * K, f.r * (1.0 - q))
    } else {
        Frame::new(f.l * (1.0 - p), f.r + f.l * p * K)
    }
}

/// Channel utility: gain, pan, mono, polarity, swap.
///
/// ```ignore
/// Utility::new().gain(-3.db()).pan(-0.3)
/// Utility::new().mono()
/// Utility::new().invert_r().swap()
/// ```
#[derive(Clone, Debug)]
pub struct Utility {
    gain: Param,
    pan: Param,
    mono: bool,
    inv: (bool, bool),
    swap: bool,
    g_r: Ramp,
    p_r: Ramp,
}

impl Utility {
    pub fn new() -> Utility {
        Utility {
            gain: Param::Const(1.0),
            pan: Param::Const(0.0),
            mono: false,
            inv: (false, false),
            swap: false,
            g_r: Ramp::default(),
            p_r: Ramp::default(),
        }
    }
    /// Linear gain (`-3.db()`), automatable.
    pub fn gain(mut self, g: impl Into<Param>) -> Self {
        self.gain = g.into();
        self
    }
    /// Pan -1..1 (stereo-preserving), automatable.
    pub fn pan(mut self, p: impl Into<Param>) -> Self {
        self.pan = p.into();
        self
    }
    /// Sum to mono.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    /// Flip the polarity of both channels.
    pub fn invert(mut self) -> Self {
        self.inv = (true, true);
        self
    }
    pub fn invert_l(mut self) -> Self {
        self.inv.0 = true;
        self
    }
    pub fn invert_r(mut self) -> Self {
        self.inv.1 = true;
        self
    }
    /// Swap left and right.
    pub fn swap(mut self) -> Self {
        self.swap = true;
        self
    }
}

impl Effect for Utility {
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        let n = buf.len();
        self.g_r.start(ctx.p(&self.gain), n);
        self.p_r.start(ctx.p(&self.pan), n);
        let (il, ir) = (
            if self.inv.0 { -1.0 } else { 1.0 },
            if self.inv.1 { -1.0 } else { 1.0 },
        );
        for f in buf.iter_mut() {
            let mut x = if self.swap {
                Frame::new(f.r * il, f.l * ir)
            } else {
                Frame::new(f.l * il, f.r * ir)
            };
            if self.mono {
                x = Frame::mono(x.mid());
            }
            let p = self.p_r.next();
            if p != 0.0 {
                x = stereo_pan(x, p);
            }
            *f = x * self.g_r.next();
        }
    }
    fn name(&self) -> String {
        "utility".into()
    }
}

/// Mid/side stereo width, with optional mono-below-a-frequency for tight low end.
///
/// ```ignore
/// Width::new(1.4)                    // wider
/// Width::new(0.0)                    // mono
/// Width::new(1.2).bass_mono(150.0)   // wide top, mono bass
/// ```
#[derive(Clone, Debug)]
pub struct Width {
    width: Param,
    bass_hz: Option<f32>,
    hp: SvfStage,
    w_r: Ramp,
}

impl Width {
    /// 0 = mono, 1 = unchanged, 2 = double side level.
    pub fn new(width: impl Into<Param>) -> Width {
        Width {
            width: width.into(),
            bass_hz: None,
            hp: SvfStage::default(),
            w_r: Ramp::default(),
        }
    }
    /// Remove stereo information below `hz`.
    pub fn bass_mono(mut self, hz: f32) -> Self {
        self.bass_hz = Some(hz);
        self
    }
}

impl Effect for Width {
    fn prepare(&mut self, sr: f32) {
        if let Some(hz) = self.bass_hz {
            self.hp.set(SvfMode::HighPass, hz, 0.707, sr);
        }
    }
    fn process(&mut self, buf: &mut [Frame], ctx: &Ctx) {
        self.w_r
            .start(ctx.p(&self.width).clamp(0.0, 4.0), buf.len());
        let bass = self.bass_hz.is_some();
        for f in buf.iter_mut() {
            let m = f.mid();
            let mut s = f.side() * self.w_r.next();
            if bass {
                s = self.hp.tick(0, s);
            }
            *f = Frame::new(m + s, m - s);
        }
    }
    fn reset(&mut self) {
        self.hp.reset();
    }
    fn name(&self) -> String {
        "width".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utility_and_width() {
        let ctx = Ctx::offline(48_000.0);
        let mut buf = [Frame::new(1.0, 0.5); 64];
        Utility::new().swap().invert_l().process(&mut buf, &ctx);
        assert_eq!(buf[10], Frame::new(-0.5, 1.0));
        let mut buf = [Frame::new(1.0, 0.0); 64];
        Width::new(0.0).process(&mut buf, &ctx);
        assert_eq!(buf[10], Frame::new(0.5, 0.5));
        let mut buf = [Frame::new(1.0, 1.0); 64];
        Utility::new().pan(-1.0).process(&mut buf, &ctx);
        assert!((buf[10].l - std::f32::consts::SQRT_2).abs() < 1e-5 && buf[10].r.abs() < 1e-6);
    }
}
