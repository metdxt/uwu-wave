//! Band-limited oscillators.

use super::{Rng, TAU};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wave {
    Sine,
    Saw,
    Square,
    /// Pulse with duty cycle 0..1.
    Pulse(f32),
    Triangle,
    /// White noise.
    Noise,
}

/// Phase-accumulator oscillator with PolyBLEP anti-aliasing for saw/square/pulse.
#[derive(Clone, Debug)]
pub struct Osc {
    pub wave: Wave,
    pub phase: f32,
    rng: Rng,
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

impl Osc {
    pub fn new(wave: Wave) -> Osc {
        Osc { wave, phase: 0.0, rng: Rng::new(0xC0FFEE) }
    }
    pub fn with_phase(mut self, p: f32) -> Osc {
        self.phase = p.rem_euclid(1.0);
        self
    }
    pub fn seed(mut self, s: u64) -> Osc {
        self.rng = Rng::new(s);
        self
    }
    /// Generate the next sample at `freq` Hz.
    #[inline]
    pub fn next(&mut self, freq: f32, sr: f32) -> f32 {
        let dt = (freq / sr).clamp(0.0, 0.5);
        let t = self.phase;
        let y = match self.wave {
            Wave::Sine => (t * TAU).sin(),
            Wave::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Wave::Square => {
                let mut y = if t < 0.5 { 1.0 } else { -1.0 };
                y += poly_blep(t, dt);
                y -= poly_blep((t + 0.5) % 1.0, dt);
                y
            }
            Wave::Pulse(w) => {
                let w = w.clamp(0.01, 0.99);
                let mut y = if t < w { 1.0 } else { -1.0 };
                y += poly_blep(t, dt);
                y -= poly_blep((t + 1.0 - w) % 1.0, dt);
                y
            }
            Wave::Triangle => 1.0 - 4.0 * (t - 0.5).abs(),
            Wave::Noise => self.rng.bipolar(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}
