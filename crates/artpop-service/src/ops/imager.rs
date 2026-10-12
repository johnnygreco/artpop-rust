//! Instrument descriptions and magnitude/count conversions.

use crate::types::{ArtifactInput, Quantity};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, Result};
use artpop_core::filters::catalog;
use artpop_core::imager::{BandProperties, Calibration, Instrument, fnu_from_ab};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn d10() -> Quantity {
    Quantity::new(10.0, "m")
}
fn one() -> f64 {
    1.0
}
fn ab() -> String {
    "AB".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BandWidth {
    pub dlam: Quantity,
    pub lam_eff: Quantity,
}

/// Exactly one calibration path (no silent precedence).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstrumentSpec {
    /// Bundled filter properties of one or more photometric systems (AB magnitudes).
    Physical {
        photometric_systems: Vec<String>,
        #[serde(default = "d10")]
        diameter: Quantity,
        #[serde(default = "one")]
        efficiency: f64,
        #[serde(default)]
        read_noise_electrons: f64,
        #[serde(default = "ab")]
        magnitude_system: String,
    },
    /// Explicit band widths and effective wavelengths (AB magnitudes).
    Bandpasses {
        bands: IndexMap<String, BandWidth>,
        #[serde(default = "d10")]
        diameter: Quantity,
        #[serde(default = "one")]
        efficiency: f64,
        #[serde(default)]
        read_noise_electrons: f64,
        #[serde(default = "ab")]
        magnitude_system: String,
    },
    /// Properties computed from a `filters.import` catalog (AB magnitudes).
    FilterCurves {
        catalog: ArtifactInput,
        #[serde(default = "d10")]
        diameter: Quantity,
        #[serde(default = "one")]
        efficiency: f64,
        #[serde(default)]
        read_noise_electrons: f64,
        #[serde(default = "ab")]
        magnitude_system: String,
    },
    /// Instrumental zero points: the magnitude giving 1 count per second.
    Instrumental {
        zero_points: IndexMap<String, f64>,
        #[serde(default = "ab")]
        magnitude_system: String,
        #[serde(default)]
        read_noise_electrons: f64,
    },
}

impl InstrumentSpec {
    pub fn build(&self, ctx: &Ctx, pointer: &str) -> Result<Instrument> {
        let meters = |q: &Quantity| -> Result<f64> { crate::units::length_m(q, &format!("{pointer}/diameter")) };
        let inst = match self {
            InstrumentSpec::Physical { photometric_systems, diameter, efficiency, read_noise_electrons, magnitude_system } => {
                let cat = catalog();
                let mut bands = Vec::new();
                for (i, s) in photometric_systems.iter().enumerate() {
                    let fs = cat.system_filters(s).ok_or_else(|| Error::invalid(&format!("{pointer}/photometric_systems/{i}"), format!("Unknown photometric system '{s}'.")).with_details(json!({ "available": cat.systems })))?;
                    bands.extend(fs.into_iter().map(|f| BandProperties { name: f.name.clone(), dlam_angstrom: f.dlam_angstrom, lam_eff_angstrom: f.lam_eff_angstrom }));
                }
                Instrument { calibration: Calibration::Physical { bands, diameter_m: meters(diameter)?, efficiency: *efficiency }, read_noise: *read_noise_electrons, magnitude_system: magnitude_system.clone() }
            }
            InstrumentSpec::Bandpasses { bands, diameter, efficiency, read_noise_electrons, magnitude_system } => {
                let b = bands
                    .iter()
                    .map(|(n, w)| {
                        Ok(BandProperties {
                            name: n.clone(),
                            dlam_angstrom: crate::units::wavelength_angstrom(&w.dlam, &format!("{pointer}/bands/{n}/dlam"))?,
                            lam_eff_angstrom: crate::units::wavelength_angstrom(&w.lam_eff, &format!("{pointer}/bands/{n}/lam_eff"))?,
                        })
                    })
                    .collect::<Result<_>>()?;
                Instrument { calibration: Calibration::Physical { bands: b, diameter_m: meters(diameter)?, efficiency: *efficiency }, read_noise: *read_noise_electrons, magnitude_system: magnitude_system.clone() }
            }
            InstrumentSpec::FilterCurves { catalog: c, diameter, efficiency, read_noise_electrons, magnitude_system } => {
                let l = ctx.store().load(&c.artifact_id, &format!("{pointer}/catalog/artifact_id"), &["filters"])?;
                let entries: Vec<serde_json::Value> = l.meta("filters")?;
                let bands = entries
                    .iter()
                    .map(|e| BandProperties { name: e["name"].as_str().unwrap_or_default().to_string(), dlam_angstrom: e["dlam_angstrom"].as_f64().unwrap_or(f64::NAN), lam_eff_angstrom: e["lam_eff_angstrom"].as_f64().unwrap_or(f64::NAN) })
                    .collect();
                Instrument { calibration: Calibration::Physical { bands, diameter_m: meters(diameter)?, efficiency: *efficiency }, read_noise: *read_noise_electrons, magnitude_system: magnitude_system.clone() }
            }
            InstrumentSpec::Instrumental { zero_points, magnitude_system, read_noise_electrons } => Instrument {
                calibration: Calibration::Instrumental { zero_points: zero_points.iter().map(|(k, v)| (k.clone(), *v)).collect() },
                read_noise: *read_noise_electrons,
                magnitude_system: magnitude_system.clone(),
            },
        };
        inst.validate().map_err(|e| e.under(pointer))?;
        Ok(inst)
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub instrument: InstrumentSpec,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let inst = req.instrument.build(ctx, "/instrument")?;
    let mode = match &inst.calibration {
        Calibration::Physical { .. } => "physical",
        Calibration::Instrumental { .. } => "instrumental_zero_point",
    };
    Ok(OpOutput::new(json!({ "valid": true, "calibration_mode": mode, "bands": inst.bands(), "aperture_area_cm2": inst.area_cm2(), "read_noise_electrons": inst.read_noise, "magnitude_system": inst.magnitude_system, "instrument": inst })))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConvertMode {
    /// Magnitudes -> expected counts.
    MagCounts,
    /// Surface brightness (mag/arcsec^2) -> counts per pixel.
    SkyCounts,
    /// Counts -> flux at `zpt`.
    Calibration,
    /// AB magnitudes -> f_nu in erg/s/cm^2/Hz.
    AbFnu,
}

fn zpt27() -> f64 {
    27.0
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertRequest {
    #[serde(default)]
    pub instrument: Option<InstrumentSpec>,
    pub mode: ConvertMode,
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default)]
    pub values: Vec<f64>,
    #[serde(default)]
    pub exposure_s: Option<f64>,
    #[serde(default)]
    pub pixel_scale_arcsec: Option<f64>,
    #[serde(default = "zpt27")]
    pub zpt: f64,
}

pub fn convert(ctx: &Ctx, req: ConvertRequest) -> Result<OpOutput> {
    if req.mode == ConvertMode::AbFnu {
        return Ok(OpOutput::new(json!({ "mode": req.mode, "unit": "erg s^-1 cm^-2 Hz^-1", "values": req.values.iter().map(|m| fnu_from_ab(*m)).collect::<Vec<_>>() })));
    }
    let inst = req.instrument.as_ref().ok_or_else(|| Error::invalid("/instrument", "This conversion needs an instrument."))?.build(ctx, "/instrument")?;
    let band = req.band.as_deref().ok_or_else(|| Error::invalid("/band", "band is required."))?;
    let t = req.exposure_s.ok_or_else(|| Error::invalid("/exposure_s", "exposure_s is required."))?;
    let out = match req.mode {
        ConvertMode::MagCounts => json!({ "unit": "counts", "values": inst.mag_to_counts(&req.values, band, t)? }),
        ConvertMode::SkyCounts => {
            let ps = req.pixel_scale_arcsec.ok_or_else(|| Error::invalid("/pixel_scale_arcsec", "pixel_scale_arcsec is required."))?;
            let v: Vec<f64> = req.values.iter().map(|sb| inst.sb_to_counts_per_pixel(*sb, band, t, ps)).collect::<Result<_>>()?;
            json!({ "unit": "counts/pixel", "values": v, "note": "includes the pixel area (pixel_scale^2) for both calibration modes" })
        }
        ConvertMode::Calibration => json!({ "unit": "flux at zpt per count", "calibration": inst.calibration_factor(band, t, req.zpt)? }),
        ConvertMode::AbFnu => unreachable!(),
    };
    Ok(OpOutput::new(json!({ "mode": req.mode, "band": band, "exposure_s": t, "result": out })))
}
