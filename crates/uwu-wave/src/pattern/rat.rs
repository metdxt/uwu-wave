//! Tiny positive rationals for exact pattern periods (cycle counts).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rat {
    pub n: u64,
    pub d: u64,
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

const CAP: u128 = 1 << 40;

impl Rat {
    pub const ONE: Rat = Rat { n: 1, d: 1 };

    pub fn new(n: u64, d: u64) -> Rat {
        Rat::from_u128(n as u128, d.max(1) as u128)
    }
    fn from_u128(n: u128, d: u128) -> Rat {
        let (mut n, mut d) = (n.max(1), d.max(1));
        let g = {
            let (mut a, mut b) = (n, d);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        n /= g;
        d /= g;
        if n > CAP || d > CAP {
            // degenerate: fall back to an approximation
            return Rat::from_f64(n as f64 / d as f64);
        }
        Rat {
            n: n as u64,
            d: d as u64,
        }
    }
    pub fn int(n: u64) -> Rat {
        Rat::new(n.max(1), 1)
    }
    /// Best rational approximation with a denominator ≤ 10000 (non-positive → 1).
    pub fn from_f64(x: f64) -> Rat {
        if !(x.is_finite() && x > 0.0) {
            return Rat::ONE;
        }
        let (mut h0, mut h1, mut k0, mut k1) = (0u64, 1u64, 1u64, 0u64);
        let mut v = x;
        for _ in 0..32 {
            let a = v.floor();
            if a > 1e12 {
                break;
            }
            let a = a as u64;
            let h2 = a.saturating_mul(h1).saturating_add(h0);
            let k2 = a.saturating_mul(k1).saturating_add(k0);
            if k2 > 10_000 {
                break;
            }
            (h0, h1, k0, k1) = (h1, h2, k1, k2);
            let frac = v - a as f64;
            if frac < 1e-9 || ((h1 as f64 / k1 as f64) - x).abs() < 1e-9 * x {
                break;
            }
            v = 1.0 / frac;
        }
        if h1 == 0 || k1 == 0 {
            return Rat::ONE;
        }
        let g = gcd(h1, k1);
        Rat {
            n: h1 / g,
            d: k1 / g,
        }
    }
    pub fn f64(self) -> f64 {
        self.n as f64 / self.d as f64
    }
    pub fn mul(self, o: Rat) -> Rat {
        Rat::from_u128(self.n as u128 * o.n as u128, self.d as u128 * o.d as u128)
    }
    pub fn div(self, o: Rat) -> Rat {
        Rat::from_u128(self.n as u128 * o.d as u128, self.d as u128 * o.n as u128)
    }
    pub fn add(self, o: Rat) -> Rat {
        Rat::from_u128(
            self.n as u128 * o.d as u128 + o.n as u128 * self.d as u128,
            self.d as u128 * o.d as u128,
        )
    }
    /// Least common multiple: lcm(a/b, c/d) = lcm(a, c) / gcd(b, d).
    pub fn lcm(self, o: Rat) -> Rat {
        let g = gcd(self.n, o.n) as u128;
        let l = self.n as u128 / g * o.n as u128;
        Rat::from_u128(l, gcd(self.d, o.d) as u128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rat_ops() {
        assert_eq!(Rat::from_f64(1.5), Rat::new(3, 2));
        assert_eq!(Rat::from_f64(0.1f32 as f64), Rat::new(1, 10));
        assert_eq!(Rat::from_f64(1.0 / 3.0), Rat::new(1, 3));
        assert_eq!(Rat::new(1, 2).lcm(Rat::ONE), Rat::ONE);
        assert_eq!(Rat::new(3, 2).lcm(Rat::ONE), Rat::int(3));
        assert_eq!(Rat::int(2).lcm(Rat::int(3)), Rat::int(6));
        assert_eq!(Rat::new(1, 2).add(Rat::new(1, 3)), Rat::new(5, 6));
    }
}
