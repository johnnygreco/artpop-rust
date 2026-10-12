//! Discovery, schemas, validation, estimates, and citations.

use crate::registry::{self, envelope_schema};
use crate::types::CountOrMass;
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesRequest {
    /// Restrict the operation list to one domain (e.g. "population").
    #[serde(default)]
    pub domain: Option<String>,
}

pub fn capabilities(ctx: &Ctx, req: CapabilitiesRequest) -> Result<OpOutput> {
    let ops: Vec<Value> = registry::all()
        .iter()
        .filter(|d| req.domain.as_deref().is_none_or(|x| d.domain() == x))
        .map(|d| {
            let (domain, action) = d.id.split_once('.').unwrap_or((d.id, ""));
            json!({
                "operation": d.id,
                "mcp_tool": d.mcp_name(),
                "cli": format!("artpop-rust {domain} {action} --input REQUEST.json"),
                "summary": d.summary,
                "effects": d.effects,
                "stochastic": d.stochastic,
                "cost": d.cost,
            })
        })
        .collect();
    let catalog = artpop_core::filters::catalog();
    let datasets = crate::ops::data::registered(ctx).unwrap_or_default();
    Ok(OpOutput::new(json!({
        "engine": "artpop-rust",
        "engine_version": artpop_core::ENGINE_VERSION,
        "schema_version": crate::SCHEMA_VERSION,
        "algorithm_profiles": [artpop_core::ALGORITHM_PROFILE],
        "upstream_reference": { "package": "ArtPop", "version": "0.1.2", "commit": "38a886906ed175d0205bf48b6dc03f54aeb4a81a" },
        "operations": ops,
        "operation_count": registry::all().len(),
        "algorithms": {
            "rng": artpop_core::rng::RNG_ALGORITHM,
            "uniform": artpop_core::rng::UNIFORM_ALGORITHM,
            "normal": artpop_core::rng::NORMAL_ALGORITHM,
            "poisson": artpop_core::rng::POISSON_ALGORITHM,
            "imf_sampling": ["tabulated_inverse", "analytic_inverse"],
            "mass_stopping": ["whole_batch", "nearest_target"],
            "grid_samplers": ["cdf", "alias"],
            "coordinate_conventions": ["pixel-centers-v1", "artpop-0.1.2"],
            "psf_profiles": ["gaussian", "moffat"],
            "psf_discretization": ["center", "linear_interp", "oversample", "integrate"],
            "convolution": { "boundaries": ["wrap", "fill"], "methods": ["auto", "direct", "fft"] },
            "interpolation": ["linear", "nearest", "nearest-up", "previous", "next", "zero", "slinear", "quadratic", "cubic", "integer spline order"],
            "spatial_models": ["sersic", "plummer", "constant", "ring", "gaussian", "sum"],
        },
        "limits": ctx.config().limits,
        "formats": {
            "import": ["csv", "fits", "mist .iso.cmd", "parsec cmd", "artpop-bundle-v1"],
            "export": ["fits", "csv", "json", "png", "svg", "artpop-bundle-v1"],
        },
        "native_libraries": { "cfitsio": artpop_io::fits::cfitsio_version() },
        "filters": { "systems": catalog.systems, "filter_count": catalog.filters.len() },
        "datasets": datasets,
        "data_dir": ctx.config().data_dir,
        "artifact_root": ctx.config().artifact_root,
    })))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSide {
    #[default]
    Input,
    Output,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaRequest {
    pub operation: String,
    #[serde(default)]
    pub side: SchemaSide,
}

pub fn schema(_ctx: &Ctx, req: SchemaRequest) -> Result<OpOutput> {
    let d = registry::find(&req.operation).ok_or_else(|| Error::invalid("/operation", format!("Unknown operation '{}'.", req.operation)))?;
    let schema = match req.side {
        SchemaSide::Input => d.input_schema(),
        SchemaSide::Output => envelope_schema(),
    };
    Ok(OpOutput::new(json!({ "operation": d.id, "mcp_tool": d.mcp_name(), "side": req.side, "schema": schema, "example": example(d.id) })))
}

/// Small executable request examples for discovery.
pub fn example(id: &str) -> Value {
    let seed = json!({ "seed": "000000000000002a" });
    match id {
        "imf.sample" => json!({ "schema_version": "1", "imf": { "kind": "kroupa" }, "count": 1000, "m_min": 0.08, "m_max": 120, "execution": seed }),
        "population.build" => json!({ "schema_version": "1", "isochrone": { "artifact_id": "sha256:..." }, "normalization": { "kind": "count", "value": 100000 }, "imf": { "kind": "kroupa" }, "distance": { "value": 10, "unit": "pc" }, "execution": seed }),
        "psf.build" => json!({ "schema_version": "1", "kind": "moffat", "fwhm": { "value": 0.6, "unit": "arcsec" }, "pixel_scale_arcsec": 0.2, "width": 41, "height": 41 }),
        "spatial.grid-build" => json!({ "schema_version": "1", "weights_yx": [[0, 1], [2, 3]], "width": 2, "height": 2 }),
        _ => json!({ "schema_version": "1" }),
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateRequest {
    pub operation: String,
    pub request: Value,
}

pub fn validate(ctx: &Ctx, req: ValidateRequest) -> Result<OpOutput> {
    let d = registry::find(&req.operation).ok_or_else(|| Error::invalid("/operation", format!("Unknown operation '{}'.", req.operation)))?;
    let (obj, exec) = ctx.svc.prepare(req.request).map_err(|e| e.under("/request"))?;
    let resolved = (d.validate)(Value::Object(obj)).map_err(|e| e.under("/request"))?;
    let mut warnings = Vec::new();
    if d.stochastic && exec.seed.is_none() {
        warnings.push("This stochastic operation will fail without execution.seed.");
    }
    let mut full = resolved;
    if let Value::Object(m) = &mut full {
        m.insert("schema_version".into(), json!("1"));
        m.insert("execution".into(), serde_json::to_value(&exec).unwrap());
    }
    Ok(OpOutput::new(json!({ "operation": d.id, "valid": true, "resolved_request": full, "notes": warnings })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EstimateRequest {
    pub operation: String,
    pub request: Value,
}

pub fn estimate(ctx: &Ctx, req: EstimateRequest) -> Result<OpOutput> {
    let d = registry::find(&req.operation).ok_or_else(|| Error::invalid("/operation", format!("Unknown operation '{}'.", req.operation)))?;
    let (obj, exec) = ctx.svc.prepare(req.request).map_err(|e| e.under("/request"))?;
    let resolved = (d.validate)(Value::Object(obj)).map_err(|e| e.under("/request"))?;
    let mut est = json!({ "operation": d.id, "cost": d.cost, "stochastic": d.stochastic });
    let image_bytes = |w: u64, h: u64, planes: u64| w * h * 8 * planes;
    match d.id {
        "population.build" => {
            let norm: CountOrMass = serde_json::from_value(resolved["normalization"].clone()).map_err(|e| Error::invalid("/request/normalization", e.to_string()))?;
            match norm {
                CountOrMass::Count { value } => {
                    est["stars"] = json!({ "minimum": 0, "maximum": value });
                    est["catalog_bytes_upper_bound"] = json!(value * (8 * 12 + 12));
                }
                CountOrMass::CurrentMass { value, .. } => {
                    // Kroupa mean initial mass on [0.1, 1] Msun is ~0.3 Msun.
                    let n = (value / 0.25).ceil() as u64;
                    est["stars"] = json!({ "approximate": n, "note": "depends on the isochrone mass range, IMF, and cutoff" });
                    est["catalog_bytes_approximate"] = json!(n * (8 * 12 + 12));
                }
            }
        }
        "spatial.grid-build" | "spatial.sample" | "source.build" => {
            let w = resolved.get("width").or_else(|| resolved.pointer("/geometry/width")).and_then(Value::as_u64).unwrap_or(0);
            let h = resolved.get("height").or_else(|| resolved.pointer("/geometry/height")).and_then(Value::as_u64).unwrap_or(0);
            est["image_pixels"] = json!(w * h);
            est["grid_note"] = json!("Sérsic grids have side 2*ceil(r_eff_pixels*num_effective_radii)+1; each cell costs 8 bytes (16 with the alias sampler).");
        }
        "observe.ideal" | "observe.artificial" | "image.convolve" | "image.histogram" => {
            est["note"] = json!("Image size follows the referenced source/image geometry; artificial observations store up to 6 f64 planes.");
            if let (Some(w), Some(h)) = (resolved.pointer("/geometry/width").and_then(Value::as_u64), resolved.pointer("/geometry/height").and_then(Value::as_u64)) {
                est["image_bytes"] = json!(image_bytes(w, h, 1));
            }
        }
        _ => {}
    }
    est["limits"] = serde_json::to_value(&exec).unwrap();
    Ok(OpOutput::new(est))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CitationsRequest {
    /// Artifact IDs whose provenance should be summarized.
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

pub fn citations(ctx: &Ctx, req: CitationsRequest) -> Result<OpOutput> {
    let mut used = Vec::new();
    for (i, id) in req.artifact_ids.iter().enumerate() {
        let l = ctx.store().load(id, &format!("/artifact_ids/{i}"), &[])?;
        used.push(json!({ "artifact_id": id, "kind": l.manifest.kind, "created_by": l.manifest.created_by, "inputs": l.manifest.inputs }));
    }
    let provenance: Value = serde_json::from_str(artpop_core::filters::PROVENANCE_JSON).unwrap_or(Value::Null);
    Ok(OpOutput::new(json!({
        "references": [
            { "topic": "ArtPop", "citation": "Greco, J. P. & Danieli, S. 2022, ApJ, 941, 26 (ArtPop: A Stellar Population and Image Simulation Python Package)" },
            { "topic": "MIST isochrones", "citation": "Dotter, A. 2016, ApJS, 222, 8; Choi, J. et al. 2016, ApJ, 823, 102" },
            { "topic": "MESA", "citation": "Paxton, B. et al. 2011, 2013, 2015, ApJS" },
            { "topic": "Kroupa IMF", "citation": "Kroupa, P. 2001, MNRAS, 322, 231" },
            { "topic": "Salpeter IMF", "citation": "Salpeter, E. E. 1955, ApJ, 121, 161" },
            { "topic": "Scalo IMF", "citation": "Scalo, J. 1998, ASPC, 142, 201" },
            { "topic": "Remnant masses", "citation": "Renzini, A. & Ciotti, L. 1993, ApJ, 416, L49" },
            { "topic": "Filter definitions", "citation": "Fukugita, M. et al. 1996, AJ, 111, 1748" },
            { "topic": "Moffat alpha 4.765", "citation": "Trujillo, I. et al. 2001, MNRAS, 328, 977" },
            { "topic": "Poisson sampler (PTRS)", "citation": "Hörmann, W. 1993, Insurance: Mathematics and Economics, 12, 39" },
            { "topic": "FITS I/O", "citation": "Pence, W. 1999, ASPC, 172, 487 (CFITSIO)" },
            { "topic": "PARSEC isochrones", "citation": "Bressan, A. et al. 2012, MNRAS, 427, 127" },
        ],
        "bundled_data_provenance": provenance,
        "artifacts": used,
    })))
}
