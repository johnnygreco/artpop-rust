//! Ideal and artificial observations.

use crate::codec::{SourceLoad, image_member_owned, load_source_with, read_image};
use crate::ops::image::{SmoothUnits, load_psf, render_smooth};
use crate::ops::imager::InstrumentSpec;
use crate::ops::source::source_mask;
use crate::store::Member;
use crate::types::{ArtifactInput, Selection};
use crate::{Ctx, OpOutput};
use artpop_core::convolve::{self, Boundary, Method};
use artpop_core::error::{Error, Result};
use artpop_core::image::{Image, InvalidPolicy, deposit};
use artpop_core::imager;
use artpop_core::spatial::Coordinates;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn zpt27() -> f64 {
    27.0
}
fn wrap() -> Boundary {
    Boundary::Wrap
}
fn auto() -> Method {
    Method::Auto
}
fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdealRequest {
    pub source: ArtifactInput,
    pub bandpass: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psf: Option<ArtifactInput>,
    #[serde(default = "zpt27")]
    pub output_zpt: f64,
    #[serde(default = "wrap")]
    pub boundary: Boundary,
    #[serde(default = "auto")]
    pub convolution_method: Method,
    /// Histogram bins (default: the source's coordinate convention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram_convention: Option<Coordinates>,
    /// Discrete stars to include (smooth light is included only with `all`).
    #[serde(default)]
    pub selection: Selection,
    #[serde(default = "yes")]
    pub include_smooth: bool,
}

struct Rendered {
    image: Image,
    stats: Value,
    smooth: Vec<Value>,
    convolution: Value,
    inputs: Vec<String>,
    timings: Timings,
}

/// Wall-clock stage timings reported in results (never stored in artifacts).
#[derive(Default)]
struct Timings(Vec<(&'static str, f64)>, Option<std::time::Instant>);

impl Timings {
    fn start() -> Timings {
        Timings(Vec::new(), Some(std::time::Instant::now()))
    }
    fn lap(&mut self, stage: &'static str) {
        let now = std::time::Instant::now();
        if let Some(t) = self.1 {
            self.0.push((stage, (now - t).as_secs_f64() * 1000.0));
        }
        self.1 = Some(now);
    }
    fn json(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.to_string(), json!((v * 10.0).round() / 10.0))).collect())
    }
}

/// Deposit `weights` of the selected inside stars, add smooth light, convolve.
#[allow(clippy::too_many_arguments)]
fn render(
    ctx: &Ctx,
    s: &crate::codec::SourceData,
    source_id: &str,
    weights: &[f64],
    selection: &Selection,
    convention: Option<Coordinates>,
    smooth_units: Option<&SmoothUnits>,
    band: &str,
    psf: &Option<ArtifactInput>,
    boundary: Boundary,
    method: Method,
) -> Result<Rendered> {
    use rayon::prelude::*;
    let mut timings = Timings::start();
    let sel = source_mask(ctx, s, selection, "/selection")?;
    let mask: Vec<bool> = (0..s.len()).into_par_iter().with_min_len(1 << 15).map(|i| s.inside[i] && sel.as_ref().is_none_or(|m| m[i])).collect();
    let marked_outside = (0..s.len()).into_par_iter().with_min_len(1 << 15).filter(|i| !s.inside[*i] && sel.as_ref().is_none_or(|m| m[*i])).count();
    let conv = convention.unwrap_or(s.geometry.coordinates);
    let (mut img, st) = deposit(conv, s.geometry.width, s.geometry.height, &s.x, &s.y, Some(weights), Some(&mask), InvalidPolicy::Error)?;
    let mut stats = serde_json::to_value(&st).unwrap();
    stats["marked_outside_excluded"] = json!(marked_outside);
    timings.lap("deposit");
    let mut smooth = Vec::new();
    if let Some(units) = smooth_units {
        if selection.is_all() {
            smooth = render_smooth(s, band, None, units, &mut img)?;
        } else if !s.smooth.is_empty() {
            ctx.warn("SMOOTH_EXCLUDED", "Smooth (unresolved) light is excluded for star selections.");
        }
    }
    timings.lap("smooth");
    let mut inputs = vec![source_id.to_string()];
    let mut convolution = Value::Null;
    if let Some(p) = psf {
        let (k, kmeta) = load_psf(ctx, &p.artifact_id, "/psf/artifact_id")?;
        let (out, info) = convolve::convolve(&img, &k, boundary, method, true)?;
        img = out;
        convolution = json!({ "psf_artifact_id": p.artifact_id, "info": info, "psf": kmeta });
        inputs.push(p.artifact_id.clone());
    }
    timings.lap("convolve");
    Ok(Rendered { image: img, stats, smooth, convolution, inputs, timings })
}

pub fn ideal(ctx: &Ctx, req: IdealRequest) -> Result<OpOutput> {
    let t0 = std::time::Instant::now();
    let s = load_source_with(ctx, &req.source.artifact_id, "/source/artifact_id", &SourceLoad::for_render(&req.bandpass, &req.selection))?;
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let zpt = req.output_zpt;
    let flux = artpop_core::mem::par_map_f64(s.band_column(&req.bandpass, "/bandpass")?, |m| 10f64.powf(0.4 * (zpt - m)));
    let units = SmoothUnits::Flux { zpt: req.output_zpt };
    let r = render(ctx, &s, &req.source.artifact_id, &flux, &req.selection, req.histogram_convention, req.include_smooth.then_some(&units), &req.bandpass, &req.psf, req.boundary, req.convolution_method)?;
    let unit = format!("flux (zpt={})", req.output_zpt);
    let meta = json!({
        "kind": "ideal",
        "bandpass": req.bandpass,
        "zpt": req.output_zpt,
        "magnitude_system": s.magnitude_system,
        "unit": unit,
        "width": r.image.width,
        "height": r.image.height,
        "geometry": s.geometry,
        "histogram": r.stats,
        "smooth_components": r.smooth,
        "convolution": r.convolution,
        "boundary": req.boundary,
        "planes": { "image": unit },
    });
    let sum = r.image.sum();
    let (width, height) = (r.image.width, r.image.height);
    let mut timings = r.timings;
    timings.0.insert(0, ("load_source", load_ms));
    let a = ctx.put("observation", meta, vec![image_member_owned("image", r.image, &unit)], r.inputs)?;
    timings.lap("store");
    Ok(OpOutput::new(json!({ "observation_artifact_id": a.artifact_id, "bandpass": req.bandpass, "zpt": req.output_zpt, "width": width, "height": height, "image_sum": sum, "histogram": r.stats, "smooth_components": r.smooth, "timings_ms": timings.json() })).with(a))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtificialRequest {
    pub source: ArtifactInput,
    pub instrument: InstrumentSpec,
    pub bandpass: String,
    pub exposure_s: f64,
    /// Constant sky surface brightness (mag/arcsec^2); omit for no sky.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky_sb_mag_arcsec2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psf: Option<ArtifactInput>,
    #[serde(default = "zpt27")]
    pub output_zpt: f64,
    #[serde(default = "wrap")]
    pub boundary: Boundary,
    #[serde(default = "auto")]
    pub convolution_method: Method,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram_convention: Option<Coordinates>,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default = "yes")]
    pub include_smooth: bool,
    /// Also store ArtPop's `raw_counts + read_noise^2` estimator (may be negative).
    #[serde(default)]
    pub include_observed_variance: bool,
    /// Store ArtPop's per-star magnitude error estimate.
    #[serde(default)]
    pub include_magnitude_errors: bool,
}

pub fn artificial(ctx: &Ctx, req: ArtificialRequest) -> Result<OpOutput> {
    let t0 = std::time::Instant::now();
    let s = load_source_with(ctx, &req.source.artifact_id, "/source/artifact_id", &SourceLoad::for_render(&req.bandpass, &req.selection))?;
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let band_mags = s.band_column(&req.bandpass, "/bandpass")?;
    let inst = req.instrument.build(ctx, "/instrument")?;
    if !inst.magnitude_system.eq_ignore_ascii_case(&s.magnitude_system) && s.magnitude_system != "unspecified" {
        return Err(Error::new(artpop_core::ErrorCode::UnitMismatch, format!("Source magnitudes are {} but the instrument expects {}.", s.magnitude_system, inst.magnitude_system)).at("/instrument/magnitude_system"));
    }
    artpop_core::error::require_positive(req.exposure_s, "/exposure_s", "Exposure time")?;
    let seed = ctx.seed()?;
    let counts = inst.mag_to_counts(band_mags, &req.bandpass, req.exposure_s)?;
    let units = SmoothUnits::Counts { instrument: &inst, exposure_s: req.exposure_s, zpt: req.output_zpt };
    let r = render(ctx, &s, &req.source.artifact_id, &counts, &req.selection, req.histogram_convention, req.include_smooth.then_some(&units), &req.bandpass, &req.psf, req.boundary, req.convolution_method)?;
    let mut expected = r.image;
    let mut timings = r.timings;
    timings.0.insert(0, ("load_source", load_ms));
    let clamped = imager::sanitize_expected(&mut expected)?;
    let sky = match req.sky_sb_mag_arcsec2 {
        Some(sb) => inst.sb_to_counts_per_pixel(sb, &req.bandpass, req.exposure_s, s.geometry.pixel_scale("/source/geometry")?)?,
        None => 0.0,
    };
    let raw = imager::realize_counts(&expected, sky, inst.read_noise, seed, &req.bandpass)?;
    timings.lap("noise");
    let cal = inst.calibration_factor(&req.bandpass, req.exposure_s, req.output_zpt)?;
    let rn2 = inst.read_noise * inst.read_noise;
    let plane = |src: &Image, f: &(dyn Fn(f64) -> f64 + Sync)| Image { width: src.width, height: src.height, data: artpop_core::mem::par_map_f64(&src.data, f) };
    let image = plane(&raw, &|v| (v - sky) * cal);
    let var_counts = plane(&expected, &|v| v + sky + rn2);
    let var_cal = plane(&var_counts, &|v| v * cal * cal);
    let unit = format!("flux (zpt={})", req.output_zpt);
    let (expected_sum, raw_sum) = (expected.sum(), raw.sum());
    let (width, height) = (expected.width, expected.height);
    let mut planes = json!({ "image": unit, "source_counts": "counts", "raw_counts": "counts", "expected_variance_counts": "counts^2", "expected_variance_calibrated": format!("({unit})^2") });
    let mut extra = Vec::new();
    if req.include_observed_variance {
        extra.push(image_member_owned("observed_variance_estimator", plane(&raw, &|v| v + rn2), "counts^2"));
        planes["observed_variance_estimator"] = json!("counts^2 (ArtPop var_image; not guaranteed positive)");
    }
    if req.include_magnitude_errors {
        let e = imager::magnitude_errors(&counts, sky, inst.read_noise);
        extra.push(Member::column("star_mag_error", &e, Some("mag")));
        planes["star_mag_error"] = json!("mag per source star (ArtPop estimate; no PSF fitting/confusion)");
    }
    let mut members = vec![
        image_member_owned("image", image, &unit),
        image_member_owned("source_counts", expected, "counts"),
        image_member_owned("raw_counts", raw, "counts"),
        image_member_owned("expected_variance_counts", var_counts, "counts^2"),
        image_member_owned("expected_variance_calibrated", var_cal, &format!("({unit})^2")),
    ];
    members.extend(extra);
    let sky_mean = sky;
    let meta = json!({
        "kind": "artificial",
        "bandpass": req.bandpass,
        "zpt": req.output_zpt,
        "magnitude_system": s.magnitude_system,
        "unit": unit,
        "exposure_s": req.exposure_s,
        "sky_sb_mag_arcsec2": req.sky_sb_mag_arcsec2,
        "sky_counts": sky,
        "calibration": cal,
        "read_noise": inst.read_noise,
        "instrument": inst,
        "width": width,
        "height": height,
        "geometry": s.geometry,
        "histogram": r.stats,
        "smooth_components": r.smooth,
        "convolution": r.convolution,
        "boundary": req.boundary,
        "negative_residuals_clamped": clamped,
        "noise": { "poisson": artpop_core::rng::POISSON_ALGORITHM, "normal": artpop_core::rng::NORMAL_ALGORITHM, "blocks": artpop_core::rng::BLOCK_ALGORITHM, "streams": ["observe.shot_noise/<band>/block<k>", "observe.read_noise/<band>/block<k>"] },
        "planes": planes,
    });
    timings.lap("planes");
    let a = ctx.put("observation", meta, members, r.inputs)?;
    timings.lap("store");
    Ok(OpOutput::new(json!({
        "observation_artifact_id": a.artifact_id,
        "bandpass": req.bandpass,
        "zpt": req.output_zpt,
        "exposure_s": req.exposure_s,
        "sky_counts": sky_mean,
        "calibration": cal,
        "read_noise": inst.read_noise,
        "width": width,
        "height": height,
        "expected_source_counts": expected_sum,
        "raw_counts_sum": raw_sum,
        "histogram": r.stats,
        "smooth_components": r.smooth,
        "planes": planes,
        "timings_ms": timings.json(),
    }))
    .with(a))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cutout {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub artifact_id: String,
    /// Metadata fields to return (default: all scalar metadata).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Plane for statistics/cutout.
    #[serde(default)]
    pub plane: Option<String>,
    #[serde(default)]
    pub statistics: bool,
    /// Bounded cutout (at most 10,000 pixels).
    #[serde(default)]
    pub cutout: Option<Cutout>,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let l = ctx.store().load(&req.artifact_id, "/artifact_id", &["observation"])?;
    let mut all = l.manifest.metadata.clone();
    if let Value::Object(m) = &mut all {
        m.insert("planes_available".into(), json!(l.manifest.members.iter().map(|x| &x.name).collect::<Vec<_>>()));
    }
    let mut out = crate::ops::pick(all, &req.fields, "/fields")?;
    if req.statistics || req.cutout.is_some() {
        let plane = req.plane.clone().unwrap_or_else(|| "image".into());
        let img = read_image(&l, &plane).map_err(|e| e.at("/plane"))?;
        if req.statistics {
            let (min, max) = img.min_max();
            let n = img.data.len() as f64;
            let mean = img.sum() / n;
            let var = img.data.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
            out[format!("statistics:{plane}")] = json!({ "min": min, "max": max, "sum": img.sum(), "mean": mean, "std": var.sqrt(), "negative_pixels": img.data.iter().filter(|v| **v < 0.0).count() });
        }
        if let Some(c) = &req.cutout {
            if c.width * c.height > 10_000 || c.x0 + c.width > img.width || c.y0 + c.height > img.height || c.width == 0 || c.height == 0 {
                return Err(Error::invalid("/cutout", "Cutouts must lie inside the image and contain at most 10,000 pixels."));
            }
            let rows: Vec<Vec<f64>> = (c.y0..c.y0 + c.height).map(|y| (c.x0..c.x0 + c.width).map(|x| img.get(x, y)).collect()).collect();
            out["cutout"] = json!({ "plane": plane, "x0": c.x0, "y0": c.y0, "values_yx": rows });
        }
    }
    Ok(OpOutput::new(out))
}
