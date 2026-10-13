//! Initial mass functions: evaluation, integration, and sampling.
//!
//! An IMF is a continuous piecewise power law `dN/dm = C_i m^{a_i}` on
//! segments separated by increasing break masses. The raw (unnormalized)
//! scale reproduces ArtPop 0.1.2 `IMFIntegrator.weights(norm_type=None)`:
//! the first segment equals one at the first break, and single power laws use
//! ArtPop's artificial reference mass 301 Msun.
//!
//! Integrals use `lo^q * expm1(q ln(hi/lo)) / q` with the exact logarithmic
//! branch at `q = 0`, so custom slopes of -1 and -2 are valid (upstream divides
//! by zero there).

use crate::error::{Error, Result};
use crate::rng::Seed;
use serde::Serialize;

/// Upstream reference mass for single-slope laws (`b = [301, 302]` in ArtPop).
pub const SINGLE_SLOPE_REFERENCE_MASS: f64 = 301.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Imf {
    pub name: String,
    pub slopes: Vec<f64>,
    pub breaks: Vec<f64>,
    #[serde(skip)]
    coeffs: Vec<f64>,
}

impl<'de> serde::Deserialize<'de> for Imf {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            name: String,
            slopes: Vec<f64>,
            breaks: Vec<f64>,
        }
        let r = Raw::deserialize(d)?;
        Imf::piecewise(&r.name, r.slopes, r.breaks).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Moment {
    Number,
    Mass,
}

impl Imf {
    pub fn kroupa() -> Imf {
        Imf::piecewise("kroupa", vec![-0.3, -1.3, -2.3], vec![0.08, 0.5]).expect("valid preset")
    }

    pub fn scalo() -> Imf {
        Imf::piecewise("scalo", vec![-1.2, -2.7, -2.3], vec![1.0, 10.0]).expect("valid preset")
    }

    pub fn salpeter() -> Imf {
        Imf::piecewise("salpeter", vec![-2.35], vec![]).expect("valid preset")
    }

    pub fn preset(name: &str) -> Option<Imf> {
        match name {
            "kroupa" => Some(Imf::kroupa()),
            "scalo" => Some(Imf::scalo()),
            "salpeter" => Some(Imf::salpeter()),
            _ => None,
        }
    }

    /// Build a continuous piecewise power law with `slopes.len() == breaks.len() + 1`.
    pub fn piecewise(name: &str, slopes: Vec<f64>, breaks: Vec<f64>) -> Result<Imf> {
        if slopes.is_empty() || slopes.len() != breaks.len() + 1 {
            return Err(Error::invalid("/imf", "An IMF needs exactly one more slope than break masses.")
                .with_details(serde_json::json!({ "slopes": slopes.len(), "breaks": breaks.len() })));
        }
        if slopes.iter().any(|a| !a.is_finite()) {
            return Err(Error::nonfinite("/imf/slopes", "IMF slopes must be finite."));
        }
        for (i, b) in breaks.iter().enumerate() {
            if !b.is_finite() || *b <= 0.0 {
                return Err(Error::invalid(&format!("/imf/breaks/{i}"), "IMF break masses must be finite and positive."));
            }
            if i > 0 && *b <= breaks[i - 1] {
                return Err(Error::invalid(&format!("/imf/breaks/{i}"), "IMF break masses must be strictly increasing."));
            }
        }
        let reference = breaks.first().copied().unwrap_or(SINGLE_SLOPE_REFERENCE_MASS);
        let mut coeffs = Vec::with_capacity(slopes.len());
        coeffs.push(reference.powf(-slopes[0]));
        for i in 1..slopes.len() {
            let prev = coeffs[i - 1];
            coeffs.push(prev * breaks[i - 1].powf(slopes[i - 1] - slopes[i]));
        }
        Ok(Imf { name: name.to_string(), slopes, breaks, coeffs })
    }

    #[inline]
    pub fn segment(&self, m: f64) -> usize {
        self.breaks.iter().take_while(|b| m >= **b).count()
    }

    /// Unnormalized `dN/dm`, identical in scale to ArtPop's raw weights.
    #[inline]
    pub fn raw(&self, m: f64) -> f64 {
        let i = self.segment(m);
        self.coeffs[i] * m.powf(self.slopes[i])
    }

    /// `∫_lo^hi m^k dN/dm dm` for `k = 0` (number) or `1` (mass).
    /// Empty or reversed intervals contribute zero.
    pub fn integrate(&self, lo: f64, hi: f64, moment: Moment) -> f64 {
        if !(hi > lo) || lo <= 0.0 {
            return 0.0;
        }
        let k = match moment {
            Moment::Number => 0.0,
            Moment::Mass => 1.0,
        };
        let mut sum = 0.0;
        let mut comp = 0.0;
        let mut start = lo;
        for seg in 0..self.slopes.len() {
            let seg_hi = if seg < self.breaks.len() { self.breaks[seg] } else { f64::INFINITY };
            let seg_lo = if seg == 0 { 0.0 } else { self.breaks[seg - 1] };
            if seg_hi <= start || seg_lo >= hi {
                continue;
            }
            let a = start.max(seg_lo);
            let b = hi.min(seg_hi);
            if b > a {
                let term = self.coeffs[seg] * power_integral(a, b, self.slopes[seg] + k);
                // Neumaier compensated summation.
                let t = sum + term;
                if sum.abs() >= term.abs() {
                    comp += (sum - t) + term;
                } else {
                    comp += (term - t) + sum;
                }
                sum = t;
            }
            start = b;
            if start >= hi {
                break;
            }
        }
        sum + comp
    }

    pub fn number(&self, lo: f64, hi: f64) -> f64 {
        self.integrate(lo, hi, Moment::Number)
    }

    pub fn mass(&self, lo: f64, hi: f64) -> f64 {
        self.integrate(lo, hi, Moment::Mass)
    }

    /// Mean initial mass of stars drawn on `[lo, hi]`.
    pub fn mean_mass(&self, lo: f64, hi: f64) -> f64 {
        self.mass(lo, hi) / self.number(lo, hi)
    }
}

/// `∫_a^b m^p dm` for `0 < a < b`, stable near `p = -1` and for narrow
/// intervals: `b - a` is exact for close bounds (Sterbenz), so
/// `ln_1p((b - a) / a)` keeps full relative precision of `ln(b / a)`.
pub fn power_integral(a: f64, b: f64, p: f64) -> f64 {
    let q = p + 1.0;
    let l = ((b - a) / a).ln_1p();
    if q == 0.0 {
        return l;
    }
    a.powf(q) * (q * l).exp_m1() / q
}

/// Upstream `sample_imf` discretization: logarithmic edges, midpoint weights,
/// cumulative sum, linear interpolation of the inverse with `m_min` below the
/// first CDF value (SciPy `interp1d(..., bounds_error=False, fill_value=m_min)`).
#[derive(Debug, Clone)]
pub struct TabulatedInverse {
    pub m_min: f64,
    pub m_max: f64,
    pub num_edges: usize,
    cdf: Vec<f64>,
    mass: Vec<f64>,
    /// `guide[g]` = number of CDF values `<= g / G` (search hint; exact results
    /// are identical to a full binary search).
    guide: Vec<u32>,
}

pub const DEFAULT_NUM_EDGES: usize = 100_000;

impl TabulatedInverse {
    pub fn new(imf: &Imf, m_min: f64, m_max: f64, num_edges: usize) -> Result<Self> {
        validate_mass_range(m_min, m_max)?;
        if num_edges < 3 {
            return Err(Error::invalid("/num_mass_edges", "The tabulated inverse needs at least 3 mass edges."));
        }
        let start = m_min.log10();
        let stop = m_max.log10();
        let step = (stop - start) / (num_edges - 1) as f64;
        let mut edges: Vec<f64> = (0..num_edges).map(|i| 10f64.powf(start + i as f64 * step)).collect();
        edges[num_edges - 1] = 10f64.powf(stop);
        let n = num_edges - 1;
        let mut mass = Vec::with_capacity(n);
        let mut cdf = Vec::with_capacity(n);
        let mut acc = 0.0;
        for i in 0..n {
            let mid = (edges[i + 1] + edges[i]) / 2.0;
            let dm = edges[i + 1] - edges[i];
            acc += imf.raw(mid) * dm;
            mass.push(mid);
            cdf.push(acc);
        }
        let max = cdf.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if !(max > 0.0) || !max.is_finite() {
            return Err(Error::new(crate::error::ErrorCode::NumericalConvergence, "IMF CDF has no positive mass."));
        }
        for c in &mut cdf {
            *c /= max;
        }
        let g = cdf.len();
        let guide = (0..=g).map(|k| cdf.partition_point(|v| *v <= k as f64 / g as f64) as u32).collect();
        Ok(TabulatedInverse { m_min, m_max, num_edges, cdf, mass, guide })
    }

    /// Number of CDF values `<= u`, using the guide table to bound the search.
    #[inline]
    fn count_le(&self, u: f64) -> usize {
        let g = self.cdf.len();
        let k = ((u * g as f64) as usize).min(g - 1);
        // Widen by one bucket on each side so rounding of u*G or k/G can
        // never exclude the answer: values <= (k-1)/G are <= u, and values
        // > (k+2)/G are > u.
        let lo = self.guide[k.saturating_sub(1)] as usize;
        let hi = (self.guide[(k + 2).min(g)] as usize).min(g);
        lo + self.cdf[lo..hi].partition_point(|v| *v <= u)
    }

    /// SciPy `interp1d(cdf, mass, fill_value=m_min)`, which uses
    /// `numpy.interp` for these inputs: exact node values, linear otherwise.
    #[inline]
    pub fn quantile(&self, u: f64) -> f64 {
        let x = &self.cdf;
        let y = &self.mass;
        let n = x.len();
        if u < x[0] || u > x[n - 1] {
            return self.m_min;
        }
        if u == x[n - 1] {
            return y[n - 1];
        }
        let j = self.count_le(u) - 1;
        if u == x[j] {
            return y[j];
        }
        let slope = (y[j + 1] - y[j]) / (x[j + 1] - x[j]);
        slope * (u - x[j]) + y[j]
    }
}

/// Exact inversion of the piecewise power-law CDF on `[m_min, m_max]`.
#[derive(Debug, Clone)]
pub struct AnalyticInverse {
    segments: Vec<(f64, f64, f64, f64)>, // (lo, hi, q = slope + 1, cumulative fraction at hi)
}

impl AnalyticInverse {
    pub fn new(imf: &Imf, m_min: f64, m_max: f64) -> Result<Self> {
        validate_mass_range(m_min, m_max)?;
        let total = imf.number(m_min, m_max);
        let mut segments = Vec::new();
        let mut acc = 0.0;
        let mut lo = m_min;
        for seg in 0..imf.slopes.len() {
            let seg_hi = if seg < imf.breaks.len() { imf.breaks[seg] } else { f64::INFINITY };
            if seg_hi <= lo {
                continue;
            }
            let hi = seg_hi.min(m_max);
            acc += imf.number(lo, hi) / total;
            segments.push((lo, hi, imf.slopes[seg] + 1.0, acc));
            lo = hi;
            if lo >= m_max {
                break;
            }
        }
        if let Some(last) = segments.last_mut() {
            last.3 = 1.0;
        }
        Ok(AnalyticInverse { segments })
    }

    pub fn quantile(&self, u: f64) -> f64 {
        let mut prev = 0.0;
        for &(lo, hi, q, cum) in &self.segments {
            if u < cum || cum >= 1.0 {
                let frac = ((u - prev) / (cum - prev)).clamp(0.0, 1.0);
                let l = (hi / lo).ln();
                let m = if q == 0.0 {
                    lo * (frac * l).exp()
                } else {
                    lo * ((frac * (q * l).exp_m1()).ln_1p() / q).exp()
                };
                return m.clamp(lo, hi);
            }
            prev = cum;
        }
        self.segments.last().map(|s| s.1).unwrap_or(f64::NAN)
    }
}

fn validate_mass_range(m_min: f64, m_max: f64) -> Result<()> {
    if !m_min.is_finite() || !m_max.is_finite() || m_min <= 0.0 || m_max <= m_min {
        return Err(Error::invalid("/mass_range", "Mass bounds must be finite with 0 < m_min < m_max.")
            .with_details(serde_json::json!({ "m_min": crate::error::finite_or_null(m_min), "m_max": crate::error::finite_or_null(m_max) })));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SamplingAlgorithm {
    TabulatedInverse,
    AnalyticInverse,
}

/// A configured inverse-CDF sampler.
pub enum Sampler {
    Tabulated(TabulatedInverse),
    Analytic(AnalyticInverse),
}

impl Sampler {
    pub fn new(imf: &Imf, m_min: f64, m_max: f64, algorithm: SamplingAlgorithm, num_edges: usize) -> Result<Self> {
        Ok(match algorithm {
            SamplingAlgorithm::TabulatedInverse => Sampler::Tabulated(TabulatedInverse::new(imf, m_min, m_max, num_edges)?),
            SamplingAlgorithm::AnalyticInverse => Sampler::Analytic(AnalyticInverse::new(imf, m_min, m_max)?),
        })
    }

    #[inline]
    pub fn quantile(&self, u: f64) -> f64 {
        match self {
            Sampler::Tabulated(t) => t.quantile(u),
            Sampler::Analytic(a) => a.quantile(u),
        }
    }

    /// Draw `n` masses in parallel fixed-size blocks (deterministic).
    pub fn sample(&self, n: usize, seed: Seed, stage: &str, keys: &[&str]) -> Vec<f64> {
        let mut out = crate::mem::zeroed_f64(n);
        crate::rng::par_blocks(seed, stage, keys, &mut out, |st, chunk| {
            for v in chunk.iter_mut() {
                *v = self.quantile(st.uniform());
            }
        });
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoppingPolicy {
    /// ArtPop `build_galaxy`: keep adding whole batches until the target is
    /// exceeded and retain the final batch.
    WholeBatch,
    /// Stop at the star whose inclusion brings the total closest to the target.
    NearestTarget,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildOutcome {
    pub masses: Vec<f64>,
    pub target_mass: f64,
    pub realized_mass: f64,
    pub draws: u64,
    pub batch_size: u64,
    pub batches: u64,
}

/// Draw stars until their summed initial mass reaches `target`.
#[allow(clippy::too_many_arguments)]
pub fn build_to_mass(
    sampler: &Sampler,
    target: f64,
    batch_size: u64,
    policy: StoppingPolicy,
    max_stars: u64,
    seed: Seed,
    stage: &str,
    keys: &[&str],
) -> Result<BuildOutcome> {
    if !target.is_finite() || target < 0.0 {
        return Err(Error::invalid("/target_mass", "Target mass must be finite and nonnegative."));
    }
    let batch = batch_size.max(1);
    let mut masses: Vec<f64> = Vec::new();
    let mut total = 0.0;
    let mut draws = 0u64;
    let mut batches = 0u64;
    while total < target {
        if draws + batch > max_stars {
            return Err(Error::new(crate::error::ErrorCode::ResourceLimit, "Reaching the target mass would exceed the configured star limit.")
                .with_details(serde_json::json!({ "max_stars": max_stars, "drawn": draws, "target_mass": target, "realized_mass": total })));
        }
        batches += 1;
        let start = masses.len();
        let label = format!("batch{batches}");
        let mut k: Vec<&str> = keys.to_vec();
        k.push(&label);
        masses.extend(sampler.sample(batch as usize, seed, stage, &k));
        draws += batch;
        match policy {
            StoppingPolicy::WholeBatch => {
                total += masses[start..].iter().sum::<f64>();
            }
            StoppingPolicy::NearestTarget => {
                let mut cut = masses.len();
                for i in start..masses.len() {
                    let next = total + masses[i];
                    if next >= target {
                        let include = (next - target).abs() <= (target - total).abs();
                        cut = if include { i + 1 } else { i };
                        total = if include { next } else { total };
                        masses.truncate(cut);
                        return Ok(BuildOutcome { realized_mass: total, masses, target_mass: target, draws, batch_size: batch, batches });
                    }
                    total = next;
                }
                let _ = cut;
            }
        }
    }
    Ok(BuildOutcome { realized_mass: total, masses, target_mass: target, draws, batch_size: batch, batches })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Seed;

    #[test]
    fn kroupa_raw_matches_upstream_formula() {
        let imf = Imf::kroupa();
        // ArtPop: alpha by segment, m_break = b1 or b2*(b1/b2)**(a2/a3).
        let up = |m: f64| -> f64 {
            let (a1, a2, a3, b1, b2) = (-0.3f64, -1.3f64, -2.3f64, 0.08f64, 0.5f64);
            let alpha = if m < b1 { a1 } else if m < b2 { a2 } else { a3 };
            let mb = if m < b2 { b1 } else { b2 * (b1 / b2).powf(a2 / a3) };
            (m / mb).powf(alpha)
        };
        for &m in &[0.01, 0.08, 0.1, 0.49, 0.5, 1.0, 30.0, 120.0] {
            assert!((imf.raw(m) / up(m) - 1.0).abs() < 1e-13, "m={m}");
        }
        let s = Imf::salpeter();
        assert!((s.raw(2.0) / (2.0f64 / 301.0).powf(-2.35) - 1.0).abs() < 1e-14);
    }

    #[test]
    fn integrals_handle_logarithmic_slopes() {
        let imf = Imf::piecewise("custom", vec![-1.0], vec![]).unwrap();
        let expected = 301.0 * (10.0f64 / 1.0).ln();
        assert!((imf.number(1.0, 10.0) / expected - 1.0).abs() < 1e-14);
        let imf2 = Imf::piecewise("custom", vec![-2.0], vec![]).unwrap();
        assert!((imf2.mass(1.0, 10.0) / (301.0f64.powi(2) * 10.0f64.ln()) - 1.0).abs() < 1e-14);
        let zero = Imf::piecewise("flat", vec![0.0, -2.0], vec![1.0]).unwrap();
        assert!(zero.number(0.5, 2.0).is_finite());
    }

    #[test]
    fn upstream_kroupa_integrals() {
        // IMFIntegrator('kroupa').integrate(0.1, 120) and m_integrate(0.1, 120)
        // from ArtPop 0.1.2 (analytic formulas evaluated in Python).
        let imf = Imf::kroupa();
        let n = imf.number(0.1, 120.0);
        let m = imf.mass(0.1, 120.0);
        let up_n = {
            let (a0, a1, a2, b0, b1, e) = (-0.3f64, -1.3f64, -2.3f64, 0.08f64, 0.5f64, 1e-3f64);
            let c0 = b0.powf(a0);
            let c1 = b0.powf(a1);
            let c2 = (b1 * (b0 / b1).powf(a1 / a2)).powf(a2);
            let ind = |m: f64| -> f64 {
                if m > b1 {
                    (b0.powf(a0 + 1.) - e.powf(a0 + 1.)) / (a0 + 1.) / c0 + (b1.powf(a1 + 1.) - b0.powf(a1 + 1.)) / (a1 + 1.) / c1 + (m.powf(a2 + 1.) - b1.powf(a2 + 1.)) / (a2 + 1.) / c2
                } else if m > b0 {
                    (b0.powf(a0 + 1.) - e.powf(a0 + 1.)) / (a0 + 1.) / c0 + (m.powf(a1 + 1.) - b0.powf(a1 + 1.)) / (a1 + 1.) / c1
                } else {
                    (m.powf(a0 + 1.) - e.powf(a0 + 1.)) / (a0 + 1.) / c0
                }
            };
            ind(120.0) - ind(0.1)
        };
        assert!((n / up_n - 1.0).abs() < 1e-13);
        assert!(m > n * 0.1);
    }

    #[test]
    fn samplers_stay_in_range_and_agree() {
        let imf = Imf::kroupa();
        let t = Sampler::new(&imf, 0.1, 0.9, SamplingAlgorithm::TabulatedInverse, DEFAULT_NUM_EDGES).unwrap();
        let a = Sampler::new(&imf, 0.1, 0.9, SamplingAlgorithm::AnalyticInverse, 0).unwrap();
        if let Sampler::Tabulated(tab) = &t {
            // Guide-table search equals a full binary search everywhere.
            let mut st = Seed(4).stream("guide", &[]);
            for _ in 0..200_000 {
                let u = st.uniform();
                assert_eq!(tab.count_le(u), tab.cdf.partition_point(|v| *v <= u));
            }
            for &u in &[tab.cdf[0], tab.cdf[1], tab.cdf[500], 0.5, 1.0] {
                assert_eq!(tab.count_le(u), tab.cdf.partition_point(|v| *v <= u));
            }
        }
        for &u in &[0.0, 1e-9, 0.25, 0.5, 0.75, 0.999999] {
            let mt = t.quantile(u);
            let ma = a.quantile(u);
            assert!((0.1..=0.9).contains(&mt) && (0.1..=0.9).contains(&ma));
            if u > 1e-4 {
                assert!((mt / ma - 1.0).abs() < 2e-4, "u={u} {mt} {ma}");
            }
        }
        let out = build_to_mass(&a, 1000.0, 100, StoppingPolicy::WholeBatch, 1_000_000, Seed(1), "t", &[]).unwrap();
        assert!(out.realized_mass >= 1000.0 && out.masses.len() as u64 == out.draws);
        let near = build_to_mass(&a, 1000.0, 100, StoppingPolicy::NearestTarget, 1_000_000, Seed(1), "t", &[]).unwrap();
        assert!((near.realized_mass - 1000.0).abs() <= 0.9);
    }
}
