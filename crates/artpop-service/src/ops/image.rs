//! Image construction and manipulation: histograms, smooth light,
//! convolution, embedding, arithmetic, previews, and plots.

use crate::codec::{SmoothProfile, SourceData, image_member, load_isochrone, load_population, load_source, read_image};
use crate::ops::population::{MagFrame, population_mask, population_numeric};
use crate::ops::source::{smooth_amplitude, source_mask};
use crate::store::Member;
use crate::types::{ArtifactInput, CatalogInput, Geometry, Selection};
use crate::{Ctx, OpOutput};
use artpop_core::convolve::{self, Boundary, Method};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::{DepositStats, Image, InvalidPolicy, deposit};
use artpop_core::imager::Instrument;
use artpop_core::spatial::Coordinates;
use artpop_io::preview::{Colormap, DisplayOptions, Origin, Series, Stretch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// An image-like artifact plane.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageRef {
    pub artifact_id: String,
    /// Observation plane (default `image`); ignored for image/psf/grid artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<String>,
}

pub struct LoadedImage {
    pub image: Image,
    pub meta: Value,
    pub kind: String,
    pub unit: Option<String>,
}

pub fn load_image_ref(ctx: &Ctx, r: &ImageRef, pointer: &str) -> Result<LoadedImage> {
    let l = ctx.store().load(&r.artifact_id, &format!("{pointer}/artifact_id"), &["image", "observation", "psf", "spatial_grid"])?;
    let member = match l.manifest.kind.as_str() {
        "observation" => r.plane.clone().unwrap_or_else(|| "image".into()),
        "psf" => "kernel".into(),
        "spatial_grid" => "weights".into(),
        _ => "image".into(),
    };
    if !l.has(&member) {
        return Err(Error::invalid(&format!("{pointer}/plane"), format!("Artifact has no plane '{member}'.")).with_details(json!({ "available": l.manifest.members.iter().map(|m| &m.name).collect::<Vec<_>>() })));
    }
    let unit = l.info(&member)?.unit.clone();
    Ok(LoadedImage { image: read_image(&l, &member)?, meta: l.manifest.metadata.clone(), kind: l.manifest.kind.clone(), unit })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WeightSpec {
    /// Count map (each star adds 1).
    #[default]
    Unit,
    /// A numeric catalog column.
    Column { column: String },
    /// Flux `10^(0.4 (zpt - mag))` from a band.
    MagnitudeFlux {
        band: String,
        #[serde(default = "zpt27")]
        zpt: f64,
    },
}

fn zpt27() -> f64 {
    27.0
}
fn err_invalid() -> InvalidPolicy {
    InvalidPolicy::Error
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistogramRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ArtifactInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positions: Option<ArtifactInput>,
    /// Required for catalogs; defaults to the source/positions geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    #[serde(default)]
    pub weights: WeightSpec,
    /// Bin convention (default: the geometry's coordinate convention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convention: Option<Coordinates>,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default = "err_invalid")]
    pub invalid: InvalidPolicy,
}

/// x, y, weights, selection mask, geometry, unit, input artifact IDs.
type HistogramInputs = (Vec<f64>, Vec<f64>, Option<Vec<f64>>, Option<Vec<bool>>, Geometry, String, Vec<String>);

fn stats_json(st: &DepositStats) -> Value {
    serde_json::to_value(st).unwrap()
}

pub fn histogram(ctx: &Ctx, req: HistogramRequest) -> Result<OpOutput> {
    let given = [req.catalog.is_some(), req.source.is_some(), req.positions.is_some()].iter().filter(|b| **b).count();
    if given != 1 {
        return Err(Error::invalid("", "Give exactly one of catalog, source, or positions."));
    }
    let (xs, ys, weights, mask, geometry, unit, inputs): HistogramInputs;
    if let Some(cat) = &req.catalog {
        let g = req.geometry.clone().ok_or_else(|| Error::invalid("/geometry", "A catalog histogram needs geometry."))?;
        let text = ctx.read_text(&cat.path, "/catalog/path")?;
        let t = artpop_io::csv::parse(&text, crate::types::delimiter(&cat.delimiter, "/catalog/delimiter")?).map_err(|e| e.under("/catalog"))?;
        xs = t.require(&cat.x_column, "/catalog/x_column")?.to_vec();
        ys = t.require(&cat.y_column, "/catalog/y_column")?.to_vec();
        let (w, u) = match &req.weights {
            WeightSpec::Unit => (None, "count".to_string()),
            WeightSpec::Column { column } => (Some(t.require(column, "/weights/column")?.to_vec()), column.clone()),
            WeightSpec::MagnitudeFlux { band, zpt } => {
                let col = cat.magnitude_columns.get(band).ok_or_else(|| Error::unknown_filter("/weights/band", band, &cat.magnitude_columns.keys().cloned().collect::<Vec<_>>()))?;
                (Some(t.require(col, "/weights/band")?.iter().map(|m| 10f64.powf(0.4 * (zpt - m))).collect()), format!("flux (zpt={zpt})"))
            }
        };
        if !req.selection.is_all() {
            return Err(Error::invalid("/selection", "Selections apply to source inputs; filter the catalog instead."));
        }
        (weights, mask, geometry, unit, inputs) = (w, None, g, u, vec![]);
    } else if let Some(src) = &req.source {
        let s = load_source(ctx, &src.artifact_id, "/source/artifact_id")?;
        let sel = source_mask(ctx, &s, &req.selection, "/selection")?;
        let m: Vec<bool> = (0..s.len()).map(|i| s.inside[i] && sel.as_ref().is_none_or(|v| v[i])).collect();
        let (w, u) = match &req.weights {
            WeightSpec::Unit => (None, "count".to_string()),
            WeightSpec::MagnitudeFlux { band, zpt } => {
                let b = s.band_index(band).map_err(|e| e.at("/weights/band"))?;
                (Some(s.mags[b].iter().map(|m| 10f64.powf(0.4 * (zpt - m))).collect()), format!("flux (zpt={zpt})"))
            }
            WeightSpec::Column { .. } => return Err(Error::invalid("/weights", "Column weights apply to catalogs; use magnitude_flux for sources.")),
        };
        xs = s.x.clone();
        ys = s.y.clone();
        (weights, mask, geometry, unit, inputs) = (w, Some(m), req.geometry.clone().unwrap_or(s.geometry.clone()), u, vec![src.artifact_id.clone()]);
    } else {
        let p = req.positions.as_ref().unwrap();
        let (pos, g) = crate::codec::load_positions(ctx, &p.artifact_id, "/positions/artifact_id")?;
        if !matches!(req.weights, WeightSpec::Unit) {
            return Err(Error::invalid("/weights", "Positions carry no magnitudes; use unit weights."));
        }
        xs = pos.x;
        ys = pos.y;
        (weights, mask, geometry, unit, inputs) = (None, Some(pos.inside), req.geometry.clone().unwrap_or(g), "count".into(), vec![p.artifact_id.clone()]);
    }
    geometry.validate("/geometry", ctx.config().limits.max_image_pixels)?;
    let conv = req.convention.unwrap_or(geometry.coordinates);
    let (img, st) = deposit(conv, geometry.width, geometry.height, &xs, &ys, weights.as_deref(), mask.as_deref(), req.invalid)?;
    let mut diagnostics = stats_json(&st);
    // Stars excluded before binning (outside-marked or unselected source stars).
    diagnostics["excluded_by_mask"] = json!(mask.as_ref().map_or(0, |m| m.iter().filter(|v| !**v).count()));
    let meta = json!({ "width": geometry.width, "height": geometry.height, "unit": unit, "convention": conv, "geometry": geometry, "diagnostics": diagnostics, "sum": img.sum() });
    let r = ctx.put("image", meta, vec![image_member("image", &img, &unit)], inputs)?;
    Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "width": geometry.width, "height": geometry.height, "unit": unit, "convention": conv, "diagnostics": diagnostics, "sum": img.sum() })).with(r))
}

pub enum SmoothUnits<'a> {
    Flux { zpt: f64 },
    Counts { instrument: &'a Instrument, exposure_s: f64, zpt: f64 },
}

/// Add a source's smooth components in `band` to `image`; returns diagnostics.
pub fn render_smooth(s: &SourceData, band: &str, components: Option<&[u32]>, units: &SmoothUnits, image: &mut Image) -> Result<Vec<Value>> {
    let mut diag = Vec::new();
    if s.smooth.is_empty() {
        return Ok(diag);
    }
    let ps = s.geometry.pixel_scale("/source/geometry")?;
    let frame = (s.geometry.width * s.geometry.height) as f64;
    for c in &s.smooth {
        if components.is_some_and(|ids| !ids.contains(&c.component_id)) {
            continue;
        }
        let Some(Some(m)) = c.apparent_mag.get(band).copied() else {
            diag.push(json!({ "component_id": c.component_id, "status": "no_flux_in_band" }));
            continue;
        };
        let (sb, amp_flux) = smooth_amplitude(&c.profile, m, match units { SmoothUnits::Flux { zpt } | SmoothUnits::Counts { zpt, .. } => *zpt }, ps, frame)?;
        let (amp, intrinsic) = match units {
            SmoothUnits::Flux { zpt } => (amp_flux, 10f64.powf(0.4 * (zpt - m))),
            SmoothUnits::Counts { instrument, exposure_s, .. } => (instrument.sb_to_counts_per_pixel(sb, band, *exposure_s, ps)?, instrument.mag_to_counts(&[m], band, *exposure_s)?[0]),
        };
        let model = c.shape.scaled(amp);
        let prepared = model.prepare()?;
        let mut captured = 0.0;
        for y in 0..image.height {
            for x in 0..image.width {
                let v = prepared.eval(x as f64, y as f64);
                image.data[y * image.width + x] += v;
                captured += v;
            }
        }
        let profile_name = match c.profile {
            SmoothProfile::Sersic { .. } => "sersic",
            SmoothProfile::Plummer { .. } => "plummer",
            SmoothProfile::Constant => "constant",
        };
        diag.push(json!({ "component_id": c.component_id, "profile": profile_name, "total_mag": m, "surface_brightness_mag_arcsec2": sb, "amplitude": amp, "intrinsic_total": intrinsic, "captured_in_frame": captured }));
    }
    Ok(diag)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SmoothRequest {
    pub source: ArtifactInput,
    pub bandpass: String,
    /// Output zero point (flux units) when no instrument is given.
    #[serde(default = "zpt27")]
    pub zpt: f64,
    /// Render expected counts instead of flux.
    #[serde(default)]
    pub instrument: Option<crate::ops::imager::InstrumentSpec>,
    #[serde(default)]
    pub exposure_s: Option<f64>,
    #[serde(default)]
    pub components: Option<Vec<u32>>,
}

pub fn smooth(ctx: &Ctx, req: SmoothRequest) -> Result<OpOutput> {
    let s = load_source(ctx, &req.source.artifact_id, "/source/artifact_id")?;
    s.band_index(&req.bandpass)?;
    let mut img = Image::zeros(s.geometry.width, s.geometry.height);
    let inst = req.instrument.as_ref().map(|i| i.build(ctx, "/instrument")).transpose()?;
    let (units, unit) = match &inst {
        Some(i) => (SmoothUnits::Counts { instrument: i, exposure_s: req.exposure_s.ok_or_else(|| Error::invalid("/exposure_s", "Counts need exposure_s."))?, zpt: req.zpt }, "counts".to_string()),
        None => (SmoothUnits::Flux { zpt: req.zpt }, format!("flux (zpt={})", req.zpt)),
    };
    let diag = render_smooth(&s, &req.bandpass, req.components.as_deref(), &units, &mut img)?;
    if diag.is_empty() {
        ctx.warn("NO_SMOOTH_LIGHT", "The source has no smooth components; the image is zero.");
    }
    let meta = json!({ "width": img.width, "height": img.height, "unit": unit, "bandpass": req.bandpass, "zpt": req.zpt, "smooth_components": diag, "geometry": s.geometry });
    let r = ctx.put("image", meta, vec![image_member("image", &img, &unit)], vec![req.source.artifact_id.clone()])?;
    Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "unit": unit, "smooth_components": diag, "sum": img.sum() })).with(r))
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
pub struct ConvolveRequest {
    pub image: ImageRef,
    pub psf: ArtifactInput,
    /// `wrap` (ArtPop operational default) or `fill`.
    #[serde(default = "wrap")]
    pub boundary: Boundary,
    #[serde(default = "auto")]
    pub method: Method,
    #[serde(default = "yes")]
    pub normalize_kernel: bool,
}

pub fn load_psf(ctx: &Ctx, id: &str, pointer: &str) -> Result<(Image, Value)> {
    let l = ctx.store().load(id, pointer, &["psf"])?;
    Ok((read_image(&l, "kernel")?, l.manifest.metadata.clone()))
}

pub fn convolve(ctx: &Ctx, req: ConvolveRequest) -> Result<OpOutput> {
    let src = load_image_ref(ctx, &req.image, "/image")?;
    let (k, _) = load_psf(ctx, &req.psf.artifact_id, "/psf/artifact_id")?;
    let (out, info) = convolve::convolve(&src.image, &k, req.boundary, req.method, req.normalize_kernel)?;
    let unit = src.unit.unwrap_or_else(|| "unknown".into());
    let mut meta = src.meta.clone();
    if let Value::Object(m) = &mut meta {
        m.insert("convolution".into(), serde_json::to_value(&info).unwrap());
        m.insert("psf_artifact_id".into(), json!(req.psf.artifact_id));
        m.insert("unit".into(), json!(unit));
        m.insert("width".into(), json!(out.width));
        m.insert("height".into(), json!(out.height));
    }
    let r = ctx.put("image", meta, vec![image_member("image", &out, &unit)], vec![req.image.artifact_id.clone(), req.psf.artifact_id.clone()])?;
    Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "convolution": info })).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitsImageInput {
    pub path: String,
    #[serde(default)]
    pub hdu: Option<usize>,
    #[serde(default)]
    pub extname: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    #[serde(default)]
    pub image: Option<ImageRef>,
    #[serde(default)]
    pub fits: Option<FitsImageInput>,
    /// Empty destination `[width, height]`.
    #[serde(default)]
    pub shape: Option<[usize; 2]>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbedMode {
    #[default]
    Add,
    Replace,
    SlicesOnly,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbedRequest {
    pub destination: Destination,
    pub source: ImageRef,
    /// Destination pixel `[x, y]` receiving the source's center pixel `(w/2, h/2)`.
    pub center: [i64; 2],
    #[serde(default)]
    pub mode: EmbedMode,
}

pub fn embed(ctx: &Ctx, req: EmbedRequest) -> Result<OpOutput> {
    let src = load_image_ref(ctx, &req.source, "/source")?;
    let (mut dst, cards, dst_meta, inputs) = match (&req.destination.image, &req.destination.fits, &req.destination.shape) {
        (Some(r), None, None) => {
            let d = load_image_ref(ctx, r, "/destination/image")?;
            let cards = d.meta.get("fits_cards").cloned().unwrap_or(json!([]));
            (d.image, cards, json!({ "artifact_id": r.artifact_id }), vec![r.artifact_id.clone(), req.source.artifact_id.clone()])
        }
        (None, Some(f), None) => {
            let p = ctx.read_path(&f.path, "/destination/fits/path")?;
            let r = artpop_io::fits::read_image(&p, f.hdu, f.extname.as_deref()).map_err(|e| e.at("/destination/fits"))?;
            (r.image, json!(r.cards), json!({ "fits": p, "hdu": r.hdu_index }), vec![req.source.artifact_id.clone()])
        }
        (None, None, Some([w, h])) => (Image::zeros(*w, *h), json!([]), json!({ "shape": [w, h] }), vec![req.source.artifact_id.clone()]),
        _ => return Err(Error::invalid("/destination", "Give exactly one of image, fits, or shape.")),
    };
    let ov = artpop_core::image::overlap((dst.width, dst.height), (src.image.width, src.image.height), (req.center[0], req.center[1]));
    let total = src.image.sum();
    let Some((dx0, dy0, sx0, sy0, nx, ny)) = ov else {
        if req.mode == EmbedMode::SlicesOnly {
            return Ok(OpOutput::new(json!({ "overlap": null, "lost_fraction": 1.0 })));
        }
        return Err(Error::new(ErrorCode::OutOfDomain, "The source image does not overlap the destination.").at("/center"));
    };
    let slices = json!({ "destination": { "x": [dx0, dx0 + nx], "y": [dy0, dy0 + ny] }, "source": { "x": [sx0, sx0 + nx], "y": [sy0, sy0 + ny] } });
    let mut kept = 0.0;
    for j in 0..ny {
        for i in 0..nx {
            kept += src.image.get(sx0 + i, sy0 + j);
        }
    }
    let lost = if total != 0.0 { 1.0 - kept / total } else { 0.0 };
    if req.mode == EmbedMode::SlicesOnly {
        return Ok(OpOutput::new(json!({ "overlap": slices, "lost_fraction": lost })));
    }
    for j in 0..ny {
        for i in 0..nx {
            let v = src.image.get(sx0 + i, sy0 + j);
            let d = &mut dst.data[(dy0 + j) * dst.width + dx0 + i];
            *d = if req.mode == EmbedMode::Add { *d + v } else { v };
        }
    }
    if lost > 0.0 {
        ctx.warn("EMBED_CLIPPED", format!("{:.3}% of the source signal fell outside the destination.", lost * 100.0));
    }
    let meta = json!({ "width": dst.width, "height": dst.height, "unit": src.unit, "embed": { "center": req.center, "mode": req.mode, "overlap": slices, "lost_fraction": lost, "destination": dst_meta }, "fits_cards": cards, "wcs_note": "no reprojection performed" });
    let unit = src.unit.clone().unwrap_or_else(|| "unknown".into());
    let r = ctx.put("image", meta, vec![image_member("image", &dst, &unit)], inputs)?;
    Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "overlap": slices, "lost_fraction": lost })).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CombineTerm {
    pub artifact_id: String,
    #[serde(default)]
    pub plane: Option<String>,
    #[serde(default = "one")]
    pub coefficient: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CombineRequest {
    pub images: Vec<CombineTerm>,
    /// Permit different units/zero points/bands (generic arithmetic).
    #[serde(default)]
    pub allow_mixed_units: bool,
}

pub fn combine(ctx: &Ctx, req: CombineRequest) -> Result<OpOutput> {
    if req.images.is_empty() {
        return Err(Error::invalid("/images", "Give at least one image."));
    }
    let mut out: Option<Image> = None;
    let mut unit: Option<String> = None;
    let mut ident: Option<(Value, Value)> = None;
    for (i, t) in req.images.iter().enumerate() {
        let p = format!("/images/{i}");
        if !t.coefficient.is_finite() {
            return Err(Error::nonfinite(&format!("{p}/coefficient"), "Coefficients must be finite."));
        }
        let im = load_image_ref(ctx, &ImageRef { artifact_id: t.artifact_id.clone(), plane: t.plane.clone() }, &p)?;
        let id = (im.meta.get("bandpass").cloned().unwrap_or(Value::Null), im.meta.get("zpt").cloned().unwrap_or(Value::Null));
        if !req.allow_mixed_units && (unit.as_ref().is_some_and(|u| Some(u) != im.unit.as_ref()) || ident.as_ref().is_some_and(|x| *x != id)) {
            return Err(Error::new(ErrorCode::UnitMismatch, "Images differ in unit, band, or zero point; convert them first or set allow_mixed_units.").at(&p));
        }
        unit = unit.or(im.unit.clone());
        ident = ident.or(Some(id));
        match &mut out {
            None => out = Some(Image { data: im.image.data.iter().map(|v| v * t.coefficient).collect(), ..im.image }),
            Some(o) => {
                if o.width != im.image.width || o.height != im.image.height {
                    return Err(Error::new(ErrorCode::ShapeMismatch, "Images must share a shape.").at(&p));
                }
                for (a, b) in o.data.iter_mut().zip(&im.image.data) {
                    *a += t.coefficient * b;
                }
            }
        }
    }
    let img = out.unwrap();
    let unit = unit.unwrap_or_else(|| "unknown".into());
    let meta = json!({ "width": img.width, "height": img.height, "unit": unit, "combination": req.images.iter().map(|t| json!({ "artifact_id": t.artifact_id, "plane": t.plane, "coefficient": t.coefficient })).collect::<Vec<_>>(), "bandpass": ident.as_ref().map(|x| x.0.clone()), "zpt": ident.as_ref().map(|x| x.1.clone()) });
    let r = ctx.put("image", meta, vec![image_member("image", &img, &unit)], req.images.iter().map(|t| t.artifact_id.clone()).collect())?;
    Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "sum": img.sum() })).with(r))
}

fn p_lo() -> f64 {
    0.1
}
fn p_hi() -> f64 {
    99.9
}
fn linear() -> Stretch {
    Stretch::Linear
}
fn gray_r() -> Colormap {
    Colormap::GrayR
}
fn lower() -> Origin {
    Origin::Lower
}
fn max1024() -> usize {
    1024
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    pub image: ImageRef,
    /// ArtPop `show_image` defaults: [0.1, 99.9].
    #[serde(default = "p_lo")]
    pub percentile_low: f64,
    #[serde(default = "p_hi")]
    pub percentile_high: f64,
    #[serde(default = "linear")]
    pub stretch: Stretch,
    #[serde(default = "gray_r")]
    pub colormap: Colormap,
    #[serde(default = "lower")]
    pub origin: Origin,
    #[serde(default = "max1024")]
    pub max_size: usize,
}

pub fn preview(ctx: &Ctx, req: PreviewRequest) -> Result<OpOutput> {
    let im = load_image_ref(ctx, &req.image, "/image")?;
    let opts = DisplayOptions { percentile_low: req.percentile_low, percentile_high: req.percentile_high, stretch: req.stretch, colormap: req.colormap, origin: req.origin, max_size: req.max_size.clamp(16, 8192) };
    let (png, info) = artpop_io::preview::render_png(&im.image, &opts)?;
    let meta = json!({ "source": req.image, "display": opts, "info": info, "media_type": "image/png" });
    let r = ctx.put("preview", meta, vec![Member::file("png", png, "image/png")], vec![req.image.artifact_id.clone()])?;
    Ok(OpOutput::new(json!({ "preview_artifact_id": r.artifact_id, "width": info.width, "height": info.height, "block_factor": info.block_factor, "vmin": info.vmin, "vmax": info.vmax, "resource": format!("artpop://artifacts/{}/preview", r.artifact_id) })).with(r))
}

fn q8() -> f64 {
    8.0
}
fn stretch5() -> f64 {
    5.0
}
fn ones3() -> [f64; 3] {
    [1.0, 1.0, 1.0]
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RgbRequest {
    pub red: ImageRef,
    pub green: ImageRef,
    pub blue: ImageRef,
    #[serde(default = "ones3")]
    pub scales: [f64; 3],
    /// Lupton et al. (2004) asinh softening (Astropy make_lupton_rgb defaults Q=8, stretch=5).
    #[serde(default = "q8")]
    pub q: f64,
    #[serde(default = "stretch5")]
    pub stretch: f64,
    #[serde(default)]
    pub minimum: f64,
    #[serde(default = "lower")]
    pub origin: Origin,
    #[serde(default = "max1024")]
    pub max_size: usize,
}

pub fn rgb(ctx: &Ctx, req: RgbRequest) -> Result<OpOutput> {
    let r = load_image_ref(ctx, &req.red, "/red")?;
    let g = load_image_ref(ctx, &req.green, "/green")?;
    let b = load_image_ref(ctx, &req.blue, "/blue")?;
    if !(req.q > 0.0 && req.stretch > 0.0) {
        return Err(Error::invalid("/q", "q and stretch must be positive."));
    }
    let (png, info) = artpop_io::preview::render_rgb([&r.image, &g.image, &b.image], req.scales, req.q, req.stretch, req.minimum, req.origin, req.max_size.clamp(16, 8192))?;
    let meta = json!({ "channels": [req.red, req.green, req.blue], "scales": req.scales, "q": req.q, "stretch": req.stretch, "minimum": req.minimum, "info": info, "media_type": "image/png" });
    let a = ctx.put("preview", meta, vec![Member::file("png", png, "image/png")], vec![req.red.artifact_id.clone(), req.green.artifact_id.clone(), req.blue.artifact_id.clone()])?;
    Ok(OpOutput::new(json!({ "preview_artifact_id": a.artifact_id, "width": info.width, "height": info.height })).with(a))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlotKind {
    /// Color (blue - red) vs magnitude for a population or isochrone.
    Cmd,
    /// log Teff vs log L.
    Hr,
    /// x vs y positions of a source.
    Spatial,
    /// IMF dN/dm on log axes.
    Imf,
}

fn max_points() -> usize {
    100_000
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlotRequest {
    pub kind: PlotKind,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub blue: Option<String>,
    #[serde(default)]
    pub red: Option<String>,
    /// Magnitude band on the y axis (CMD).
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default)]
    pub magnitude_frame: MagFrame,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default)]
    pub imf: crate::types::ImfSpec,
    #[serde(default = "max_points")]
    pub max_points: usize,
    #[serde(default)]
    pub title: Option<String>,
}

fn thin(v: Vec<f64>, max: usize) -> Vec<f64> {
    if v.len() <= max || max == 0 {
        return v;
    }
    let step = v.len().div_ceil(max);
    v.into_iter().step_by(step).collect()
}

pub fn plot(ctx: &Ctx, req: PlotRequest) -> Result<OpOutput> {
    let need = |o: &Option<String>, p: &str| o.clone().ok_or_else(|| Error::invalid(p, "This plot kind needs this field."));
    let title = req.title.clone().unwrap_or_default();
    let (svg, data_ref, desc) = match req.kind {
        PlotKind::Imf => {
            let imf = req.imf.build()?;
            let m: Vec<f64> = (0..400).map(|i| 10f64.powf(-2.0 + 4.1 * i as f64 / 399.0)).collect();
            let y: Vec<f64> = m.iter().map(|x| imf.raw(*x)).collect();
            let s = artpop_io::preview::render_svg(&[Series { x: &m, y: &y, label: imf.name.clone(), color: "#1f77b4", line: true }], "initial mass [Msun]", "dN/dm (raw)", &title, false, true, true)?;
            (s, Value::Null, "imf")
        }
        PlotKind::Spatial => {
            let id = need(&req.artifact_id, "/artifact_id")?;
            let s = load_source(ctx, &id, "/artifact_id")?;
            let mask = source_mask(ctx, &s, &req.selection, "/selection")?;
            let pick = |v: &[f64]| -> Vec<f64> { thin(v.iter().enumerate().filter(|(i, _)| mask.as_ref().is_none_or(|m| m[*i])).map(|(_, x)| *x).collect(), req.max_points) };
            let (x, y) = (pick(&s.x), pick(&s.y));
            let svg = artpop_io::preview::render_svg(&[Series { x: &x, y: &y, label: "stars".into(), color: "#222222", line: false }], "x [pixel]", "y [pixel]", &title, false, false, false)?;
            (svg, json!(id), "spatial")
        }
        PlotKind::Cmd | PlotKind::Hr => {
            let id = need(&req.artifact_id, "/artifact_id")?;
            let kind = ctx.store().load(&id, "/artifact_id", &["population", "isochrone"])?.manifest.kind;
            let (x, y, xl, yl, invert) = if kind == "population" {
                let p = load_population(ctx, &id, "/artifact_id")?;
                let mask = population_mask(ctx, &p, &req.selection, "/selection")?;
                let keep = |v: Vec<f64>| -> Vec<f64> { thin(v.into_iter().enumerate().filter(|(i, _)| mask.as_ref().is_none_or(|m| m[*i])).map(|(_, x)| x).collect(), req.max_points) };
                if req.kind == PlotKind::Cmd {
                    let (b, r) = (need(&req.blue, "/blue")?, need(&req.red, "/red")?);
                    let band = req.band.clone().unwrap_or_else(|| r.clone());
                    let bb = population_numeric(&p, &b, req.magnitude_frame, "/blue")?;
                    let rr = population_numeric(&p, &r, req.magnitude_frame, "/red")?;
                    let mm = population_numeric(&p, &band, req.magnitude_frame, "/band")?;
                    (keep(bb.iter().zip(&rr).map(|(a, c)| a - c).collect()), keep(mm), format!("{b} - {r}"), band, true)
                } else {
                    (keep(population_numeric(&p, "log_Teff", req.magnitude_frame, "/artifact_id")?), keep(population_numeric(&p, "log_L", req.magnitude_frame, "/artifact_id")?), "log Teff".into(), "log L".into(), false)
                }
            } else {
                let iso = load_isochrone(ctx, &id, "/artifact_id")?;
                let col = |n: &str, p: &str| iso.column(n).map(|c| c.to_vec()).ok_or_else(|| Error::invalid(p, format!("Isochrone has no column '{n}'.")));
                if req.kind == PlotKind::Cmd {
                    let (b, r) = (need(&req.blue, "/blue")?, need(&req.red, "/red")?);
                    let band = req.band.clone().unwrap_or_else(|| r.clone());
                    let (bb, rr, mm) = (col(&b, "/blue")?, col(&r, "/red")?, col(&band, "/band")?);
                    (bb.iter().zip(&rr).map(|(a, c)| a - c).collect(), mm, format!("{b} - {r}"), band, true)
                } else {
                    (col("log_Teff", "/artifact_id")?, col("log_L", "/artifact_id")?, "log Teff".into(), "log L".into(), false)
                }
            };
            let line = kind == "isochrone";
            let svg = artpop_io::preview::render_svg(&[Series { x: &x, y: &y, label: kind.clone(), color: "#1f77b4", line }], &xl, &yl, &title, invert, false, false)?;
            // HR diagrams conventionally plot Teff decreasing to the right.
            let svg = if req.kind == PlotKind::Hr { svg.replacen("<svg ", "<svg data-x-axis=\"log Teff\" ", 1) } else { svg };
            (svg, json!(id), if req.kind == PlotKind::Cmd { "cmd" } else { "hr" })
        }
    };
    let meta = json!({ "kind": desc, "data": data_ref, "media_type": "image/svg+xml", "max_points": req.max_points });
    let r = ctx.put("plot", meta, vec![Member::file("svg", svg.into_bytes(), "image/svg+xml")], data_ref.as_str().map(|s| vec![s.to_string()]).unwrap_or_default())?;
    Ok(OpOutput::new(json!({ "plot_artifact_id": r.artifact_id, "kind": desc })).with(r))
}
