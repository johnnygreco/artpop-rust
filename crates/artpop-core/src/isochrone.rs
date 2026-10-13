//! Generic isochrones and their IMF-weighted integrals.

use crate::error::{Error, ErrorCode, Result, require_finite};
use crate::imf::{Imf, Moment};
use crate::interp::np_interp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Isochrone {
    pub initial_mass: Vec<f64>,
    pub current_mass: Vec<f64>,
    pub filters: Vec<String>,
    /// Magnitudes indexed `[filter][row]`.
    pub mags: Vec<Vec<f64>>,
    /// Magnitude system label of `mags` (AB, Vega, ST, or native/custom label).
    pub magnitude_system: String,
    pub eep: Option<Vec<f64>>,
    pub log_l: Option<Vec<f64>>,
    pub log_teff: Option<Vec<f64>>,
    /// Additional numeric columns retained for inspection/interpolation.
    pub extra: Vec<(String, Vec<f64>)>,
    /// Model identity/provenance (source, age, metallicity, rotation, ...).
    pub metadata: serde_json::Value,
}

/// Search hint for `partition_point(|v| v <= m)` on a sorted mass table:
/// `guide[g]` is the number of rows with mass `<= lo + g * step`, so the
/// answer for a query in bin `g` lies between `guide[g]` and `guide[g + 1]`.
/// The lookup is widened by one bin on each side so that rounding in the bin
/// computation can never exclude the answer; results are exactly those of a
/// full binary search.
struct MassGuide {
    lo: f64,
    inv_step: f64,
    bins: usize,
    guide: Vec<u32>,
}

impl MassGuide {
    fn new(x: &[f64]) -> MassGuide {
        let (lo, hi) = (x[0], x[x.len() - 1]);
        let bins = (2 * x.len()).clamp(1, 1 << 14);
        let step = if hi > lo { (hi - lo) / bins as f64 } else { 1.0 };
        let guide = (0..=bins).map(|g| x.partition_point(|v| *v <= lo + g as f64 * step) as u32).collect();
        MassGuide { lo, inv_step: 1.0 / step, bins, guide }
    }

    /// Number of table values `<= m` (`m` within the table range).
    #[inline]
    fn count_le(&self, x: &[f64], m: f64) -> usize {
        let g = (((m - self.lo) * self.inv_step) as usize).min(self.bins - 1);
        let lo = self.guide[g.saturating_sub(1)] as usize;
        let hi = (self.guide[(g + 2).min(self.bins)] as usize).min(x.len());
        lo + x[lo..hi].partition_point(|v| *v <= m)
    }
}

/// Remnant mass prescription (Renzini & Ciotti 1993 as used by ArtPop).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RemnantModel {
    pub black_hole_min_initial_mass: f64,
    pub neutron_star_min_initial_mass: f64,
    pub black_hole_mass_fraction: f64,
    pub neutron_star_mass: f64,
    pub white_dwarf_intercept: f64,
    pub white_dwarf_slope: f64,
}

impl Default for RemnantModel {
    fn default() -> Self {
        RemnantModel {
            black_hole_min_initial_mass: 40.0,
            neutron_star_min_initial_mass: 8.5,
            black_hole_mass_fraction: 0.5,
            neutron_star_mass: 1.4,
            white_dwarf_intercept: 0.48,
            white_dwarf_slope: 0.077,
        }
    }
}

/// ArtPop's default upper initial-mass limit for surviving-mass normalization.
pub const DEFAULT_IMF_UPPER_MASS: f64 = 120.0;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Crossing {
    pub initial_mass: f64,
    /// Lower row index of the bracketing segment (or the exact row).
    pub row: usize,
    pub exact_node: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SurvivingMass {
    pub live: f64,
    pub black_holes: f64,
    pub neutron_stars: f64,
    pub white_dwarfs: f64,
    pub total: f64,
    pub m_min: f64,
    pub m_max: f64,
}

impl Isochrone {
    pub fn validate(&self) -> Result<()> {
        let n = self.initial_mass.len();
        if n < 2 {
            return Err(Error::new(ErrorCode::ShapeMismatch, "An isochrone needs at least two rows."));
        }
        if self.current_mass.len() != n {
            return Err(Error::new(ErrorCode::ShapeMismatch, "initial_mass and current_mass lengths differ.")
                .with_details(serde_json::json!({ "initial_mass": n, "current_mass": self.current_mass.len() })));
        }
        if self.filters.len() != self.mags.len() {
            return Err(Error::new(ErrorCode::ShapeMismatch, "Filter names and magnitude columns differ in number."));
        }
        let mut seen = std::collections::HashSet::new();
        for (f, col) in self.filters.iter().zip(&self.mags) {
            if !seen.insert(f) {
                return Err(Error::invalid("/magnitude_columns", format!("Duplicate filter column '{f}'.")));
            }
            if col.len() != n {
                return Err(Error::new(ErrorCode::ShapeMismatch, format!("Magnitude column '{f}' has the wrong length.")));
            }
            require_finite(col, "/magnitude_columns", &format!("magnitude column {f}"))?;
        }
        require_finite(&self.initial_mass, "/initial_mass", "initial_mass")?;
        require_finite(&self.current_mass, "/current_mass", "current_mass")?;
        if self.initial_mass[0] <= 0.0 {
            return Err(Error::invalid("/initial_mass", "Initial masses must be positive."));
        }
        if let Some(i) = self.initial_mass.windows(2).position(|w| w[1] < w[0]) {
            return Err(Error::invalid("/initial_mass", "Initial masses must be monotonically nondecreasing.")
                .with_details(serde_json::json!({ "row": i + 1 })));
        }
        if self.current_mass.iter().any(|m| *m < 0.0) {
            return Err(Error::invalid("/current_mass", "Current masses must be nonnegative."));
        }
        for (name, col) in [("eep", &self.eep), ("log_L", &self.log_l), ("log_Teff", &self.log_teff)] {
            if let Some(c) = col {
                if c.len() != n {
                    return Err(Error::new(ErrorCode::ShapeMismatch, format!("Column {name} has the wrong length.")));
                }
                require_finite(c, &format!("/{name}"), name)?;
            }
        }
        for (name, c) in &self.extra {
            if c.len() != n {
                return Err(Error::new(ErrorCode::ShapeMismatch, format!("Column {name} has the wrong length.")));
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.initial_mass.len()
    }

    pub fn is_empty(&self) -> bool {
        self.initial_mass.is_empty()
    }

    pub fn m_min(&self) -> f64 {
        self.initial_mass[0]
    }

    pub fn m_max(&self) -> f64 {
        self.initial_mass[self.initial_mass.len() - 1]
    }

    pub fn filter_index(&self, band: &str) -> Result<usize> {
        self.filters.iter().position(|f| f == band).ok_or_else(|| Error::unknown_filter("/band", band, &self.filters))
    }

    pub fn band(&self, band: &str) -> Result<&[f64]> {
        Ok(&self.mags[self.filter_index(band)?])
    }

    /// Look up any numeric column by name (ArtPop aliases accepted).
    pub fn column(&self, name: &str) -> Option<&[f64]> {
        match name {
            "initial_mass" | "mini" => Some(&self.initial_mass),
            "current_mass" | "star_mass" | "mact" => Some(&self.current_mass),
            "eep" | "EEP" => self.eep.as_deref(),
            "log_L" | "log_l" => self.log_l.as_deref(),
            "log_Teff" | "log_teff" => self.log_teff.as_deref(),
            _ => self
                .filters
                .iter()
                .position(|f| f == name)
                .map(|i| self.mags[i].as_slice())
                .or_else(|| self.extra.iter().find(|(n, _)| n == name).map(|(_, c)| c.as_slice())),
        }
    }

    pub fn column_names(&self) -> Vec<String> {
        let mut names = vec!["initial_mass".to_string(), "current_mass".to_string()];
        if self.eep.is_some() {
            names.push("eep".into());
        }
        if self.log_l.is_some() {
            names.push("log_L".into());
        }
        if self.log_teff.is_some() {
            names.push("log_Teff".into());
        }
        names.extend(self.filters.iter().cloned());
        names.extend(self.extra.iter().map(|(n, _)| n.clone()));
        names
    }

    /// Linear interpolation of a column in initial mass (SciPy `interp1d`
    /// default); queries outside the mass range are rejected.
    pub fn interp_in_mass(&self, column: &[f64], masses: &[f64]) -> Result<Vec<f64>> {
        let (lo, hi) = (self.m_min(), self.m_max());
        let mut out = Vec::with_capacity(masses.len());
        for &m in masses {
            if !(m >= lo && m <= hi) {
                return Err(Error::domain("/initial_mass", format!("Mass {m} lies outside the isochrone range [{lo}, {hi}].")));
            }
            out.push(np_interp(&self.initial_mass, column, m));
        }
        Ok(out)
    }

    /// Interpolate several columns at the same masses with one bracket search
    /// per mass; bitwise identical to [`np_interp`] per column.
    pub fn interp_columns(&self, columns: &[&[f64]], masses: &[f64]) -> Result<Vec<Vec<f64>>> {
        use rayon::prelude::*;
        let x = &self.initial_mass;
        let n = x.len();
        let (lo, hi) = (self.m_min(), self.m_max());
        if let Some(m) = masses.iter().find(|m| !(**m >= lo && **m <= hi)) {
            return Err(Error::domain("/initial_mass", format!("Mass {m} lies outside the isochrone range [{lo}, {hi}].")));
        }
        // Bracket: (j, exact) with exact meaning "return column[j]". A guide
        // table over uniform mass bins bounds each binary search to the few
        // rows of one bin; the result is the same row a full search returns.
        let guide = MassGuide::new(x);
        let brackets: Vec<(u32, bool)> = masses
            .par_iter()
            .with_min_len(4096)
            .map(|&m| {
                if m == x[n - 1] {
                    return ((n - 1) as u32, true);
                }
                let j = guide.count_le(x, m) - 1;
                (j as u32, m == x[j])
            })
            .collect();
        // Columns in parallel, and within a column parallel chunks; each value
        // depends only on its own bracket, so the result is order-independent.
        // The segment slope `(y[j+1] - y[j]) / (x[j+1] - x[j])` is the same
        // IEEE quotient for every star in a segment, so it is computed once
        // per row instead of once per star (identical bits).
        Ok(columns
            .par_iter()
            .map(|y| {
                let slopes: Vec<f64> = (0..n - 1).map(|j| (y[j + 1] - y[j]) / (x[j + 1] - x[j])).collect();
                let eval = |(j, exact): (u32, bool), m: f64| -> f64 {
                    let j = j as usize;
                    if exact {
                        return y[j];
                    }
                    let r = slopes[j] * (m - x[j]) + y[j];
                    if r.is_nan() { np_interp(x, y, m) } else { r }
                };
                let mut out = crate::mem::zeroed_f64(masses.len());
                out.par_chunks_mut(crate::mem::PAR_CHUNK).zip(brackets.par_chunks(crate::mem::PAR_CHUNK)).zip(masses.par_chunks(crate::mem::PAR_CHUNK)).for_each(|((o, br), ms)| {
                    for ((v, b), m) in o.iter_mut().zip(br).zip(ms) {
                        *v = eval(*b, *m);
                    }
                });
                out
            })
            .collect())
    }

    /// IMF bin weights (ArtPop `imf_weights`): each row integrates the IMF
    /// number over its midpoint cell; `sum(m * w)` is then an integral over mass.
    pub fn imf_weights(&self, imf: &Imf, m_min_norm: Option<f64>, m_max_norm: Option<f64>, norm: Moment) -> Result<Vec<f64>> {
        let m_min = m_min_norm.unwrap_or(self.m_min());
        let m_max = m_max_norm.unwrap_or(self.m_max());
        if m_min > self.m_min() {
            return Err(Error::invalid("/m_min_norm", "The normalization minimum mass must be <= the isochrone minimum mass."));
        }
        if !(m_max > m_min) {
            return Err(Error::invalid("/m_max_norm", "The normalization maximum mass must exceed the minimum."));
        }
        let norm_value = imf.integrate(m_min, m_max, norm);
        let mini = &self.initial_mass;
        let n = mini.len();
        let mut w = Vec::with_capacity(n);
        for i in 0..n {
            let m1 = if i == 0 { m_min } else { mini[i] - 0.5 * (mini[i] - mini[i - 1]) };
            let m2 = if i == n - 1 { mini[i] } else { mini[i] + 0.5 * (mini[i + 1] - mini[i]) };
            w.push(imf.number(m1, m2) / norm_value);
        }
        Ok(w)
    }

    fn weighted_flux(&self, w: &[f64], band: &str, power: f64) -> Result<f64> {
        let mags = self.band(band)?;
        Ok(w.iter().zip(mags).map(|(wi, m)| wi * 10f64.powf(-0.4 * power * m)).sum())
    }

    /// IMF-weighted integrated magnitude per unit formed mass (or per star).
    pub fn ssp_mag(&self, band: &str, imf: &Imf, norm: Moment) -> Result<f64> {
        let w = self.imf_weights(imf, None, None, norm)?;
        Ok(-2.5 * self.weighted_flux(&w, band, 1.0)?.log10())
    }

    pub fn ssp_color(&self, blue: &str, red: &str, imf: &Imf) -> Result<f64> {
        let w = self.imf_weights(imf, None, None, Moment::Mass)?;
        Ok(-2.5 * (self.weighted_flux(&w, blue, 1.0)? / self.weighted_flux(&w, red, 1.0)?).log10())
    }

    pub fn ssp_sbf_mag(&self, band: &str, imf: &Imf) -> Result<f64> {
        let w = self.imf_weights(imf, None, None, Moment::Mass)?;
        Ok(-2.5 * (self.weighted_flux(&w, band, 2.0)? / self.weighted_flux(&w, band, 1.0)?).log10())
    }

    /// Mean current stellar mass per star formed (number-normalized weights).
    pub fn ssp_mean_star_mass(&self, imf: &Imf) -> Result<f64> {
        let w = self.imf_weights(imf, None, Some(self.m_max()), Moment::Number)?;
        Ok(w.iter().zip(&self.current_mass).map(|(a, b)| a * b).sum())
    }

    /// Surviving mass per unit initial mass formed on `[m_min, m_max]`.
    ///
    /// Remnant intervals are intersected with `[iso.m_max, m_max]`, so a
    /// restricted upper limit can never produce negative remnant mass.
    pub fn ssp_surviving_mass(&self, imf: &Imf, m_min: Option<f64>, m_max: Option<f64>, remnants: Option<&RemnantModel>) -> Result<SurvivingMass> {
        let lo = m_min.unwrap_or(self.m_min());
        let hi = m_max.unwrap_or(DEFAULT_IMF_UPPER_MASS);
        let w = self.imf_weights(imf, Some(lo), Some(hi), Moment::Mass)?;
        let live: f64 = self.current_mass.iter().zip(&w).map(|(a, b)| a * b).sum();
        let (mut bh, mut ns, mut wd) = (0.0, 0.0, 0.0);
        if let Some(r) = remnants {
            let norm = imf.mass(lo, hi);
            let top = self.m_max();
            let bh_lo = r.black_hole_min_initial_mass.max(top);
            bh = r.black_hole_mass_fraction * imf.mass(bh_lo, hi) / norm;
            if top <= r.black_hole_min_initial_mass {
                let ns_lo = r.neutron_star_min_initial_mass.max(top);
                ns = r.neutron_star_mass * imf.number(ns_lo, r.black_hole_min_initial_mass.min(hi)) / norm;
            }
            if top < r.neutron_star_min_initial_mass {
                let wd_hi = r.neutron_star_min_initial_mass.min(hi);
                wd = (r.white_dwarf_intercept * imf.number(top, wd_hi) + r.white_dwarf_slope * imf.mass(top, wd_hi)) / norm;
            }
        }
        Ok(SurvivingMass { live, black_holes: bh, neutron_stars: ns, white_dwarfs: wd, total: live + bh + ns + wd, m_min: lo, m_max: hi })
    }

    /// Every initial mass with the given magnitude (ArtPop `mag_to_mass`
    /// without duplicate exact-node roots).
    pub fn mag_to_mass(&self, mag: f64, band: &str) -> Result<Vec<Crossing>> {
        let x = self.band(band)?;
        let y = &self.initial_mass;
        let (min, max) = x.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
        if !(mag >= min && mag <= max) {
            return Err(Error::domain("/magnitude", format!("Magnitude {mag} is outside the isochrone range [{min}, {max}] in {band}."))
                .with_details(serde_json::json!({ "minimum": min, "maximum": max })));
        }
        let mut out: Vec<Crossing> = Vec::new();
        for i in 0..x.len() {
            if x[i] == mag {
                if !out.iter().any(|c| c.exact_node && c.row == i) {
                    out.push(Crossing { initial_mass: y[i], row: i, exact_node: true });
                }
                continue;
            }
            if i + 1 < x.len() {
                let dx = x[i + 1] - x[i];
                let inside = if dx > 0.0 { mag <= x[i + 1] && mag > x[i] } else { mag >= x[i + 1] && mag < x[i] };
                if inside {
                    if mag == x[i + 1] {
                        // Reported once as an exact node at row i + 1.
                        out.push(Crossing { initial_mass: y[i + 1], row: i + 1, exact_node: true });
                    } else {
                        let slope = (y[i + 1] - y[i]) / (x[i + 1] - x[i]);
                        out.push(Crossing { initial_mass: slope * (mag - x[i]) + y[i], row: i, exact_node: false });
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn nearest(&self, m: f64) -> (f64, usize) {
        let mut best = 0;
        let mut d = f64::INFINITY;
        for (i, v) in self.initial_mass.iter().enumerate() {
            let di = (v - m).abs();
            if di < d {
                d = di;
                best = i;
            }
        }
        (self.initial_mass[best], best)
    }

    /// ArtPop `calculate_mag_limit`: tabulate the sampled fraction on 500
    /// logarithmic masses, invert by linear interpolation, and convert the
    /// limiting mass to a magnitude.
    pub fn magnitude_limit(&self, imf: &Imf, band: &str, fraction: f64, moment: Moment, distance_modulus: f64) -> Result<(f64, f64)> {
        if !(fraction > 0.0 && fraction <= 1.0) {
            return Err(Error::invalid("/fraction", "The sampled fraction must satisfy 0 < fraction <= 1."));
        }
        let (lo, hi) = (self.m_min(), self.m_max());
        let n = 500;
        let (a, b) = (lo.log10(), hi.log10());
        let step = (b - a) / (n - 1) as f64;
        let mut m_vals: Vec<f64> = (0..n).map(|i| 10f64.powf(a + i as f64 * step)).collect();
        m_vals[n - 1] = 10f64.powf(b);
        let norm = imf.integrate(lo, hi, moment);
        let mut pairs: Vec<(f64, f64)> = m_vals.iter().map(|m| (imf.integrate(*m, hi, moment) / norm, *m)).collect();
        // SciPy interp1d sorts x ascending with a stable mergesort.
        pairs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
        let fx: Vec<f64> = pairs.iter().map(|p| p.0).collect();
        let fy: Vec<f64> = pairs.iter().map(|p| p.1).collect();
        if fraction < fx[0] || fraction > fx[n - 1] {
            return Err(Error::domain("/fraction", "Requested fraction is outside the tabulated range."));
        }
        let mass_limit = np_interp(&fx, &fy, fraction);
        let mag = np_interp(&self.initial_mass, self.band(band)?, mass_limit) + distance_modulus;
        Ok((mag, mass_limit))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fixture() -> Isochrone {
        let text = include_str!("../../../fixtures/lsst_feh_m1.00_10gyr_iso.csv");
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        let mut cols: Vec<Vec<f64>> = vec![Vec::new(); header.len()];
        for line in lines {
            for (i, v) in line.split(',').enumerate() {
                cols[i].push(v.parse().unwrap());
            }
        }
        let get = |name: &str| cols[header.iter().position(|h| *h == name).unwrap()].clone();
        let filters: Vec<String> = ["LSST_u", "LSST_g", "LSST_r", "LSST_i", "LSST_z", "LSST_y"].iter().map(|s| s.to_string()).collect();
        Isochrone {
            initial_mass: get("initial_mass"),
            current_mass: get("star_mass"),
            mags: filters.iter().map(|f| get(f)).collect(),
            filters,
            magnitude_system: "AB".into(),
            eep: Some(get("EEP")),
            log_l: Some(get("log_L")),
            log_teff: Some(get("log_Teff")),
            extra: vec![],
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn upstream_reference_values() {
        // ArtPop tests/test_stars.py::test_isochrone_calculations (7 decimals).
        let iso = fixture();
        iso.validate().unwrap();
        let k = Imf::kroupa();
        assert!((iso.ssp_mag("LSST_i", &k, Moment::Mass).unwrap() - 4.7117543).abs() < 5e-8);
        assert!((iso.ssp_sbf_mag("LSST_i", &k).unwrap() - -1.4292711).abs() < 5e-8);
        assert!((iso.ssp_color("LSST_u", "LSST_y", &k).unwrap() - 2.2320949).abs() < 5e-8);
        let s = Imf::salpeter();
        let rem = RemnantModel::default();
        assert!((iso.ssp_surviving_mass(&s, None, None, Some(&rem)).unwrap().total - 0.7014041).abs() < 5e-8);
        assert!((iso.ssp_surviving_mass(&s, None, None, None).unwrap().total - 0.5809369).abs() < 5e-8);
    }

    #[test]
    fn mass_guide_matches_full_binary_search() {
        let iso = fixture();
        let x = &iso.initial_mass;
        let guide = MassGuide::new(x);
        let mut st = crate::rng::Seed(6).stream("guide", &[]);
        for _ in 0..300_000 {
            let m = st.uniform_range(iso.m_min(), iso.m_max());
            assert_eq!(guide.count_le(x, m), x.partition_point(|v| *v <= m));
        }
        for &m in x.iter().chain([iso.m_min(), iso.m_max()].iter()) {
            assert_eq!(guide.count_le(x, m), x.partition_point(|v| *v <= m));
            let up = f64::from_bits(m.to_bits() + 1).min(iso.m_max());
            let down = f64::from_bits(m.to_bits() - 1).max(iso.m_min());
            assert_eq!(guide.count_le(x, up), x.partition_point(|v| *v <= up));
            assert_eq!(guide.count_le(x, down), x.partition_point(|v| *v <= down));
        }
        // Duplicate masses and a tiny table.
        let dup = [0.1, 0.2, 0.2, 0.2, 0.5, 0.9, 0.9];
        let g = MassGuide::new(&dup);
        for m in [0.1, 0.15, 0.2, 0.2000001, 0.5, 0.7, 0.9] {
            assert_eq!(g.count_le(&dup, m), dup.partition_point(|v| *v <= m));
        }
    }

    #[test]
    fn restricted_remnant_limit_cannot_subtract_mass() {
        let iso = fixture();
        let k = Imf::kroupa();
        let rem = RemnantModel::default();
        let with = iso.ssp_surviving_mass(&k, None, Some(2.0), Some(&rem)).unwrap();
        let without = iso.ssp_surviving_mass(&k, None, Some(2.0), None).unwrap();
        assert!(with.black_holes == 0.0 && with.neutron_stars == 0.0);
        assert!(with.total >= without.total);
    }

    #[test]
    fn magnitude_inversion_returns_every_root() {
        let iso = fixture();
        let roots = iso.mag_to_mass(2.0, "LSST_i").unwrap();
        assert!(roots.len() >= 2, "giant branch magnitudes recur: {roots:?}");
        assert!(iso.mag_to_mass(-50.0, "LSST_i").is_err());
    }
}
