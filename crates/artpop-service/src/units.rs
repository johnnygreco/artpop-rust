//! Unit-checked conversions to canonical internal units.

use crate::types::Quantity;
use artpop_core::error::{Error, ErrorCode, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Length,
    Angle,
    Time,
    Mass,
    Wavelength,
}

impl Dimension {
    fn name(self) -> &'static str {
        match self {
            Dimension::Length => "length",
            Dimension::Angle => "angle",
            Dimension::Time => "time",
            Dimension::Mass => "mass",
            Dimension::Wavelength => "wavelength",
        }
    }
}

const PC_M: f64 = 3.085_677_581_491_367e16;

/// (unit, dimension, factor to canonical unit)
/// Canonical units: pc, arcsec, s, Msun, Angstrom.
const UNITS: &[(&str, Dimension, f64)] = &[
    ("pc", Dimension::Length, 1.0),
    ("kpc", Dimension::Length, 1e3),
    ("Mpc", Dimension::Length, 1e6),
    ("Gpc", Dimension::Length, 1e9),
    ("m", Dimension::Length, 1.0 / PC_M),
    ("km", Dimension::Length, 1e3 / PC_M),
    ("cm", Dimension::Length, 1e-2 / PC_M),
    ("au", Dimension::Length, 1.495978707e11 / PC_M),
    ("lyr", Dimension::Length, 9460730472580800.0 / PC_M),
    ("arcsec", Dimension::Angle, 1.0),
    ("arcmin", Dimension::Angle, 60.0),
    ("deg", Dimension::Angle, 3600.0),
    ("rad", Dimension::Angle, 180.0 / std::f64::consts::PI * 3600.0),
    ("mas", Dimension::Angle, 1e-3),
    ("s", Dimension::Time, 1.0),
    ("min", Dimension::Time, 60.0),
    ("h", Dimension::Time, 3600.0),
    ("d", Dimension::Time, 86400.0),
    ("Msun", Dimension::Mass, 1.0),
    ("Angstrom", Dimension::Wavelength, 1.0),
    ("nm", Dimension::Wavelength, 10.0),
    ("um", Dimension::Wavelength, 1e4),
    ("micron", Dimension::Wavelength, 1e4),
];

pub fn convert(q: &Quantity, dim: Dimension, pointer: &str) -> Result<f64> {
    if !q.value.is_finite() {
        return Err(Error::nonfinite(&format!("{pointer}/value"), "Quantity values must be finite."));
    }
    let unit = match q.unit.as_str() {
        "AA" | "angstrom" | "Å" => "Angstrom",
        "M_sun" | "solMass" => "Msun",
        "radian" => "rad",
        "degree" => "deg",
        "sec" | "second" => "s",
        "hr" | "hour" => "h",
        "day" => "d",
        "ly" => "lyr",
        u => u,
    };
    match UNITS.iter().find(|(u, _, _)| *u == unit) {
        Some((_, d, f)) if *d == dim => Ok(q.value * f),
        Some((_, d, _)) => Err(Error::new(ErrorCode::UnitMismatch, format!("Unit '{}' is a {}; expected a {}.", q.unit, d.name(), dim.name()))
            .at(&format!("{pointer}/unit"))
            .with_details(serde_json::json!({ "received": q.unit, "expected_dimension": dim.name(), "accepted": accepted(dim) }))),
        None => Err(Error::invalid(&format!("{pointer}/unit"), format!("Unknown unit '{}'.", q.unit))
            .with_details(serde_json::json!({ "accepted": accepted(dim) }))),
    }
}

pub fn accepted(dim: Dimension) -> Vec<&'static str> {
    UNITS.iter().filter(|(_, d, _)| *d == dim).map(|(u, _, _)| *u).collect()
}

pub fn length_pc(q: &Quantity, pointer: &str) -> Result<f64> {
    convert(q, Dimension::Length, pointer)
}

/// Lengths in meters without a parsec round trip for metric units.
pub fn length_m(q: &Quantity, pointer: &str) -> Result<f64> {
    let direct = match q.unit.as_str() {
        "m" => Some(1.0),
        "cm" => Some(1e-2),
        "mm" => Some(1e-3),
        "km" => Some(1e3),
        _ => None,
    };
    match direct {
        Some(f) if q.value.is_finite() => Ok(q.value * f),
        _ => Ok(length_pc(q, pointer)? * PC_M),
    }
}

pub fn angle_arcsec(q: &Quantity, pointer: &str) -> Result<f64> {
    convert(q, Dimension::Angle, pointer)
}

pub fn angle_rad(q: &Quantity, pointer: &str) -> Result<f64> {
    Ok(convert(q, Dimension::Angle, pointer)? / 3600.0 * std::f64::consts::PI / 180.0)
}

pub fn time_s(q: &Quantity, pointer: &str) -> Result<f64> {
    convert(q, Dimension::Time, pointer)
}

pub fn mass_msun(q: &Quantity, pointer: &str) -> Result<f64> {
    convert(q, Dimension::Mass, pointer)
}

pub fn wavelength_angstrom(q: &Quantity, pointer: &str) -> Result<f64> {
    convert(q, Dimension::Wavelength, pointer)
}

/// FWHM in pixels from an angular quantity (or `pixel`) and a pixel scale.
pub fn fwhm_pixels(q: &Quantity, pixel_scale_arcsec: Option<f64>, pointer: &str) -> Result<f64> {
    if matches!(q.unit.as_str(), "pixel" | "pix" | "px") {
        artpop_core::error::require_positive(q.value, &format!("{pointer}/value"), "FWHM")?;
        return Ok(q.value);
    }
    let arcsec = angle_arcsec(q, pointer)?;
    let ps = pixel_scale_arcsec.ok_or_else(|| Error::invalid("/pixel_scale_arcsec", "An angular FWHM needs pixel_scale_arcsec."))?;
    artpop_core::error::require_positive(ps, "/pixel_scale_arcsec", "Pixel scale")?;
    artpop_core::error::require_positive(arcsec, &format!("{pointer}/value"), "FWHM")?;
    Ok(arcsec / ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_and_dimension_errors() {
        assert_eq!(length_pc(&Quantity::new(5.0, "Mpc"), "/d").unwrap(), 5e6);
        assert!((angle_rad(&Quantity::new(180.0, "deg"), "/t").unwrap() - std::f64::consts::PI).abs() < 1e-15);
        assert_eq!(time_s(&Quantity::new(2.0, "min"), "/t").unwrap(), 120.0);
        let e = length_pc(&Quantity::new(1.0, "arcsec"), "/d").unwrap_err();
        assert_eq!(e.code, ErrorCode::UnitMismatch);
        assert_eq!(e.path.as_deref(), Some("/d/unit"));
        assert_eq!(fwhm_pixels(&Quantity::new(0.6, "arcsec"), Some(0.2), "/f").unwrap(), 0.6 / 0.2);
    }
}
