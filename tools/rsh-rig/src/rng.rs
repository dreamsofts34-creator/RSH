//! Seeded RNG for the rig: xorshift64* plus a Box-Muller Gaussian.
//!
//! Deterministic by construction: the same seed always yields the same
//! stream, so a rig failure can be reproduced with `--seed N`. No
//! external RNG crate is pulled in for this on purpose — the rig must
//! stay dependency-light and its randomness must be exactly specified
//! here, not behind a crate version.

/// xorshift64* generator (Vigna). State must never be 0.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    /// Cached second Gaussian from the last Box-Muller pair.
    spare: Option<f64>,
}

impl Rng {
    /// Create a generator from any seed (0 is remapped to a fixed
    /// nonzero state so the generator can never wedge).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
            spare: None,
        }
    }

    /// Next raw `u64`.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform `f64` in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // Top 53 bits → [0, 1).
        let v = self.next_u64() >> 11;
        (v as f64) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform `i64` in `[lo, hi]`.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i64
    }

    /// True with probability `p` (`0.0..=1.0`).
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Standard-normal sample (mean 0, σ 1), Box-Muller with a cached
    /// spare so pairs are not wasted.
    pub fn gaussian(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let mut u1 = self.next_f64();
        // log(0) is -inf; clamp the (measure-zero) exact 0 away.
        if u1 < f64::MIN_POSITIVE {
            u1 = f64::MIN_POSITIVE;
        }
        let u2 = self.next_f64();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }

    /// Gaussian sample with the given mean and standard deviation.
    pub fn gaussian_ms(&mut self, mean: f64, sigma: f64) -> f64 {
        mean + self.gaussian() * sigma
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn gaussian_is_roughly_standard_normal() {
        let mut rng = Rng::new(7);
        let n = 20_000;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..n {
            let v = rng.gaussian();
            sum += v;
            sum_sq += v * v;
        }
        let mean = sum / f64::from(n);
        let var = sum_sq / f64::from(n) - mean * mean;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 1.0).abs() < 0.1, "var {var}");
    }
}
