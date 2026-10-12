//! Spatial model evaluation, probability-grid construction, and sampling.

use crate::codec::{image_member, positions_bundle, read_image};
use crate::ops::source::{SpatialModelSpec, sample_spatial};
use crate::store::Member;
use crate::types::{ArtifactInput, Geometry, Quantity};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::Image;
use artpop_core::spatial::{self, CategoricalMethod, Model, OutsidePolicy, WeightGrid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GridSpec {
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvaluateRequest {
    /// Model in pixel units.
    pub model: Model,
    #[serde(default)]
    pub x: Vec<f64>,
    #[serde(default)]
    pub y: Vec<f64>,
    /// Evaluate on integer pixels of this grid and store an image artifact.
    #[serde(default)]
    pub grid: Option<GridSpec>,
}

pub fn evaluate(ctx: &Ctx, req: EvaluateRequest) -> Result<OpOutput> {
    let m = req.model.prepare()?;
    if let Some(g) = &req.grid {
        let p = spatial::checked_pixels(g.width, g.height, ctx.config().limits.max_image_pixels)?;
        let mut data = Vec::with_capacity(p);
        for y in 0..g.height {
            for x in 0..g.width {
                data.push(m.eval(x as f64, y as f64));
            }
        }
        let img = Image::new(g.width, g.height, data)?;
        let (min, max) = img.min_max();
        let r = ctx.put("image", json!({ "width": g.width, "height": g.height, "unit": "model", "model": req.model, "sum": img.sum() }), vec![image_member("image", &img, "model")], vec![])?;
        return Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "sum": img.sum(), "min": min, "max": max })).with(r));
    }
    if req.x.len() != req.y.len() || req.x.is_empty() {
        return Err(Error::new(ErrorCode::ShapeMismatch, "Give equal-length, nonempty x and y (or a grid).").at("/x"));
    }
    if req.x.len() > ctx.config().limits.max_inline_values {
        return Err(Error::new(ErrorCode::ResourceLimit, "Too many points for an inline request; use grid.").at("/x"));
    }
    let v: Vec<Value> = req.x.iter().zip(&req.y).map(|(x, y)| artpop_core::error::finite_or_null(m.eval(*x, *y))).collect();
    Ok(OpOutput::new(json!({ "values": v })))
}

fn cdf() -> CategoricalMethod {
    CategoricalMethod::Cdf
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GridBuildRequest {
    /// Row-major weights `weights_yx[y][x]` (small grids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_yx: Option<Vec<Vec<f64>>>,
    /// Weights from an image artifact (e.g. `spatial.evaluate` or an imported FITS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_image: Option<ArtifactInput>,
    /// Model in pixel units evaluated at integer pixels of the grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    pub width: usize,
    pub height: usize,
    /// Default categorical sampler stored with the grid.
    #[serde(default = "cdf")]
    pub sampler: CategoricalMethod,
}

pub fn load_grid(ctx: &Ctx, id: &str, pointer: &str) -> Result<(WeightGrid, Value)> {
    let l = ctx.store().load(id, pointer, &["spatial_grid"])?;
    let img = read_image(&l, "weights")?;
    let g = WeightGrid::from_weights(img.width, img.height, img.data)?;
    Ok((g, l.manifest.metadata.clone()))
}

pub fn grid_build(ctx: &Ctx, req: GridBuildRequest) -> Result<OpOutput> {
    let given = [req.weights_yx.is_some(), req.weights_image.is_some(), req.model.is_some()].iter().filter(|b| **b).count();
    if given != 1 {
        return Err(Error::invalid("", "Give exactly one of weights_yx, weights_image, or model."));
    }
    spatial::checked_pixels(req.width, req.height, ctx.exec.max_grid_pixels)?;
    let grid = if let Some(rows) = &req.weights_yx {
        if rows.len() != req.height || rows.iter().any(|r| r.len() != req.width) {
            return Err(Error::new(ErrorCode::ShapeMismatch, format!("weights_yx must have {} rows of {} values.", req.height, req.width)).at("/weights_yx"));
        }
        if req.width * req.height > ctx.config().limits.max_inline_values {
            return Err(Error::new(ErrorCode::ResourceLimit, "Inline weights are too large; use weights_image.").at("/weights_yx"));
        }
        WeightGrid::from_weights(req.width, req.height, rows.iter().flatten().copied().collect()).map_err(|e| e.under("/weights_yx"))?
    } else if let Some(a) = &req.weights_image {
        let l = ctx.store().load(&a.artifact_id, "/weights_image/artifact_id", &["image"])?;
        let img = read_image(&l, "image")?;
        if img.width != req.width || img.height != req.height {
            return Err(Error::new(ErrorCode::ShapeMismatch, "Image shape differs from width/height.").at("/weights_image"));
        }
        WeightGrid::from_weights(img.width, img.height, img.data)?
    } else {
        WeightGrid::from_model(req.model.as_ref().unwrap(), req.width, req.height, ctx.exec.max_grid_pixels).map_err(|e| e.under("/model"))?
    };
    let img = Image::new(grid.width, grid.height, grid.weights.clone())?;
    let meta = json!({
        "width": grid.width,
        "height": grid.height,
        "total_weight": grid.total,
        "positive_cells": grid.positive_cells,
        "sampler": req.sampler,
        "layout": "row-major weights[y][x]",
        "model": req.model,
    });
    let inputs = req.weights_image.iter().map(|a| a.artifact_id.clone()).collect();
    let r = ctx.put("spatial_grid", meta, vec![Member::f64("weights", &img.data, vec![img.height, img.width], None)], inputs)?;
    Ok(OpOutput::new(json!({ "spatial_grid_artifact_id": r.artifact_id, "width": grid.width, "height": grid.height, "total_weight": grid.total, "positive_cells": grid.positive_cells, "bytes": grid.weights.len() * 8 })).with(r))
}

fn error_outside() -> OutsidePolicy {
    OutsidePolicy::RetainMarked
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleRequest {
    /// A stored probability grid...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<ArtifactInput>,
    /// ...or a distribution (sersic, plummer, uniform) in physical units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<SpatialModelSpec>,
    /// Distance for physical distributions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<Quantity>,
    pub count: u64,
    pub geometry: Geometry,
    #[serde(default)]
    pub offset_pixels: [f64; 2],
    #[serde(default = "error_outside")]
    pub outside: OutsidePolicy,
    /// Override the grid's stored sampler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<CategoricalMethod>,
}

pub fn sample(ctx: &Ctx, req: SampleRequest) -> Result<OpOutput> {
    req.geometry.validate("/geometry", u64::MAX)?;
    if req.count == 0 || req.count > ctx.exec.max_stars {
        return Err(Error::new(ErrorCode::ResourceLimit, "count must be between 1 and execution.max_stars.").at("/count"));
    }
    if req.offset_pixels.iter().any(|v| !v.is_finite()) {
        return Err(Error::nonfinite("/offset_pixels", "Offsets must be finite."));
    }
    let seed = ctx.seed()?;
    let n = req.count as usize;
    let (positions, meta, inputs) = match (&req.grid, &req.distribution) {
        (Some(g), None) => {
            let (grid, gmeta) = load_grid(ctx, &g.artifact_id, "/grid/artifact_id")?;
            let method = req.sampler.unwrap_or_else(|| serde_json::from_value(gmeta["sampler"].clone()).unwrap_or(CategoricalMethod::Cdf));
            let keys = [g.artifact_id.as_str()];
            let p = spatial::sample_grid(&grid, method, n, (req.geometry.width, req.geometry.height), (req.offset_pixels[0], req.offset_pixels[1]), req.geometry.coordinates, req.outside, spatial::Draws { seed, stage: "spatial.sample", keys: &keys })?;
            (p, json!({ "kind": "grid", "grid_artifact_id": g.artifact_id, "sampler": method, "grid_shape": [grid.width, grid.height] }), vec![g.artifact_id.clone()])
        }
        (None, Some(d)) => {
            let dist = req.distance.as_ref().map(|q| crate::units::length_pc(q, "/distance")).transpose()?;
            let (p, _, _, m) = sample_spatial(ctx, d, n, &req.geometry, (req.offset_pixels[0], req.offset_pixels[1]), req.outside, dist, Some(spatial::Draws { seed, stage: "spatial.sample", keys: &["distribution"] }))?;
            (p, m, vec![])
        }
        _ => return Err(Error::invalid("/grid", "Give exactly one of grid or distribution.")),
    };
    if positions.outside_count() > 0 && req.outside != OutsidePolicy::Error {
        ctx.warn("STARS_OUTSIDE_FRAME", format!("{} of {} positions fall outside the frame (policy {:?}).", positions.outside_count(), positions.generated, req.outside));
    }
    let mut m = meta.clone();
    m["outside_policy"] = json!(req.outside);
    m["offset_pixels"] = json!(req.offset_pixels);
    let (generated, retained, outside) = (positions.generated, positions.x.len(), positions.outside_count());
    let (bm, members) = positions_bundle(positions, &req.geometry, m);
    let r = ctx.put("positions", bm, members, inputs)?;
    Ok(OpOutput::new(json!({ "positions_artifact_id": r.artifact_id, "generated": generated, "retained": retained, "outside": outside, "sampling": meta })).with(r))
}
