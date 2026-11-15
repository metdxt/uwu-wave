//! Parameter smoothing.

/// One-pole smoothed value (call `next()` per sample).
#[derive(Clone, Copy, Debug, Default)]
pub struct Smoothed {
    pub value: f32,
    pub target: f32,
    coef: f32,
    init: bool,
}

impl Smoothed {
    /// `time` in seconds (≈63% convergence).
    pub fn new(time: f32, sr: f32) -> Smoothed {
        Smoothed { value: 0.0, target: 0.0, coef: super::time_coef(time, sr), init: false }
    }
    /// Set target; the very first call jumps directly.
    #[inline]
    pub fn set(&mut self, target: f32) {
        self.target = target;
        if !self.init {
            self.value = target;
            self.init = true;
        }
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        self.value = self.target + (self.value - self.target) * self.coef;
        self.value
    }
}

/// Linear ramp across a block: `start(target, n)` then `next()` n times.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ramp {
    pub value: f32,
    step: f32,
    left: usize,
    init: bool,
}

impl Ramp {
    #[inline]
    pub fn start(&mut self, target: f32, n: usize) {
        if !self.init || n == 0 {
            self.value = target;
            self.step = 0.0;
            self.left = 0;
            self.init = true;
            return;
        }
        self.step = (target - self.value) / n as f32;
        self.left = n;
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.left > 0 {
            self.value += self.step;
            self.left -= 1;
        }
        self.value
    }
}
