//! Delay lines.

/// A mono circular delay line with fractional (linear / cubic) reads.
#[derive(Clone, Debug, Default)]
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    /// Capacity in samples (the maximum delay is `len - 1`).
    pub fn new(len: usize) -> DelayLine {
        DelayLine { buf: vec![0.0; len.max(4)], pos: 0 }
    }
    /// Capacity for `secs` seconds at `sr`.
    pub fn with_secs(secs: f32, sr: f32) -> DelayLine {
        DelayLine::new((secs * sr).ceil() as usize + 4)
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    /// Write the next sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
        self.buf[self.pos] = x;
    }
    /// Read `d` samples back (integer). `tap(0)` is the last pushed sample.
    #[inline]
    pub fn tap(&self, d: usize) -> f32 {
        let n = self.buf.len();
        let d = d.min(n - 1);
        self.buf[(self.pos + n - d) % n]
    }
    /// Linear-interpolated fractional read.
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let d = d.clamp(0.0, (self.buf.len() - 2) as f32);
        let i = d.floor();
        let f = d - i;
        let a = self.tap(i as usize);
        let b = self.tap(i as usize + 1);
        a + (b - a) * f
    }
    /// Cubic (Hermite) fractional read – smoother for modulated delays (chorus/flanger).
    #[inline]
    pub fn read_cubic(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, (self.buf.len() - 3) as f32);
        let i = d.floor() as usize;
        let f = d - d.floor();
        let (xm1, x0, x1, x2) = (self.tap(i - 1), self.tap(i), self.tap(i + 1), self.tap(i + 2));
        let c0 = x0;
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * f + c2) * f + c1) * f + c0
    }
    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
    }
}
