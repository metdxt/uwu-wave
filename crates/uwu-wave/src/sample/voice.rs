//! Sample playback head shared by [`Sampler`](super::Sampler) and [`SampleKit`](super::SampleKit):
//! fractional-rate Hermite reading, sample-rate conversion, octave mip-mapping for large upward
//! transpositions, start/end points, reverse, crossfaded loops and de-clicking.

use super::load::Sample;
use crate::dsp::Frame;
use std::sync::Arc;

/// Which part of a sample plays and how (all times in seconds of the original recording).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Start offset (after trimming).
    pub start: f32,
    /// Optional end point.
    pub end: Option<f32>,
    /// Loop points (start, end) — the region between them repeats while the voice sounds.
    pub looped: Option<(f32, f32)>,
    /// Loop crossfade length.
    pub xfade: f32,
    /// Play backwards (from the end point towards the start offset).
    pub reverse: bool,
    /// Skip leading silence (see [`Sample::onset`]).
    pub trim: bool,
}

impl Default for Region {
    fn default() -> Region {
        Region {
            start: 0.0,
            end: None,
            looped: None,
            xfade: 0.01,
            reverse: false,
            trim: false,
        }
    }
}

/// A playback head.
#[derive(Clone, Default)]
pub(crate) struct Play {
    smp: Option<Arc<Sample>>,
    pos: f64,
    step: f64,
    /// Playable region in frames of `smp`.
    lo: f64,
    hi: f64,
    lp: Option<(f64, f64)>,
    xf: f64,
    reverse: bool,
    declick: f64,
    fade_in: f32,
    fade_in_step: f32,
}

impl Play {
    /// Start playing `smp`, advancing `ratio` source frames (at the sample's own rate) per output
    /// frame at `out_sr`... i.e. `ratio` is the pitch ratio (1 = original pitch).
    pub(crate) fn start(smp: &Arc<Sample>, reg: &Region, ratio: f64, out_sr: f32) -> Play {
        let mut s = smp;
        let mut step = ratio * smp.sr as f64 / out_sr as f64;
        // octave mip-map: avoid aliasing when reading much faster than real time
        let mut level = 0;
        while step > 1.6 && level < 3 && s.len() > 64 {
            s = s.half();
            step *= 0.5;
            level += 1;
        }
        let sr = s.sr as f64;
        let len = s.len() as f64;
        let trim = if reg.trim {
            smp.onset() as f64 / smp.sr as f64
        } else {
            0.0
        };
        let lo = ((trim + reg.start.max(0.0) as f64) * sr).clamp(0.0, len);
        let hi = reg
            .end
            .map(|e| ((trim + e as f64) * sr).clamp(lo, len))
            .unwrap_or(len);
        let lp = reg.looped.and_then(|(a, b)| {
            let a = ((trim + a.max(0.0) as f64) * sr).clamp(lo, hi);
            let b = ((trim + b as f64) * sr).clamp(lo, hi);
            (b - a >= 8.0).then_some((a, b))
        });
        let xf = match lp {
            Some((a, b)) => (reg.xfade.max(0.0) as f64 * sr)
                .min(a - lo)
                .min((b - a) * 0.5)
                .max(0.0),
            None => 0.0,
        };
        let fade_in_frames = if (reg.reverse && hi < len - 1.0) || (!reg.reverse && lo > 0.0) {
            (0.001 * out_sr).max(1.0)
        } else {
            0.0
        };
        Play {
            smp: Some(s.clone()),
            pos: if reg.reverse { hi - 1.0 } else { lo },
            step,
            lo,
            hi,
            lp,
            xf,
            reverse: reg.reverse,
            declick: (0.002 * sr).max(1.0),
            fade_in: if fade_in_frames > 0.0 { 0.0 } else { 1.0 },
            fade_in_step: if fade_in_frames > 0.0 {
                1.0 / fade_in_frames
            } else {
                0.0
            },
        }
    }

    pub(crate) fn is_playing(&self) -> bool {
        self.smp.is_some()
    }

    pub(crate) fn stop(&mut self) {
        self.smp = None;
    }

    /// Multiply the playback rate (for pitch modulation); relative to the rate set at start.
    #[inline]
    pub(crate) fn set_step(&mut self, step: f64) {
        self.step = step;
    }
    #[inline]
    pub(crate) fn step(&self) -> f64 {
        self.step
    }

    /// Next output frame, or `None` once the end has been reached.
    #[inline]
    pub(crate) fn next(&mut self) -> Option<Frame> {
        let s = self.smp.as_ref()?;
        let mut out;
        if !self.reverse {
            if let Some((a, b)) = self.lp {
                if self.pos >= b {
                    self.pos -= b - a;
                }
                out = s.hermite(self.pos);
                if self.xf > 0.0 && self.pos > b - self.xf {
                    let t = ((self.pos - (b - self.xf)) / self.xf) as f32;
                    let (ga, gb) = (
                        (t * std::f32::consts::FRAC_PI_2).cos(),
                        (t * std::f32::consts::FRAC_PI_2).sin(),
                    );
                    out = out * ga + s.hermite(self.pos - (b - a)) * gb;
                }
            } else {
                if self.pos >= self.hi {
                    self.smp = None;
                    return None;
                }
                out = s.hermite(self.pos);
                let left = self.hi - self.pos;
                if left < self.declick {
                    out *= (left / self.declick) as f32;
                }
            }
            self.pos += self.step;
        } else {
            if let Some((a, b)) = self.lp {
                if self.pos < a {
                    self.pos += b - a;
                }
                out = s.hermite(self.pos);
            } else {
                if self.pos < self.lo {
                    self.smp = None;
                    return None;
                }
                out = s.hermite(self.pos);
                let left = self.pos - self.lo;
                if left < self.declick {
                    out *= (left / self.declick) as f32;
                }
            }
            self.pos -= self.step;
        }
        if self.fade_in < 1.0 {
            self.fade_in = (self.fade_in + self.fade_in_step).min(1.0);
            out *= self.fade_in;
        }
        Some(out)
    }
}

/// A head that is fading out (voice steal / retrigger / choke) to avoid clicks.
#[derive(Clone, Default)]
pub(crate) struct Fader {
    pub play: Play,
    gain: f32,
    dec: f32,
}

impl Fader {
    /// Take over `play` at the current `gain` and fade it out over `secs`.
    pub(crate) fn begin(&mut self, play: &mut Play, gain: f32, secs: f32, sr: f32) {
        if !play.is_playing() || gain <= 1e-5 {
            play.stop();
            return;
        }
        self.play = std::mem::take(play);
        self.gain = gain;
        self.dec = gain / (secs * sr).max(1.0);
    }
    /// Add the fading audio into `out`, already panned by `(gl, gr)`.
    #[inline]
    pub(crate) fn render(&mut self, out: &mut [Frame], gl: f32, gr: f32) {
        if !self.play.is_playing() {
            return;
        }
        for f in out.iter_mut() {
            match self.play.next() {
                Some(x) if self.gain > 0.0 => {
                    f.l += x.l * self.gain * gl;
                    f.r += x.r * self.gain * gr;
                    self.gain -= self.dec;
                }
                _ => {
                    self.play.stop();
                    return;
                }
            }
        }
    }
    pub(crate) fn active(&self) -> bool {
        self.play.is_playing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, sr: f32, secs: f32) -> Arc<Sample> {
        let n = (sr * secs) as usize;
        Arc::new(Sample::from_mono(
            &(0..n)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5)
                .collect::<Vec<_>>(),
            sr,
        ))
    }

    fn crossings(v: &[f32]) -> usize {
        v.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    #[test]
    fn resamples_and_repitches() {
        let s = sine(441.0, 44100.0, 1.0);
        let mut p = Play::start(&s, &Region::default(), 2.0, 48000.0);
        let mut v = vec![];
        while let Some(f) = p.next() {
            v.push(f.l);
        }
        // one octave up: half the duration, ~882 Hz
        let secs = v.len() as f32 / 48000.0;
        assert!((secs - 0.5).abs() < 0.01, "{secs}");
        let hz = crossings(&v) as f32 / secs;
        assert!((hz - 882.0).abs() < 5.0, "{hz}");
        assert!(v.iter().all(|x| x.is_finite() && x.abs() <= 0.6));
    }

    #[test]
    fn far_up_uses_mipmap_and_stays_clean() {
        let s = sine(1000.0, 48000.0, 1.0);
        let mut p = Play::start(&s, &Region::default(), 4.0, 48000.0);
        let v: Vec<f32> = std::iter::from_fn(|| p.next()).map(|f| f.l).collect();
        let hz = crossings(&v) as f32 / (v.len() as f32 / 48000.0);
        assert!((hz - 4000.0).abs() < 30.0, "{hz}");
    }

    #[test]
    fn loops_forever_and_reverse_ends() {
        let s = sine(100.0, 48000.0, 0.1);
        let reg = Region {
            looped: Some((0.02, 0.08)),
            ..Region::default()
        };
        let mut p = Play::start(&s, &reg, 1.0, 48000.0);
        for _ in 0..48000 {
            assert!(p.next().is_some());
        }
        let reg = Region {
            reverse: true,
            ..Region::default()
        };
        let mut p = Play::start(&s, &reg, 1.0, 48000.0);
        let n = std::iter::from_fn(|| p.next()).count();
        assert!((n as i32 - 4800).abs() < 3, "{n}");
    }
}
