//! Reproducible random streams for algorithm profile `rust-v1`.
//!
//! A request supplies one 64-bit master seed as 16 hexadecimal digits. Each
//! stochastic stage draws from its own ChaCha8 stream whose 256-bit key is
//! `SHA-256("artpop-rust/rng/v1" | seed | stage | keys...)`. Adding a band,
//! a component, or another stage therefore never perturbs an existing stream.
//!
//! Uniform, normal, and Poisson variates are implemented here rather than
//! taken from a distribution crate so that their algorithms are frozen with
//! the profile version:
//! - uniform: top 53 bits of a `u64`, scaled to `[0, 1)`;
//! - normal: Marsaglia polar method, second variate cached;
//! - Poisson: multiplication method for `lambda < 10`, Hörmann's PTRS
//!   transformed rejection for `lambda >= 10` (no platform substitutions).
//!
//! Bulk draws are split into fixed blocks of [`BLOCK`] elements with one
//! stream per block ([`par_blocks`]), so they parallelize deterministically.

use crate::error::{Error, Result};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use sha2::{Digest, Sha256};

pub const RNG_ALGORITHM: &str = "chacha8-sha256-streams-v1";
pub const UNIFORM_ALGORITHM: &str = "u64-top53-v1";
pub const NORMAL_ALGORITHM: &str = "marsaglia-polar-v1";
pub const POISSON_ALGORITHM: &str = "mult-lt10-ptrs-ge10-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seed(pub u64);

impl Seed {
    /// Parse the contract's fixed-width representation: exactly 16 hex digits.
    pub fn parse(text: &str, path: &str) -> Result<Seed> {
        if text.len() != 16 || !text.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::invalid(path, "Seed must be exactly 16 hexadecimal digits, e.g. \"000000000000002a\".")
                .with_details(serde_json::json!({ "received": text })));
        }
        u64::from_str_radix(text, 16)
            .map(Seed)
            .map_err(|_| Error::invalid(path, "Seed is not valid hexadecimal."))
    }

    pub fn to_hex(self) -> String {
        format!("{:016x}", self.0)
    }

    /// Derive an independent stream for a stage and a list of stable keys.
    pub fn stream(self, stage: &str, keys: &[&str]) -> Stream {
        let mut h = Sha256::new();
        h.update(b"artpop-rust/rng/v1\0");
        h.update(self.0.to_le_bytes());
        h.update(stage.as_bytes());
        for k in keys {
            h.update(b"\0");
            h.update(k.as_bytes());
        }
        let digest = h.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Stream { rng: ChaCha8Rng::from_seed(key), cached_normal: None }
    }
}

pub struct Stream {
    rng: ChaCha8Rng,
    cached_normal: Option<f64>,
}

impl Stream {
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.rng.next_u64()
    }

    /// Uniform double in `[0, 1)`.
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9007199254740992.0)
    }

    /// Uniform double in `[lo, hi)`.
    #[inline]
    pub fn uniform_range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    /// Standard normal variate (Marsaglia polar method).
    pub fn normal(&mut self) -> f64 {
        if let Some(v) = self.cached_normal.take() {
            return v;
        }
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let f = (-2.0 * s.ln() / s).sqrt();
                self.cached_normal = Some(v * f);
                return u * f;
            }
        }
    }

    /// Poisson variate with mean `lambda` (finite, nonnegative, below 9e18).
    pub fn poisson(&mut self, lambda: f64) -> u64 {
        if lambda <= 0.0 {
            return 0;
        }
        if lambda < 10.0 {
            let enlam = (-lambda).exp();
            let mut x = 0u64;
            let mut prod = 1.0;
            loop {
                prod *= self.uniform();
                if prod > enlam {
                    x += 1;
                } else {
                    return x;
                }
            }
        }
        // Hörmann (1993), "The transformed rejection method for generating
        // Poisson random variables", algorithm PTRS.
        let slam = lambda.sqrt();
        let loglam = lambda.ln();
        let b = 0.931 + 2.53 * slam;
        let a = -0.059 + 0.02483 * b;
        let invalpha = 1.1239 + 1.1328 / (b - 3.4);
        let vr = 0.9277 - 3.6224 / (b - 2.0);
        loop {
            let u = self.uniform() - 0.5;
            let v = self.uniform();
            let us = 0.5 - u.abs();
            let kf = floor_exact((2.0 * a / us + b) * u + lambda + 0.43);
            if us >= 0.07 && v <= vr {
                return kf as u64;
            }
            if kf < 0.0 || (us < 0.013 && v > us) {
                continue;
            }
            if v.ln() + invalpha.ln() - (a / (us * us) + b).ln() <= -lambda + kf * loglam - libm::lgamma(kf + 1.0) {
                return kf as u64;
            }
        }
    }
}

/// `f64::floor` for |x| < 2^63 without a library call (identical results,
/// including -inf, which PTRS rejects as `kf < 0`).
#[inline]
fn floor_exact(x: f64) -> f64 {
    let t = (x as i64) as f64;
    if t > x { t - 1.0 } else { t }
}

/// Maximum Poisson mean accepted by the sampler.
pub const MAX_POISSON_MEAN: f64 = 9.0e18;

/// Elements per logical random block. Part of the `rust-v1` algorithm: each
/// block draws from its own stream keyed by its index, so results do not
/// depend on how many worker threads process the blocks.
pub const BLOCK: usize = 1 << 16;
pub const BLOCK_ALGORITHM: &str = "blocks-65536-v1";

/// Fill `out` in parallel; block `k` uses the stream `(stage, keys..., "block<k>")`.
pub fn par_blocks<T: Send>(seed: Seed, stage: &str, keys: &[&str], out: &mut [T], f: impl Fn(&mut Stream, &mut [T]) + Sync) {
    use rayon::prelude::*;
    out.par_chunks_mut(BLOCK).enumerate().for_each(|(k, chunk)| {
        let block = format!("block{k}");
        let mut all: Vec<&str> = keys.to_vec();
        all.push(&block);
        let mut st = seed.stream(stage, &all);
        f(&mut st, chunk);
    });
}

/// Two equally long outputs filled together; block `k` of both uses the
/// stream `(stage, keys..., "block<k>")`, so a `(x, y)` pair drawn in `f`
/// consumes the stream exactly as a single tuple array would.
pub fn par_blocks2<A: Send, B: Send>(seed: Seed, stage: &str, keys: &[&str], a: &mut [A], b: &mut [B], f: impl Fn(&mut Stream, &mut [A], &mut [B]) + Sync) {
    use rayon::prelude::*;
    assert_eq!(a.len(), b.len(), "paired block fills need equal lengths");
    a.par_chunks_mut(BLOCK).zip(b.par_chunks_mut(BLOCK)).enumerate().for_each(|(k, (ca, cb))| {
        let block = format!("block{k}");
        let mut all: Vec<&str> = keys.to_vec();
        all.push(&block);
        let mut st = seed.stream(stage, &all);
        f(&mut st, ca, cb);
    });
}

/// Fill `out` in parallel blocks while reading the matching block of `src`.
pub fn par_blocks_zip<T: Send, S: Sync>(seed: Seed, stage: &str, keys: &[&str], out: &mut [T], src: &[S], f: impl Fn(&mut Stream, &mut [T], &[S]) + Sync) {
    use rayon::prelude::*;
    assert_eq!(out.len(), src.len(), "zipped block fills need equal lengths");
    out.par_chunks_mut(BLOCK).zip(src.par_chunks(BLOCK)).enumerate().for_each(|(k, (chunk, s))| {
        let block = format!("block{k}");
        let mut all: Vec<&str> = keys.to_vec();
        all.push(&block);
        let mut st = seed.stream(stage, &all);
        f(&mut st, chunk, s);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_and_zipped_block_fills_match_single_arrays() {
        let n = 3 * BLOCK + 17;
        let mut pairs = vec![(0.0, 0.0); n];
        par_blocks(Seed(9), "pair", &["k"], &mut pairs, |st, chunk| {
            for p in chunk.iter_mut() {
                *p = (st.uniform(), st.normal());
            }
        });
        let (mut xs, mut ys) = (vec![0.0; n], vec![0.0; n]);
        par_blocks2(Seed(9), "pair", &["k"], &mut xs, &mut ys, |st, cx, cy| {
            for (x, y) in cx.iter_mut().zip(cy.iter_mut()) {
                *x = st.uniform();
                *y = st.normal();
            }
        });
        assert!(pairs.iter().zip(xs.iter().zip(&ys)).all(|(p, (x, y))| p.0 == *x && p.1 == *y));
        let src: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut a = src.clone();
        par_blocks(Seed(3), "zip", &[], &mut a, |st, chunk| {
            for v in chunk.iter_mut() {
                *v += st.uniform();
            }
        });
        let mut b = vec![0.0; n];
        par_blocks_zip(Seed(3), "zip", &[], &mut b, &src, |st, chunk, s| {
            for (v, x) in chunk.iter_mut().zip(s) {
                *v = x + st.uniform();
            }
        });
        assert_eq!(a, b);
    }

    #[test]
    fn seed_parsing_is_strict() {
        assert_eq!(Seed::parse("000000000000007b", "/s").unwrap().0, 123);
        assert!(Seed::parse("7b", "/s").is_err());
        assert!(Seed::parse("00000000000000zz", "/s").is_err());
    }

    #[test]
    fn streams_are_independent_and_reproducible() {
        let s = Seed(42);
        let a: Vec<u64> = (0..4).map({
            let mut st = s.stream("mass", &["c0"]);
            move |_| st.next_u64()
        }).collect();
        let mut again = s.stream("mass", &["c0"]);
        assert_eq!(a, (0..4).map(|_| again.next_u64()).collect::<Vec<_>>());
        let mut other = s.stream("mass", &["c1"]);
        assert_ne!(a[0], other.next_u64());
    }

    #[test]
    fn exact_floor_matches_std() {
        for x in [-2.5, -2.0, -1.0, -0.3, 0.0, 0.7, 1.0, 1.9999, 12345.5, 9.0e18, f64::NEG_INFINITY] {
            let want = x.floor();
            let got = floor_exact(x);
            assert!(got == want || (x == f64::NEG_INFINITY && got < 0.0), "{x}: {got} vs {want}");
        }
    }

    #[test]
    fn poisson_and_normal_moments() {
        let mut st = Seed(7).stream("test", &[]);
        for &lam in &[0.5, 3.0, 12.0, 250.0, 1.0e6] {
            let n = 200_000;
            let xs: Vec<f64> = (0..n).map(|_| st.poisson(lam) as f64).collect();
            let mean = xs.iter().sum::<f64>() / n as f64;
            let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
            let se = (lam / n as f64).sqrt();
            assert!((mean - lam).abs() < 6.0 * se, "lambda {lam}: mean {mean}");
            assert!((var / lam - 1.0).abs() < 0.03, "lambda {lam}: var {var}");
        }
        let n = 200_000;
        let zs: Vec<f64> = (0..n).map(|_| st.normal()).collect();
        let mean = zs.iter().sum::<f64>() / n as f64;
        let var = zs.iter().map(|z| z * z).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.015 && (var - 1.0).abs() < 0.015);
    }
}
