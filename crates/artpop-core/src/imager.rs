//! Instrument conversions and detector noise.
//!
//! Exactly one calibration path is configured per instrument: physical band
//! properties (AB flux density, effective width/wavelength, aperture,
//! efficiency) or instrumental zero points (magnitude of 1 count/s). Sky
//! surface brightness is converted per pixel by multiplying by the pixel
//! area in both paths (ArtPop 0.1.2 omits it for zero points).

use crate::error::{Error, ErrorCode, Result};
use crate::image::Image;
use crate::rng::{MAX_POISSON_MEAN, Seed, par_blocks};
use serde::{Deserialize, Serialize};

/// Planck constant in erg s (CODATA 2018, exact; Astropy 6 default).
pub const H_ERG_S: f64 = 6.62607015e-27;
/// AB magnitude zero point: `f_nu = 10^(-0.4 (m + 48.6))` erg/s/cm^2/Hz.
pub const AB_ZERO: f64 = 48.6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BandProperties {
    pub name: String,
    pub dlam_angstrom: f64,
    pub lam_eff_angstrom: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Calibration {
    Physical { bands: Vec<BandProperties>, diameter_m: f64, efficiency: f64 },
    Instrumental { zero_points: Vec<(String, f64)> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Instrument {
    pub calibration: Calibration,
    pub read_noise: f64,
    pub magnitude_system: String,
}

pub fn fnu_from_ab(mag: f64) -> f64 {
    10f64.powf((mag + AB_ZERO) / -2.5)
}

impl Instrument {
    pub fn validate(&self) -> Result<()> {
        crate::error::require_nonnegative(self.read_noise, "/instrument/read_noise_electrons", "Read noise")?;
        match &self.calibration {
            Calibration::Physical { bands, diameter_m, efficiency } => {
                crate::error::require_positive(*diameter_m, "/instrument/diameter", "Aperture diameter")?;
                crate::error::require_positive(*efficiency, "/instrument/efficiency", "Efficiency")?;
                if bands.is_empty() {
                    return Err(Error::invalid("/instrument", "A physical instrument needs at least one band."));
                }
                for b in bands {
                    crate::error::require_positive(b.dlam_angstrom, "/instrument/bands", "Band width")?;
                    crate::error::require_positive(b.lam_eff_angstrom, "/instrument/bands", "Effective wavelength")?;
                }
                if !self.magnitude_system.eq_ignore_ascii_case("AB") {
                    return Err(Error::invalid("/instrument/magnitude_system", "Physical photon-count conversion requires AB magnitudes; convert Vega/ST magnitudes first."));
                }
            }
            Calibration::Instrumental { zero_points } => {
                if zero_points.is_empty() {
                    return Err(Error::invalid("/instrument/zero_points", "At least one instrumental zero point is required."));
                }
                for (b, z) in zero_points {
                    if !z.is_finite() {
                        return Err(Error::nonfinite(&format!("/instrument/zero_points/{b}"), "Zero points must be finite."));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn bands(&self) -> Vec<String> {
        match &self.calibration {
            Calibration::Physical { bands, .. } => bands.iter().map(|b| b.name.clone()).collect(),
            Calibration::Instrumental { zero_points } => zero_points.iter().map(|z| z.0.clone()).collect(),
        }
    }

    fn missing(&self, band: &str) -> Error {
        Error::unknown_filter("/bandpass", band, &self.bands())
    }

    pub fn area_cm2(&self) -> Option<f64> {
        match &self.calibration {
            Calibration::Physical { diameter_m, .. } => {
                let r = diameter_m * 100.0 / 2.0;
                Some(std::f64::consts::PI * r * r)
            }
            _ => None,
        }
    }

    /// Counts per unit `10^(-0.4 m)`, i.e. `counts(m) = factor * 10^(-0.4 m)`.
    fn count_factor(&self, band: &str, exptime: f64) -> Result<f64> {
        match &self.calibration {
            Calibration::Physical { bands, efficiency, .. } => {
                let b = bands.iter().find(|b| b.name == band).ok_or_else(|| self.missing(band))?;
                let area = self.area_cm2().unwrap();
                Ok((b.dlam_angstrom / b.lam_eff_angstrom) * fnu_from_ab(0.0) / H_ERG_S * area * exptime * efficiency)
            }
            Calibration::Instrumental { zero_points } => {
                let z = zero_points.iter().find(|z| z.0 == band).ok_or_else(|| self.missing(band))?.1;
                Ok(10f64.powf(0.4 * z) * exptime)
            }
        }
    }

    /// Expected counts for each magnitude (ArtPop `mag_to_counts`).
    pub fn mag_to_counts(&self, mags: &[f64], band: &str, exptime: f64) -> Result<Vec<f64>> {
        crate::error::require_positive(exptime, "/exposure_s", "Exposure time")?;
        match &self.calibration {
            Calibration::Physical { bands, efficiency, .. } => {
                let b = bands.iter().find(|b| b.name == band).ok_or_else(|| self.missing(band))?;
                let area = self.area_cm2().unwrap();
                Ok(mags
                    .iter()
                    .map(|m| (b.dlam_angstrom / b.lam_eff_angstrom) * fnu_from_ab(*m) / H_ERG_S * area * exptime * efficiency)
                    .collect())
            }
            Calibration::Instrumental { zero_points } => {
                let z = zero_points.iter().find(|z| z.0 == band).ok_or_else(|| self.missing(band))?.1;
                Ok(mags.iter().map(|m| 10f64.powf(0.4 * (z - m)) * exptime).collect())
            }
        }
    }

    /// Counts per pixel for a surface brightness in mag/arcsec^2.
    pub fn sb_to_counts_per_pixel(&self, sb: f64, band: &str, exptime: f64, pixel_scale: f64) -> Result<f64> {
        crate::error::require_positive(pixel_scale, "/pixel_scale_arcsec", "Pixel scale")?;
        Ok(self.mag_to_counts(&[sb], band, exptime)?[0] * pixel_scale * pixel_scale)
    }

    /// Factor converting counts to flux at magnitude zero point `zpt`.
    pub fn calibration_factor(&self, band: &str, exptime: f64, zpt: f64) -> Result<f64> {
        crate::error::require_positive(exptime, "/exposure_s", "Exposure time")?;
        match &self.calibration {
            Calibration::Physical { .. } => {
                // ArtPop: 10^(0.4 zpt) 10^(0.4*48.6) / ((dlam/lam)/h) / t / area / eff.
                let f = self.count_factor(band, exptime)?;
                Ok(10f64.powf(0.4 * zpt) / f)
            }
            Calibration::Instrumental { zero_points } => {
                let z = zero_points.iter().find(|z| z.0 == band).ok_or_else(|| self.missing(band))?.1;
                Ok(10f64.powf(0.4 * (zpt - z)) / exptime)
            }
        }
    }
}

/// Validate expected counts: tiny negative FFT residuals are clamped to zero
/// (relative tolerance 1e-12 of the image maximum); material negatives fail.
pub fn sanitize_expected(img: &mut Image) -> Result<u64> {
    let max = img.data.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    let tol = 1e-12 * max.max(f64::MIN_POSITIVE);
    let mut clamped = 0;
    for v in &mut img.data {
        if !v.is_finite() {
            return Err(Error::nonfinite("/source", "Expected counts contain a non-finite value."));
        }
        if *v < 0.0 {
            if -*v <= tol {
                *v = 0.0;
                clamped += 1;
            } else {
                return Err(Error::new(ErrorCode::OutOfDomain, "Expected counts are materially negative; use a nonnegative PSF for physical observations.")
                    .with_details(serde_json::json!({ "value": *v, "tolerance": tol })));
            }
        }
    }
    Ok(clamped)
}

/// Poisson realization of `expected + sky` plus Gaussian read noise, drawn
/// in pixel blocks from the streams `observe.shot_noise/<band>` and
/// `observe.read_noise/<band>` (deterministic for any thread count).
pub fn realize_counts(expected: &Image, sky: f64, read_noise: f64, seed: Seed, band: &str) -> Result<Image> {
    crate::error::require_nonnegative(sky, "/sky_sb_mag_arcsec2", "Sky counts")?;
    let mut out: Vec<f64> = expected.data.iter().map(|e| e + sky).collect();
    if let Some(lam) = out.iter().find(|l| **l > MAX_POISSON_MEAN) {
        return Err(Error::new(ErrorCode::ResourceLimit, "Expected counts exceed the Poisson sampler's supported range.")
            .with_details(serde_json::json!({ "mean": lam, "maximum": MAX_POISSON_MEAN })));
    }
    par_blocks(seed, "observe.shot_noise", &[band], &mut out, |st, chunk| {
        for v in chunk.iter_mut() {
            *v = st.poisson(*v) as f64;
        }
    });
    if read_noise > 0.0 {
        par_blocks(seed, "observe.read_noise", &[band], &mut out, |st, chunk| {
            for v in chunk.iter_mut() {
                *v += read_noise * st.normal();
            }
        });
    }
    Ok(Image { width: expected.width, height: expected.height, data: out })
}

/// ArtPop's per-star magnitude error estimate (no PSF fitting or confusion).
pub fn magnitude_errors(counts: &[f64], sky: f64, read_noise: f64) -> Vec<f64> {
    counts
        .iter()
        .map(|c| {
            let n_s = (read_noise * read_noise + sky + c).sqrt() / c;
            2.5 * (1.0 + n_s).log10()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lsst() -> Instrument {
        let c = crate::filters::catalog();
        let bands = c
            .system_filters("LSST")
            .unwrap()
            .into_iter()
            .map(|f| BandProperties { name: f.name.clone(), dlam_angstrom: f.dlam_angstrom, lam_eff_angstrom: f.lam_eff_angstrom })
            .collect();
        Instrument { calibration: Calibration::Physical { bands, diameter_m: 2.0 / std::f64::consts::PI.sqrt(), efficiency: 1.0 }, read_noise: 0.0, magnitude_system: "AB".into() }
    }

    #[test]
    fn physical_conversion_scales_and_inverts() {
        let inst = lsst();
        assert!((inst.area_cm2().unwrap() - 1.0e4).abs() < 1e-8);
        let c1 = inst.mag_to_counts(&[20.0], "LSST_i", 60.0).unwrap()[0];
        let c2 = inst.mag_to_counts(&[20.0], "LSST_i", 120.0).unwrap()[0];
        assert!((c2 / c1 - 2.0).abs() < 1e-14);
        // Calibrated flux of a star equals 10^(0.4 (zpt - m)).
        let cal = inst.calibration_factor("LSST_i", 60.0, 27.0).unwrap();
        assert!((c1 * cal / 10f64.powf(0.4 * 7.0) - 1.0).abs() < 1e-12);
        let s1 = inst.sb_to_counts_per_pixel(21.0, "LSST_i", 60.0, 0.2).unwrap();
        let s2 = inst.sb_to_counts_per_pixel(21.0, "LSST_i", 60.0, 0.4).unwrap();
        assert!((s2 / s1 - 4.0).abs() < 1e-12);
    }

    #[test]
    fn instrumental_zero_point_scales_sky_with_pixel_area() {
        let inst = Instrument { calibration: Calibration::Instrumental { zero_points: vec![("my_i".into(), 25.0)] }, read_noise: 0.0, magnitude_system: "AB".into() };
        assert!((inst.mag_to_counts(&[25.0], "my_i", 100.0).unwrap()[0] - 100.0).abs() < 1e-12);
        let a = inst.sb_to_counts_per_pixel(21.0, "my_i", 100.0, 0.2).unwrap();
        let b = inst.sb_to_counts_per_pixel(21.0, "my_i", 100.0, 0.4).unwrap();
        assert!((b / a - 4.0).abs() < 1e-12);
        assert_eq!(inst.mag_to_counts(&[1.0], "LSST_i", 1.0).unwrap_err().code, ErrorCode::UnknownFilter);
    }
}
