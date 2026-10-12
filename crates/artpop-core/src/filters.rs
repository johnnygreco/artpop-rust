//! Filter metadata, throughput-curve integrals, and magnitude-system offsets.
//!
//! The bundled metadata (164 MIST filters in 12 photometric systems and the
//! MIST zero-point table) is converted once from ArtPop's pickles into
//! reviewable JSON (`data/*.json`, see `data/provenance.json`) and embedded in
//! the binary. Missing conversions are explicit `MISSING_ZEROPOINT` errors,
//! never silent zero offsets.

use crate::error::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

const FILTERS_JSON: &str = include_str!("../../../data/filters.json");
const ZEROPOINTS_JSON: &str = include_str!("../../../data/zeropoints.json");
pub const PROVENANCE_JSON: &str = include_str!("../../../data/provenance.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterInfo {
    pub name: String,
    pub system: String,
    pub dlam_angstrom: f64,
    pub lam_eff_angstrom: f64,
}

#[derive(Debug, Deserialize)]
struct FiltersFile {
    systems: Vec<String>,
    filters: Vec<FilterInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroPoint {
    pub filter: String,
    pub native_system: String,
    pub vega_to_st: f64,
    pub vega_to_ab: f64,
}

#[derive(Debug, Deserialize)]
struct ZeroPointFile {
    zeropoints: Vec<ZeroPoint>,
}

pub struct Catalog {
    pub systems: Vec<String>,
    pub filters: Vec<FilterInfo>,
    by_name: HashMap<String, usize>,
    zeropoints: HashMap<String, ZeroPoint>,
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let f: FiltersFile = serde_json::from_str(FILTERS_JSON).expect("embedded filters.json is valid");
        let z: ZeroPointFile = serde_json::from_str(ZEROPOINTS_JSON).expect("embedded zeropoints.json is valid");
        let by_name = f.filters.iter().enumerate().map(|(i, x)| (x.name.clone(), i)).collect();
        let zeropoints = z.zeropoints.into_iter().map(|x| (x.filter.clone(), x)).collect();
        Catalog { systems: f.systems, filters: f.filters, by_name, zeropoints }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum MagSystem {
    AB,
    Vega,
    ST,
}

impl MagSystem {
    pub fn parse(s: &str, path: &str) -> Result<MagSystem> {
        match s.to_ascii_lowercase().as_str() {
            "ab" => Ok(MagSystem::AB),
            "vega" => Ok(MagSystem::Vega),
            "st" => Ok(MagSystem::ST),
            _ => Err(Error::invalid(path, format!("Unknown magnitude system '{s}'; expected AB, Vega, or ST."))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MagSystem::AB => "AB",
            MagSystem::Vega => "Vega",
            MagSystem::ST => "ST",
        }
    }
}

impl Catalog {
    pub fn get(&self, name: &str) -> Option<&FilterInfo> {
        self.by_name.get(name).map(|i| &self.filters[*i])
    }

    pub fn system_filters(&self, system: &str) -> Option<Vec<&FilterInfo>> {
        let canonical = self.canonical_system(system)?;
        Some(self.filters.iter().filter(|f| f.system == canonical).collect())
    }

    /// Case-insensitive lookup of a photometric system name.
    pub fn canonical_system(&self, system: &str) -> Option<&str> {
        self.systems.iter().find(|s| s.eq_ignore_ascii_case(system)).map(|s| s.as_str())
    }

    pub fn zeropoint(&self, filter: &str) -> Option<&ZeroPoint> {
        self.zeropoints.get(filter)
    }

    /// Native MIST magnitude system of a filter.
    pub fn native_system(&self, filter: &str) -> Result<MagSystem> {
        let z = self.zeropoint(filter).ok_or_else(|| missing_zeropoint(filter))?;
        MagSystem::parse(&z.native_system, "/native_system")
    }

    /// Offset to add to a magnitude in `from` to express it in `to`
    /// (ArtPop `ZeroPointConverter`; defined relative to the native system).
    pub fn offset(&self, filter: &str, from: MagSystem, to: MagSystem) -> Result<f64> {
        if from == to {
            return Ok(0.0);
        }
        let z = self.zeropoint(filter).ok_or_else(|| missing_zeropoint(filter))?;
        // Express each system's magnitude as Vega + offset_vega_to(system).
        let rel = |s: MagSystem| -> f64 {
            match s {
                MagSystem::Vega => 0.0,
                MagSystem::AB => z.vega_to_ab,
                MagSystem::ST => z.vega_to_st,
            }
        };
        Ok(rel(to) - rel(from))
    }
}

fn missing_zeropoint(filter: &str) -> Error {
    Error::new(ErrorCode::MissingZeropoint, format!("No AB/Vega/ST zero-point conversion is available for filter '{filter}'."))
        .with_details(serde_json::json!({ "filter": filter, "table": "MIST zeropoints.txt (bundled)" }))
}

/// A filter throughput curve with wavelengths in Angstrom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCurve {
    pub name: String,
    pub wavelength_angstrom: Vec<f64>,
    pub transmission: Vec<f64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CurveProperties {
    pub effective_throughput: f64,
    pub lam_eff_angstrom: f64,
    pub lam_pivot_angstrom: f64,
    pub dlam_angstrom: f64,
}

/// `numpy.trapz(y, x)`.
pub fn trapz(y: &[f64], x: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..x.len().saturating_sub(1) {
        s += (x[i + 1] - x[i]) * (y[i + 1] + y[i]) / 2.0;
    }
    s
}

impl FilterCurve {
    pub fn validate(&self) -> Result<()> {
        let n = self.wavelength_angstrom.len();
        if n < 2 || n != self.transmission.len() {
            return Err(Error::new(ErrorCode::ShapeMismatch, format!("Filter curve '{}' needs at least two wavelength/transmission pairs of equal length.", self.name)));
        }
        crate::error::require_finite(&self.wavelength_angstrom, "/wavelength", "wavelength")?;
        crate::error::require_finite(&self.transmission, "/transmission", "transmission")?;
        if self.wavelength_angstrom.iter().any(|w| *w <= 0.0) || self.wavelength_angstrom.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Error::invalid("/wavelength", format!("Filter curve '{}' wavelengths must be positive and strictly increasing.", self.name)));
        }
        if self.transmission.iter().any(|t| *t < 0.0) {
            return Err(Error::invalid("/transmission", format!("Filter curve '{}' has negative transmission.", self.name)));
        }
        if !(trapz(&self.transmission, &self.wavelength_angstrom) > 0.0) {
            return Err(Error::invalid("/transmission", format!("Filter curve '{}' has zero integrated transmission.", self.name)));
        }
        Ok(())
    }

    /// Fukugita et al. (1996) definitions exactly as ArtPop's `FilterSystem`.
    pub fn properties(&self) -> Result<CurveProperties> {
        self.validate()?;
        let lam = &self.wavelength_angstrom;
        let tr = &self.transmission;
        let ln_lam: Vec<f64> = lam.iter().map(|l| l.ln()).collect();
        let theff = trapz(tr, &ln_lam);
        let ln_tr: Vec<f64> = ln_lam.iter().zip(tr).map(|(l, t)| l * t).collect();
        let lam_eff = (trapz(&ln_tr, &ln_lam) / theff).exp();
        let lam_tr: Vec<f64> = lam.iter().zip(tr).map(|(l, t)| l * t).collect();
        let lam_pivot = (trapz(&lam_tr, lam) / theff).sqrt();
        let tmax = tr.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let dlam = trapz(tr, lam) / tmax;
        Ok(CurveProperties { effective_throughput: theff, lam_eff_angstrom: lam_eff, lam_pivot_angstrom: lam_pivot, dlam_angstrom: dlam })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_complete() {
        let c = catalog();
        assert_eq!(c.systems.len(), 12);
        assert_eq!(c.filters.len(), 164);
        assert_eq!(c.system_filters("lsst").unwrap().len(), 6);
        let i = c.get("LSST_i").unwrap();
        assert!(i.lam_eff_angstrom > 7000.0 && i.dlam_angstrom > 1000.0);
    }

    #[test]
    fn conversions_follow_zero_point_converter() {
        let c = catalog();
        // CFHT_u is natively AB with Vega/AB = 0.367125 (zeropoints.txt).
        assert_eq!(c.offset("CFHT_u", MagSystem::AB, MagSystem::AB).unwrap(), 0.0);
        assert!((c.offset("CFHT_u", MagSystem::AB, MagSystem::Vega).unwrap() + 0.367125).abs() < 1e-12);
        let ab_to_st = c.offset("CFHT_u", MagSystem::AB, MagSystem::ST).unwrap();
        assert!((ab_to_st - (-0.424539 - 0.367125)).abs() < 1e-12);
        assert_eq!(c.offset("not_a_filter", MagSystem::AB, MagSystem::Vega).unwrap_err().code, ErrorCode::MissingZeropoint);
    }

    #[test]
    fn analytic_box_filter() {
        let curve = FilterCurve { name: "box".into(), wavelength_angstrom: vec![4000.0, 5000.0, 6000.0], transmission: vec![1.0, 1.0, 1.0] };
        let p = curve.properties().unwrap();
        assert!((p.dlam_angstrom - 2000.0).abs() < 1e-9);
        assert!((p.effective_throughput - (6000.0f64 / 4000.0).ln()).abs() < 1e-12);
    }
}
