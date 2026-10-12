//! Sources: positioned stars plus smooth (unresolved) light components.

use crate::codec::{Labels, SmoothComponent, SmoothProfile, SourceData, label_json, load_population, load_positions, load_source, source_bundle};
use crate::ops::population::{BuildRequest as PopBuild, IsochroneInput, SamplingOptions, build_population, put_population};
use crate::ops::spatial::load_grid;
use crate::store::Member;
use crate::types::{ArtifactInput, CatalogInput, Extinction, Geometry, ImfSpec, MagLimitSpec, Page, Quantity, Selection};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::InvalidPolicy;
use artpop_core::population::{self as pop, CutoffFrame, Population};
use artpop_core::spatial::{self, CategoricalMethod, Coordinates, Model, OutsidePolicy, Positions};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn cdf() -> CategoricalMethod {
    CategoricalMethod::Cdf
}
fn ten() -> f64 {
    10.0
}
fn zero_deg() -> Quantity {
    Quantity::new(0.0, "deg")
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpatialModelSpec {
    /// Square-truncated Sérsic grid of side `2 ceil(r_eff_pix * num_effective_radii) + 1`.
    Sersic {
        effective_radius: Quantity,
        n: f64,
        #[serde(default)]
        ellipticity: f64,
        /// Counterclockwise from +x.
        #[serde(default = "zero_deg")]
        theta: Quantity,
        #[serde(default = "ten")]
        num_effective_radii: f64,
        #[serde(default = "cdf")]
        sampler: CategoricalMethod,
    },
    /// 3D Plummer radii projected onto the sky.
    Plummer { scale_radius: Quantity },
    /// Uniform over the frame.
    Uniform,
    /// A stored probability grid (`spatial.grid-build`).
    Grid {
        grid: ArtifactInput,
        #[serde(default = "cdf")]
        sampler: CategoricalMethod,
    },
    /// Previously sampled positions (`spatial.sample`); count must match.
    Positions { positions: ArtifactInput },
}

fn retain() -> OutsidePolicy {
    OutsidePolicy::RetainMarked
}
fn error_invalid() -> InvalidPolicy {
    InvalidPolicy::Error
}
fn yes() -> bool {
    true
}
fn observed() -> CutoffFrame {
    CutoffFrame::Observed
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UniformSurfaceBrightness {
    pub isochrone: IsochroneInput,
    /// mag/arcsec^2 in `band`.
    pub surface_brightness: f64,
    pub band: String,
    pub distance: Quantity,
    #[serde(default)]
    pub imf: ImfSpec,
    #[serde(default)]
    pub extinction_mag: Extinction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mag_limit: Option<MagLimitSpec>,
    /// Whether `surface_brightness` includes extinction (`observed`, default) or not.
    #[serde(default = "observed")]
    pub surface_brightness_frame: CutoffFrame,
    #[serde(default = "yes")]
    pub add_remnants: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    /// Positions and apparent magnitudes from a CSV table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogInput>,
    /// A population to place with `spatial_model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub population: Option<ArtifactInput>,
    /// Build the population from a constant surface brightness (uniform positions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uniform_surface_brightness: Option<UniformSurfaceBrightness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_model: Option<SpatialModelSpec>,
    pub geometry: Geometry,
    #[serde(default)]
    pub offset_pixels: [f64; 2],
    #[serde(default = "retain")]
    pub outside: OutsidePolicy,
    /// Catalog rows with non-finite coordinates: `error` (default) or `drop`.
    #[serde(default = "error_invalid")]
    pub invalid: InvalidPolicy,
    /// Star labels: phases, component, eep, log_L, log_Teff, initial_mass, current_mass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_type: Option<String>,
    /// Carry unresolved light as smooth components (default true).
    #[serde(default = "yes")]
    pub smooth_light: bool,
}

/// Store a source (consumed: its columns move into the artifact).
pub fn put_source(ctx: &Ctx, s: SourceData, inputs: Vec<String>) -> Result<crate::store::ArtifactRef> {
    let (meta, members) = source_bundle(s);
    ctx.put("source", meta, members, inputs)
}

fn source_summary(s: &SourceData) -> Value {
    json!({
        "geometry": s.geometry,
        "bands": s.bands,
        "magnitude_system": s.magnitude_system,
        "num_stars": s.len(),
        "inside": s.inside.iter().filter(|v| **v).count(),
        "outside_retained": s.inside.iter().filter(|v| !**v).count(),
        "labels": s.labels.as_ref().map(|l| &l.0),
        "smooth_components": s.smooth.iter().map(|c| json!({ "component_id": c.component_id, "label": c.label, "profile": c.profile, "apparent_mag": c.apparent_mag })).collect::<Vec<_>>(),
        "components": s.components,
        "provenance": s.provenance,
    })
}

pub fn build(ctx: &Ctx, req: BuildRequest) -> Result<OpOutput> {
    req.geometry.validate("/geometry", ctx.config().limits.max_image_pixels)?;
    let given = [req.catalog.is_some(), req.population.is_some(), req.uniform_surface_brightness.is_some()].iter().filter(|b| **b).count();
    if given != 1 {
        return Err(Error::invalid("", "Give exactly one of catalog, population, or uniform_surface_brightness."));
    }
    if req.offset_pixels.iter().any(|v| !v.is_finite()) {
        return Err(Error::nonfinite("/offset_pixels", "Offsets must be finite."));
    }
    if let Some(cat) = &req.catalog {
        if req.spatial_model.is_some() {
            return Err(Error::invalid("/spatial_model", "Catalog sources already have positions; omit spatial_model."));
        }
        let s = catalog_source(ctx, cat, &req)?;
        let mut res = source_summary(&s);
        let r = put_source(ctx, s, vec![])?;
        res["source_artifact_id"] = json!(r.artifact_id);
        return Ok(OpOutput::new(res).with(r));
    }
    let mut out = OpOutput::default();
    let (population, pop_id) = if let Some(p) = &req.population {
        (load_population(ctx, &p.artifact_id, "/population/artifact_id")?, p.artifact_id.clone())
    } else {
        let u = req.uniform_surface_brightness.as_ref().unwrap();
        let (p, r, iso_ref) = uniform_population(ctx, u, &req.geometry)?;
        if let Some(i) = iso_ref {
            out.artifacts.push(i);
        }
        let id = r.artifact_id.clone();
        out.artifacts.push(r);
        (p, id)
    };
    let model = match (&req.spatial_model, &req.uniform_surface_brightness) {
        (Some(m), _) => m.clone(),
        (None, Some(_)) => SpatialModelSpec::Uniform,
        (None, None) => return Err(Error::invalid("/spatial_model", "A population source needs spatial_model.")),
    };
    let s = population_source(ctx, &population, &pop_id, &model, &req)?;
    let mut res = source_summary(&s);
    let r = put_source(ctx, s, vec![pop_id.clone()])?;
    res["source_artifact_id"] = json!(r.artifact_id);
    res["population_artifact_id"] = json!(pop_id);
    out.result = res;
    Ok(out.with(r))
}

fn inside_frame(coords: Coordinates, x: f64, y: f64, g: &Geometry) -> bool {
    coords.inside(x, y, g.width, g.height)
}

fn catalog_source(ctx: &Ctx, cat: &CatalogInput, req: &BuildRequest) -> Result<SourceData> {
    let text = ctx.read_text(&cat.path, "/catalog/path")?;
    let t = artpop_io::csv::parse(&text, crate::types::delimiter(&cat.delimiter, "/catalog/delimiter")?).map_err(|e| e.under("/catalog"))?;
    let xs = t.require(&cat.x_column, "/catalog/x_column")?;
    let ys = t.require(&cat.y_column, "/catalog/y_column")?;
    if cat.magnitude_columns.is_empty() {
        return Err(Error::invalid("/catalog/magnitude_columns", "Map at least one band to a magnitude column."));
    }
    let mag_cols: Vec<&[f64]> = cat.magnitude_columns.iter().map(|(b, c)| t.require(c, &format!("/catalog/magnitude_columns/{b}"))).collect::<Result<_>>()?;
    let label_col = cat.label_column.as_ref().map(|c| t.require(c, "/catalog/label_column")).transpose()?;
    if t.rows() as u64 > ctx.exec.max_stars {
        return Err(Error::new(ErrorCode::ResourceLimit, "Catalog exceeds execution.max_stars.").with_details(json!({ "rows": t.rows(), "max_stars": ctx.exec.max_stars })));
    }
    let coords = req.geometry.coordinates;
    let mut s = SourceData {
        geometry: req.geometry.clone(),
        bands: cat.magnitude_columns.keys().cloned().collect(),
        magnitude_system: cat.magnitude_system.clone().unwrap_or_else(|| "unspecified".into()),
        x: Vec::with_capacity(t.rows()),
        y: Vec::with_capacity(t.rows()),
        inside: Vec::with_capacity(t.rows()),
        mags: vec![Vec::with_capacity(t.rows()); mag_cols.len()],
        component: Vec::with_capacity(t.rows()),
        star_index: Vec::with_capacity(t.rows()),
        labels: cat.label_column.as_ref().map(|c| (Labels::Numeric { name: c.clone() }, Vec::new())),
        smooth: vec![],
        components: vec![json!({ "id": 0, "label": "catalog", "path": cat.path })],
        provenance: json!({ "kind": "catalog", "path": cat.path, "rows": t.rows() }),
    };
    let (mut invalid, mut dropped) = (0u64, 0u64);
    for r in 0..t.rows() {
        let (x, y) = (xs[r] + req.offset_pixels[0], ys[r] + req.offset_pixels[1]);
        if !x.is_finite() || !y.is_finite() {
            if req.invalid == InvalidPolicy::Error {
                return Err(Error::nonfinite("/catalog", format!("Catalog row {} has a non-finite coordinate.", r + 1)).with_details(json!({ "row": r, "alternatives": ["invalid: drop"] })));
            }
            invalid += 1;
            continue;
        }
        let ins = inside_frame(coords, x, y, &req.geometry);
        if !ins {
            match req.outside {
                OutsidePolicy::Error => return Err(Error::new(ErrorCode::OutOfDomain, format!("Catalog row {} lies outside the frame.", r + 1)).at("/outside")),
                OutsidePolicy::Drop => {
                    dropped += 1;
                    continue;
                }
                OutsidePolicy::RetainMarked => {}
            }
        }
        s.x.push(x);
        s.y.push(y);
        s.inside.push(ins);
        for (k, col) in mag_cols.iter().enumerate() {
            s.mags[k].push(col[r]);
        }
        s.component.push(0);
        s.star_index.push(r as u64);
        if let (Some((_, v)), Some(lc)) = (s.labels.as_mut(), label_col) {
            v.push(lc[r]);
        }
    }
    if invalid > 0 {
        ctx.warn("INVALID_ROWS_DROPPED", format!("{invalid} catalog rows with non-finite coordinates were dropped."));
    }
    s.provenance["invalid_dropped"] = json!(invalid);
    s.provenance["outside_dropped"] = json!(dropped);
    Ok(s)
}

fn uniform_population(ctx: &Ctx, u: &UniformSurfaceBrightness, g: &Geometry) -> Result<(Population, crate::store::ArtifactRef, Option<crate::store::ArtifactRef>)> {
    let ps = g.pixel_scale("/geometry")?;
    let (iso, _iso_id, _) = crate::ops::population::resolve_isochrone(ctx, &u.isochrone, "/uniform_surface_brightness/isochrone")?;
    let imf = u.imf.build()?;
    let b = iso.filter_index(&u.band).map_err(|e| e.at("/uniform_surface_brightness/band"))?;
    let mean_mag = iso.ssp_mag(&u.band, &imf, artpop_core::imf::Moment::Number)?;
    let d = crate::units::length_pc(&u.distance, "/uniform_surface_brightness/distance")?;
    let ext = u.extinction_mag.resolve(&iso.filters, "/uniform_surface_brightness/extinction_mag")?;
    let a = if u.surface_brightness_frame == CutoffFrame::Observed { ext[b] } else { 0.0 };
    let per_pix = pop::stars_per_pixel(u.surface_brightness, mean_mag + a, d, ps);
    let n = (per_pix * (g.width * g.height) as f64) as u64;
    if n == 0 {
        return Err(Error::domain("/uniform_surface_brightness/surface_brightness", "The surface brightness corresponds to zero stars in this frame."));
    }
    let preq = PopBuild {
        isochrone: u.isochrone.clone(),
        normalization: crate::types::CountOrMass::Count { value: n },
        imf: u.imf.clone(),
        distance: u.distance.clone(),
        extinction_mag: u.extinction_mag.clone(),
        mag_limit: u.mag_limit.clone(),
        add_remnants: u.add_remnants,
        remnant_model: Default::default(),
        mass_tolerance: 0.01,
        sampling: SamplingOptions::default(),
        label: Some("uniform".into()),
    };
    let (p, iso_id, iso_ref) = build_population(ctx, &preq, "uniform-surface-brightness")?;
    let r = put_population(ctx, &p, vec![iso_id])?;
    ctx.warn("UNIFORM_SB_COUNT", format!("Uniform surface brightness implies {per_pix:.6} stars/pixel ({n} stars, mean absolute {} = {mean_mag:.4} with the selected IMF).", u.band));
    Ok((p, r, iso_ref))
}

fn check_center(center: (f64, f64), g: &Geometry) -> Result<()> {
    let (x0, x1) = g.coordinates.frame(g.width);
    let (y0, y1) = g.coordinates.frame(g.height);
    if !(center.0 > x0 && center.0 < x1 && center.1 > y0 && center.1 < y1) {
        return Err(Error::invalid("/offset_pixels", format!("Source center ({}, {}) falls outside the image.", center.0, center.1)));
    }
    Ok(())
}

/// Sample `n` positions from a spatial model; also returns the matching
/// unit-amplitude smooth profile when the model has one.
#[allow(clippy::too_many_arguments)]
pub fn sample_spatial(
    ctx: &Ctx,
    model: &SpatialModelSpec,
    n: usize,
    g: &Geometry,
    offset: (f64, f64),
    outside: OutsidePolicy,
    distance_pc: Option<f64>,
    draws: Option<spatial::Draws>,
) -> Result<(Positions, Option<SmoothProfile>, Option<Model>, Value)> {
    let coords = g.coordinates;
    let (dx, dy) = offset;
    let center = coords.model_center(g.width, g.height);
    let shape_center = (center.0 + dx, center.1 + dy);
    let distance = || -> Result<f64> {
        distance_pc.ok_or_else(|| Error::invalid("/distance", "Physical spatial models need a single distance (give distance, or build sources per component and compose them)."))
    };
    let req_outside = outside;
    let out: (Positions, Option<SmoothProfile>, Option<Model>, Value) = match model {
        SpatialModelSpec::Sersic { effective_radius, n: sn, ellipticity, theta, num_effective_radii, sampler } => {
            let ps = g.pixel_scale("/geometry")?;
            let r_pc = crate::units::length_pc(effective_radius, "/spatial_model/effective_radius")?;
            artpop_core::error::require_positive(r_pc, "/spatial_model/effective_radius/value", "Effective radius")?;
            let theta_rad = crate::units::angle_rad(theta, "/spatial_model/theta")?;
            let r_arcsec = spatial::angular_size_arcsec(r_pc, distance()?);
            let r_pix = r_arcsec / ps;
            check_center(shape_center, g)?;
            let side = spatial::sersic_grid_side(r_pix, *num_effective_radii)?;
            spatial::checked_pixels(side, side, ctx.exec.max_grid_pixels)?;
            let c = (side / 2) as f64;
            let grid_model = Model::Sersic { x0: c, y0: c, amplitude: 1.0, r_eff: r_pix, n: *sn, ellip: *ellipticity, theta: theta_rad };
            let pos = if n > 0 {
                let grid = spatial::WeightGrid::from_model(&grid_model, side, side, ctx.exec.max_grid_pixels).map_err(|e| e.under("/spatial_model"))?;
                spatial::sample_grid(&grid, *sampler, n, (g.width, g.height), (dx, dy), coords, req_outside, draws.unwrap())?
            } else {
                Positions::default()
            };
            let shape = Model::Sersic { x0: shape_center.0, y0: shape_center.1, amplitude: 1.0, r_eff: r_pix, n: *sn, ellip: *ellipticity, theta: theta_rad };
            (pos, Some(SmoothProfile::Sersic { r_eff_arcsec: r_arcsec, n: *sn, ellip: *ellipticity, theta_rad }), Some(shape), json!({ "kind": "sersic", "r_eff_pc": r_pc, "r_eff_arcsec": r_arcsec, "r_eff_pixels": r_pix, "grid_side": side, "sampler": sampler, "truncation": "square grid (ArtPop sersic_xy)" }))
        }
        SpatialModelSpec::Plummer { scale_radius } => {
            let ps = g.pixel_scale("/geometry")?;
            let a_pc = crate::units::length_pc(scale_radius, "/spatial_model/scale_radius")?;
            let a_arcsec = spatial::angular_size_arcsec(a_pc, distance()?);
            let a_pix = a_arcsec / ps;
            check_center(shape_center, g)?;
            let pos = if n > 0 { spatial::sample_plummer(n, a_pix, (g.width, g.height), (dx, dy), coords, req_outside, draws.unwrap())? } else { Positions::default() };
            let shape = Model::Plummer { x0: shape_center.0, y0: shape_center.1, amplitude: 1.0, scale_radius: a_pix };
            (pos, Some(SmoothProfile::Plummer { scale_radius_arcsec: a_arcsec }), Some(shape), json!({ "kind": "plummer", "scale_radius_pc": a_pc, "scale_radius_arcsec": a_arcsec, "scale_radius_pixels": a_pix }))
        }
        SpatialModelSpec::Uniform => {
            if dx != 0.0 || dy != 0.0 {
                ctx.warn("OFFSET_IGNORED", "Offsets do not apply to a uniform distribution.");
            }
            let pos = if n > 0 { spatial::sample_uniform(n, (g.width, g.height), coords, draws.unwrap()) } else { Positions::default() };
            (pos, Some(SmoothProfile::Constant), Some(Model::Constant { amplitude: 1.0 }), json!({ "kind": "uniform" }))
        }
        SpatialModelSpec::Grid { grid, sampler } => {
            let (wg, gmeta) = load_grid(ctx, &grid.artifact_id, "/spatial_model/grid/artifact_id")?;
            let pos = if n > 0 { spatial::sample_grid(&wg, *sampler, n, (g.width, g.height), (dx, dy), coords, req_outside, draws.unwrap())? } else { Positions::default() };
            (pos, None, None, json!({ "kind": "grid", "grid_artifact_id": grid.artifact_id, "sampler": sampler, "grid": gmeta }))
        }
        SpatialModelSpec::Positions { positions } => {
            let (pos, pg) = load_positions(ctx, &positions.artifact_id, "/spatial_model/positions/artifact_id")?;
            if pos.generated as usize != n || pos.x.len() != n {
                return Err(Error::new(ErrorCode::ShapeMismatch, format!("Positions artifact has {} retained of {} generated; the population has {n} stars.", pos.x.len(), pos.generated)).at("/spatial_model/positions"));
            }
            if pg.width != g.width || pg.height != g.height || pg.coordinates != g.coordinates {
                return Err(Error::new(ErrorCode::ShapeMismatch, "Positions were sampled for a different geometry.").at("/spatial_model/positions"));
            }
            (pos, None, None, json!({ "kind": "positions", "positions_artifact_id": positions.artifact_id }))
        }
    };
    Ok(out)
}

fn population_source(ctx: &Ctx, p: &Population, pop_id: &str, model: &SpatialModelSpec, req: &BuildRequest) -> Result<SourceData> {
    let g = &req.geometry;
    let n = p.num_stars();
    let d0 = p.components[0].distance_pc;
    let single = p.components.iter().all(|c| (c.distance_pc - d0).abs() <= 1e-9 * d0);
    let keys = [pop_id];
    let draws = if n > 0 { Some(spatial::Draws { seed: ctx.seed()?, stage: "source.position", keys: &keys }) } else { None };
    let (positions, profile, shape, model_meta) = sample_spatial(ctx, model, n, g, (req.offset_pixels[0], req.offset_pixels[1]), req.outside, if single { Some(d0) } else { None }, draws)?;
    if positions.outside_count() > 0 {
        ctx.warn("STARS_OUTSIDE_FRAME", format!("{} of {} stars fall outside the frame (policy {:?}).", positions.outside_count(), positions.generated, req.outside));
    }
    let idx: Vec<usize> = positions.index.iter().map(|i| *i as usize).collect();
    let mags: Vec<Vec<f64>> = (0..p.filters.len()).map(|b| {
        let all = p.apparent_mags(b);
        idx.iter().map(|i| all[*i]).collect()
    }).collect();
    let labels = match req.label_type.as_deref() {
        None => None,
        Some("phases") => {
            let eep = p.catalog.eep.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "Phase labels need EEP values.").at("/label_type"))?;
            let cats: Vec<String> = pop::PHASE_LABELS.iter().map(|s| s.to_string()).collect();
            Some((Labels::Categorical { name: "phase".into(), categories: cats.clone() }, idx.iter().map(|i| cats.iter().position(|c| c == pop::phase_label(eep[*i])).map_or(f64::NAN, |k| k as f64)).collect()))
        }
        Some(name) => {
            let col = crate::ops::population::population_numeric(p, name, crate::ops::population::MagFrame::Apparent, "/label_type")?;
            Some((Labels::Numeric { name: name.into() }, idx.iter().map(|i| col[*i]).collect()))
        }
    };
    let mut smooth = Vec::new();
    if req.smooth_light {
        for c in &p.components {
            if c.unresolved.is_empty() {
                continue;
            }
            match (&profile, &shape) {
                (Some(pr), Some(sh)) => {
                    let mags: IndexMap<String, Option<f64>> = p.filters.iter().enumerate().map(|(b, f)| (f.clone(), p.unresolved_apparent_mag(c.id, b))).collect();
                    smooth.push(SmoothComponent { component_id: c.id, label: c.label.clone(), profile: pr.clone(), shape: sh.clone(), apparent_mag: mags });
                }
                _ => ctx.warn("SMOOTH_LIGHT_SKIPPED", format!("Component {} has unresolved light but the {} model has no analytic smooth profile; render it with image.smooth or use sersic/plummer/uniform.", c.id, model_meta["kind"])),
            }
        }
    }
    Ok(SourceData {
        geometry: g.clone(),
        bands: p.filters.clone(),
        magnitude_system: p.magnitude_system.clone(),
        x: positions.x.clone(),
        y: positions.y.clone(),
        inside: positions.inside.clone(),
        mags,
        component: idx.iter().map(|i| p.catalog.component[*i]).collect(),
        star_index: positions.index.clone(),
        labels,
        smooth,
        components: p.components.iter().map(|c| json!({ "id": c.id, "label": c.label, "distance_pc": c.distance_pc })).collect(),
        provenance: json!({ "kind": "population", "population_artifact_id": pop_id, "spatial_model": model_meta, "offset_pixels": req.offset_pixels, "outside_policy": req.outside, "generated": positions.generated, "outside": positions.outside_count() }),
    })
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeRequest {
    pub sources: Vec<ArtifactInput>,
    /// Keep every source's smooth components (default true); false keeps stars only (ArtPop `Source.__add__`).
    #[serde(default = "yes")]
    pub retain_smooth: bool,
}

pub fn compose(ctx: &Ctx, req: ComposeRequest) -> Result<OpOutput> {
    if req.sources.len() < 2 {
        return Err(Error::invalid("/sources", "Composition needs at least two sources."));
    }
    let srcs: Vec<SourceData> = req.sources.iter().enumerate().map(|(i, s)| load_source(ctx, &s.artifact_id, &format!("/sources/{i}/artifact_id"))).collect::<Result<_>>()?;
    let g0 = &srcs[0].geometry;
    for (i, s) in srcs.iter().enumerate().skip(1) {
        let g = &s.geometry;
        if g.width != g0.width || g.height != g0.height || g.coordinates != g0.coordinates || g.pixel_scale_arcsec != g0.pixel_scale_arcsec {
            return Err(Error::new(ErrorCode::ShapeMismatch, "Sources must share width, height, pixel scale, and coordinate convention.").at(&format!("/sources/{i}")));
        }
        if s.magnitude_system != srcs[0].magnitude_system {
            return Err(Error::new(ErrorCode::ModelAlignment, "Sources must share a magnitude system.").at(&format!("/sources/{i}")));
        }
    }
    let bands: Vec<String> = srcs[0].bands.iter().filter(|b| srcs.iter().all(|s| s.bands.contains(b))).cloned().collect();
    if bands.is_empty() {
        return Err(Error::new(ErrorCode::ModelAlignment, "Sources share no common band."));
    }
    if srcs.iter().any(|s| s.bands.len() != bands.len()) {
        ctx.warn("BANDS_INTERSECTED", format!("Only bands common to every source were kept: {bands:?}."));
    }
    let mut out = SourceData {
        geometry: g0.clone(),
        bands: bands.clone(),
        magnitude_system: srcs[0].magnitude_system.clone(),
        x: vec![],
        y: vec![],
        inside: vec![],
        mags: vec![Vec::new(); bands.len()],
        component: vec![],
        star_index: vec![],
        labels: Some((Labels::Numeric { name: "source".into() }, vec![])),
        smooth: vec![],
        components: vec![],
        provenance: json!({ "kind": "scene", "sources": req.sources.iter().map(|s| &s.artifact_id).collect::<Vec<_>>(), "retain_smooth": req.retain_smooth }),
    };
    let mut comp_base = 0u32;
    for (si, s) in srcs.iter().enumerate() {
        out.x.extend_from_slice(&s.x);
        out.y.extend_from_slice(&s.y);
        out.inside.extend_from_slice(&s.inside);
        out.star_index.extend_from_slice(&s.star_index);
        out.component.extend(s.component.iter().map(|c| c + comp_base));
        for (k, b) in bands.iter().enumerate() {
            let bi = s.band_index(b)?;
            out.mags[k].extend_from_slice(&s.mags[bi]);
        }
        out.labels.as_mut().unwrap().1.extend(std::iter::repeat_n((si + 1) as f64, s.len()));
        let ncomp = s.components.len().max(1) as u32;
        for c in &s.components {
            let mut c = c.clone();
            c["id"] = json!(c["id"].as_u64().unwrap_or(0) as u32 + comp_base);
            c["source"] = json!(req.sources[si].artifact_id);
            out.components.push(c);
        }
        if req.retain_smooth {
            for sc in &s.smooth {
                let mut sc = sc.clone();
                sc.component_id += comp_base;
                sc.apparent_mag.retain(|b, _| bands.contains(b));
                out.smooth.push(sc);
            }
        } else if !s.smooth.is_empty() {
            ctx.warn("SMOOTH_DROPPED", format!("Source {si} smooth components were dropped (retain_smooth = false)."));
        }
        comp_base += ncomp;
    }
    let mut res = source_summary(&out);
    let r = put_source(ctx, out, req.sources.iter().map(|s| s.artifact_id.clone()).collect())?;
    res["source_artifact_id"] = json!(r.artifact_id);
    Ok(OpOutput::new(res).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub fields: Vec<String>,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let s = load_source(ctx, &req.artifact_id, "/artifact_id")?;
    Ok(OpOutput::new(crate::ops::pick(source_summary(&s), &req.fields, "/fields")?))
}

/// Discrete-star mask for a source selection (None = all stars).
pub fn source_mask(ctx: &Ctx, s: &SourceData, sel: &Selection, pointer: &str) -> Result<Option<Vec<bool>>> {
    let n = s.len();
    Ok(match sel {
        Selection::All => None,
        Selection::Components { ids } => Some(s.component.iter().map(|c| ids.contains(c)).collect()),
        Selection::Labels { values } => {
            let (lab, v) = s.labels.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This source has no labels.").at(pointer))?;
            Some(v.iter().map(|x| values.contains(&label_json(lab, *x))).collect())
        }
        Selection::Phases { phases } => {
            let (lab, v) = s.labels.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "Phase selection needs a source built with label_type = phases.").at(pointer))?;
            if !matches!(lab, Labels::Categorical { name, .. } if name == "phase") {
                return Err(Error::new(ErrorCode::DataMissing, "Phase selection needs a source built with label_type = phases.").at(pointer));
            }
            // Aggregate phases map to their component labels.
            let expand = |p: &str| -> Vec<&'static str> {
                match p {
                    "giants" => vec!["RGB", "CHeB", "EAGB", "TPAGB"],
                    "AGB" => vec!["EAGB", "TPAGB"],
                    "all" => pop::PHASE_LABELS.to_vec(),
                    other => pop::PHASE_LABELS.iter().copied().filter(|x| *x == other).collect(),
                }
            };
            let wanted: Vec<&str> = phases.iter().flat_map(|p| expand(p)).collect();
            Some(v.iter().map(|x| matches!(label_json(lab, *x), Value::String(ref sv) if wanted.contains(&sv.as_str()))).collect())
        }
        Selection::Rows { start, stop, step } => {
            if *step == 0 {
                return Err(Error::invalid(pointer, "Row step must be positive."));
            }
            let mut m = vec![false; n];
            for i in (*start..stop.unwrap_or(n).min(n)).step_by(*step) {
                m[i] = true;
            }
            Some(m)
        }
        Selection::Mask { artifact_id } => {
            let l = ctx.store().load(artifact_id, pointer, &["mask"])?;
            let m = l.bools("mask")?;
            if m.len() != n {
                return Err(Error::new(ErrorCode::ShapeMismatch, "Mask length differs from the number of source stars.").at(pointer));
            }
            Some(m)
        }
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryOutput {
    #[default]
    Rows,
    Mask,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub artifact_id: String,
    /// x, y, inside, component, star_index, label, or band names.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default)]
    pub output: QueryOutput,
    #[serde(default)]
    pub page: Page,
}

pub fn query(ctx: &Ctx, req: QueryRequest) -> Result<OpOutput> {
    let s = load_source(ctx, &req.artifact_id, "/artifact_id")?;
    let mask = source_mask(ctx, &s, &req.selection, "/selection")?;
    if req.output == QueryOutput::Mask {
        let m = mask.unwrap_or_else(|| vec![true; s.len()]);
        let count = m.iter().filter(|v| **v).count();
        let r = ctx.put("mask", json!({ "target": req.artifact_id, "target_kind": "source", "selection": req.selection, "selected": count, "length": m.len() }), vec![Member::bools("mask", &m)], vec![req.artifact_id.clone()])?;
        return Ok(OpOutput::new(json!({ "mask_artifact_id": r.artifact_id, "selected": count, "total": m.len() })).with(r));
    }
    let mut cols = req.columns.clone();
    if cols.is_empty() {
        cols = vec!["x".into(), "y".into(), "inside".into()];
        cols.extend(s.bands.iter().cloned());
    }
    let (start, limit) = req.page.resolve(ctx.config().limits.max_page_rows)?;
    let idx: Vec<usize> = (0..s.len()).filter(|i| mask.as_ref().is_none_or(|m| m[*i])).collect();
    let end = (start + limit).min(idx.len());
    let mut rows = Vec::new();
    for &r in &idx[start.min(end)..end] {
        let mut row = Vec::new();
        for (ci, c) in cols.iter().enumerate() {
            row.push(match c.as_str() {
                "x" => json!(s.x[r]),
                "y" => json!(s.y[r]),
                "inside" => json!(s.inside[r]),
                "component" => json!(s.component[r]),
                "star_index" => json!(s.star_index[r]),
                "label" => {
                    let (lab, v) = s.labels.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This source has no labels.").at(&format!("/columns/{ci}")))?;
                    label_json(lab, v[r])
                }
                band => artpop_core::error::finite_or_null(s.mags[s.band_index(band).map_err(|e| e.at(&format!("/columns/{ci}")))?][r]),
            });
        }
        rows.push(row);
    }
    Ok(OpOutput::new(json!({ "columns": cols, "rows": rows, "selected": idx.len(), "start": start, "next_cursor": if end < idx.len() { json!(format!("row:{end}")) } else { Value::Null } })))
}

/// Surface brightness (mag/arcsec^2) and per-pixel flux amplitude at `zpt`
/// of a smooth component with total magnitude `m_tot` (ArtPop
/// `mag_to_image_amplitude`; Sérsic f_n evaluated in log space).
pub fn smooth_amplitude(profile: &SmoothProfile, m_tot: f64, zpt: f64, ps: f64, frame_pixels: f64) -> Result<(f64, f64)> {
    let flux = 10f64.powf(0.4 * (zpt - m_tot));
    Ok(match profile {
        SmoothProfile::Sersic { r_eff_arcsec, n, ellip, .. } => {
            let bn = artpop_core::special::sersic_bn(*n)?;
            let log10_fn = (artpop_core::special::ln_gamma(2.0 * n) + n.ln() + bn - 2.0 * n * bn.ln()) / std::f64::consts::LN_10;
            let r_circ = r_eff_arcsec * (1.0 - ellip).sqrt();
            let area = std::f64::consts::PI * r_circ * r_circ;
            let mu_e = m_tot + 2.5 * (2.0 * area).log10() + 2.5 * log10_fn;
            (mu_e, 10f64.powf(0.4 * (zpt - mu_e)) * ps * ps)
        }
        SmoothProfile::Plummer { scale_radius_arcsec } => {
            let area = std::f64::consts::PI * scale_radius_arcsec * scale_radius_arcsec;
            (zpt - 2.5 * (flux / area).log10(), flux / area * ps * ps)
        }
        SmoothProfile::Constant => {
            let amp = flux / frame_pixels;
            (zpt - 2.5 * (amp / (ps * ps)).log10(), amp)
        }
    })
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmplitudeRequest {
    /// Source whose smooth component to use (with `component_id`)...
    #[serde(default)]
    pub source: Option<ArtifactInput>,
    #[serde(default)]
    pub component_id: Option<u32>,
    /// ...or an explicit profile: sersic {r_eff_arcsec, n, ellip, theta_rad}, plummer {scale_radius_arcsec}, constant.
    #[serde(default)]
    pub profile: Option<Value>,
    /// Total magnitude of the smooth component (default: the component's unresolved magnitude in `band`).
    #[serde(default)]
    pub total_mag: Option<f64>,
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default = "zpt27")]
    pub zpt: f64,
    #[serde(default)]
    pub pixel_scale_arcsec: Option<f64>,
    /// Frame size in pixels for constant profiles.
    #[serde(default)]
    pub frame_pixels: Option<f64>,
}

fn zpt27() -> f64 {
    27.0
}

pub fn amplitude(ctx: &Ctx, req: AmplitudeRequest) -> Result<OpOutput> {
    let (profile, m_tot, ps, frame) = match (&req.source, &req.profile) {
        (Some(src), None) => {
            let s = load_source(ctx, &src.artifact_id, "/source/artifact_id")?;
            let cid = req.component_id.unwrap_or(0);
            let c = s.smooth.iter().find(|c| c.component_id == cid).ok_or_else(|| Error::invalid("/component_id", format!("Source has no smooth component {cid}.")))?;
            let m = match req.total_mag {
                Some(m) => m,
                None => {
                    let band = req.band.as_ref().ok_or_else(|| Error::invalid("/band", "Give band or total_mag."))?;
                    c.apparent_mag.get(band).copied().flatten().ok_or_else(|| Error::unknown_filter("/band", band, &c.apparent_mag.keys().cloned().collect::<Vec<_>>()))?
                }
            };
            (c.profile.clone(), m, s.geometry.pixel_scale("/source")?, (s.geometry.width * s.geometry.height) as f64)
        }
        (None, Some(p)) => {
            let prof: SmoothProfile = serde_json::from_value(p.clone()).map_err(|e| Error::invalid("/profile", e.to_string()))?;
            let m = req.total_mag.ok_or_else(|| Error::invalid("/total_mag", "total_mag is required with an explicit profile."))?;
            let ps = req.pixel_scale_arcsec.ok_or_else(|| Error::invalid("/pixel_scale_arcsec", "pixel_scale_arcsec is required with an explicit profile."))?;
            (prof, m, ps, req.frame_pixels.unwrap_or(f64::NAN))
        }
        _ => return Err(Error::invalid("/source", "Give exactly one of source or profile.")),
    };
    let (sb, amp) = smooth_amplitude(&profile, m_tot, req.zpt, ps, frame)?;
    let (sb_name, param) = match profile {
        SmoothProfile::Sersic { .. } => ("mu_e", "amplitude at r_eff"),
        SmoothProfile::Plummer { .. } => ("mu_0", "central amplitude"),
        SmoothProfile::Constant => ("mu", "constant amplitude"),
    };
    Ok(OpOutput::new(json!({ "profile": profile, "total_mag": m_tot, "zpt": req.zpt, "surface_brightness_name": sb_name, "surface_brightness_mag_arcsec2": sb, "amplitude_flux_per_pixel": amp, "amplitude_parameter": param })))
}
