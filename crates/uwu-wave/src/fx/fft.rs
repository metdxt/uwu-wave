//! Small radix-2 complex FFT used by the [`Convolver`](super::Convolver) (no dependencies).

use std::ops::{Add, AddAssign, Mul, Sub};

/// A complex number (f32).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct C {
    pub re: f32,
    pub im: f32,
}

impl C {
    pub const ZERO: C = C { re: 0.0, im: 0.0 };
    #[inline]
    pub const fn new(re: f32, im: f32) -> C {
        C { re, im }
    }
    #[inline]
    pub fn conj(self) -> C {
        C::new(self.re, -self.im)
    }
    /// Multiply by i.
    #[inline]
    pub fn mul_i(self) -> C {
        C::new(-self.im, self.re)
    }
    /// Multiply by -i.
    #[inline]
    pub fn mul_neg_i(self) -> C {
        C::new(self.im, -self.re)
    }
    #[inline]
    pub fn scale(self, k: f32) -> C {
        C::new(self.re * k, self.im * k)
    }
}

impl Add for C {
    type Output = C;
    #[inline]
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for C {
    type Output = C;
    #[inline]
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for C {
    type Output = C;
    #[inline]
    fn mul(self, o: C) -> C {
        C::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}
impl AddAssign for C {
    #[inline]
    fn add_assign(&mut self, o: C) {
        self.re += o.re;
        self.im += o.im;
    }
}

/// Precomputed in-place iterative radix-2 FFT of a fixed power-of-two size.
#[derive(Clone, Debug)]
pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    tw: Vec<C>,
}

impl Fft {
    /// Plan an FFT of size `n` (power of two, ≥ 2).
    pub fn new(n: usize) -> Fft {
        assert!(
            n >= 2 && n.is_power_of_two(),
            "FFT size must be a power of two, got {n}"
        );
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                C::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Fft { n, rev, tw }
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Forward transform (e^{-i…}), unscaled.
    pub fn forward(&self, buf: &mut [C]) {
        self.run(buf, false)
    }
    /// Inverse transform (e^{+i…}), **unscaled** (divide by `len()` yourself).
    pub fn inverse(&self, buf: &mut [C]) {
        self.run(buf, true)
    }
    fn run(&self, buf: &mut [C], inv: bool) {
        let n = self.n;
        assert_eq!(buf.len(), n);
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                buf.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let w = self.tw[k * step];
                    let w = if inv { w.conj() } else { w };
                    let a = buf[start + k];
                    let b = buf[start + k + half] * w;
                    buf[start + k] = a + b;
                    buf[start + k + half] = a - b;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dft_and_round_trips() {
        let n = 64;
        let x: Vec<C> = (0..n)
            .map(|i| C::new((i as f32 * 0.7).sin(), (i as f32 * 0.31).cos()))
            .collect();
        let mut y = x.clone();
        let f = Fft::new(n);
        f.forward(&mut y);
        for k in [0, 1, 5, 31, 63] {
            let mut s = C::ZERO;
            for (i, xi) in x.iter().enumerate() {
                let a = -2.0 * std::f32::consts::PI * (k * i) as f32 / n as f32;
                s += *xi * C::new(a.cos(), a.sin());
            }
            assert!((s.re - y[k].re).abs() < 1e-3 && (s.im - y[k].im).abs() < 1e-3);
        }
        f.inverse(&mut y);
        for (a, b) in x.iter().zip(&y) {
            assert!((a.re - b.re / n as f32).abs() < 1e-5 && (a.im - b.im / n as f32).abs() < 1e-5);
        }
    }
}
