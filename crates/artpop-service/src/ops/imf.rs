//! IMF evaluation, integration, and sampling.

use crate::store::Member;
use crate::types::{ImfSpec, Quantity};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, Result};
use artpop_core::imf::{self, Moment, SamplingAlgorithm, StoppingPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImfQuantity {
    #[default]
    DnDm,
    MDnDm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NormKind {
    /// Raw ArtPop scale (first segment equals 1 at the first break).
    #[default]
    None,
    /// Divide by the sum of the returned values.
    Sum,
    /// Divide by the number integral over the bounds.
    Number,
    /// Divide by the mass integral over the bounds.
    Mass,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvaluateRequest {
    #[serde(default)]
    pub imf: ImfSpec,
    /// Masses in Msun.
    pub masses: Vec<f64>,
    #[serde(default)]
    pub quantity: ImfQuantity,
    #[serde(default)]
    pub normalization: NormKind,
    /// Normalization bounds (default: min/max of `masses`).
    #[serde(default)]
    pub norm_m_min: Option<f64>,
    #[serde(default)]
    pub norm_m_max: Option<f64>,
}

pub fn evaluate(ctx: &Ctx, req: EvaluateRequest) -> Result<OpOutput> {
    let imf = req.imf.build()?;
    if req.masses.is_empty() || req.masses.len() > ctx.config().limits.max_inline_values {
        return Err(Error::invalid("/masses", "Provide between 1 and max_inline_values masses."));
    }
    if req.masses.iter().any(|m| !m.is_finite() || *m <= 0.0) {
        return Err(Error::invalid("/masses", "Masses must be finite and positive."));
    }
    let mut v: Vec<f64> = req.masses.iter().map(|m| imf.raw(*m)).collect();
    let (lo, hi) = (
        req.norm_m_min.unwrap_or_else(|| req.masses.iter().cloned().fold(f64::INFINITY, f64::min)),
        req.norm_m_max.unwrap_or_else(|| req.masses.iter().cloned().fold(0.0, f64::max)),
    );
    let norm = match req.normalization {
        NormKind::None => 1.0,
        NormKind::Sum => v.iter().sum(),
        NormKind::Number => imf.number(lo, hi),
        NormKind::Mass => imf.mass(lo, hi),
    };
    for (x, m) in v.iter_mut().zip(&req.masses) {
        *x /= norm;
        if matches!(req.quantity, ImfQuantity::MDnDm) {
            *x *= m;
        }
    }
    Ok(OpOutput::new(json!({ "imf": imf, "values": v, "normalization_value": norm, "normalization_bounds": if matches!(req.normalization, NormKind::Number | NormKind::Mass) { json!([lo, hi]) } else { json!(null) } })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrateRequest {
    #[serde(default)]
    pub imf: ImfSpec,
    pub m_min: f64,
    pub m_max: f64,
    #[serde(default = "number")]
    pub moment: Moment,
    /// Optional normalization range: the result is divided by the same moment over it.
    #[serde(default)]
    pub normalize_over: Option<[f64; 2]>,
}

fn number() -> Moment {
    Moment::Number
}

pub fn integrate(_ctx: &Ctx, req: IntegrateRequest) -> Result<OpOutput> {
    let imf = req.imf.build()?;
    if !(req.m_min > 0.0) || !(req.m_max > req.m_min) || !req.m_max.is_finite() {
        return Err(Error::invalid("/m_min", "Require finite bounds with 0 < m_min < m_max."));
    }
    let v = imf.integrate(req.m_min, req.m_max, req.moment);
    let norm = match req.normalize_over {
        Some([a, b]) => {
            if !(a > 0.0 && b > a) {
                return Err(Error::invalid("/normalize_over", "Normalization bounds need 0 < low < high."));
            }
            imf.integrate(a, b, req.moment)
        }
        None => 1.0,
    };
    Ok(OpOutput::new(json!({ "integral": v / norm, "raw_integral": v, "normalization": norm, "moment": req.moment, "imf": imf })))
}

fn default_algorithm() -> SamplingAlgorithm {
    SamplingAlgorithm::TabulatedInverse
}
fn edges() -> usize {
    imf::DEFAULT_NUM_EDGES
}
fn m_lo() -> f64 {
    0.08
}
fn m_hi() -> f64 {
    120.0
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleRequest {
    #[serde(default)]
    pub imf: ImfSpec,
    pub count: u64,
    #[serde(default = "m_lo")]
    pub m_min: f64,
    #[serde(default = "m_hi")]
    pub m_max: f64,
    /// `tabulated_inverse` (ArtPop discretization, default) or `analytic_inverse`.
    #[serde(default = "default_algorithm")]
    pub algorithm: SamplingAlgorithm,
    #[serde(default = "edges")]
    pub num_mass_edges: usize,
}

fn masses_artifact(ctx: &Ctx, masses: &[f64], meta: serde_json::Value) -> Result<(crate::store::ArtifactRef, serde_json::Value)> {
    let sum: f64 = masses.iter().sum();
    let n = masses.len() as f64;
    let (min, max) = masses.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), m| (a.min(*m), b.max(*m)));
    let stats = json!({ "count": masses.len(), "total_mass_msun": sum, "mean_mass_msun": if n > 0.0 { json!(sum / n) } else { json!(null) }, "min_mass_msun": if n > 0.0 { json!(min) } else { json!(null) }, "max_mass_msun": if n > 0.0 { json!(max) } else { json!(null) } });
    let mut m = meta;
    m["statistics"] = stats.clone();
    let r = ctx.put("masses", m, vec![Member::column("initial_mass", masses, Some("Msun"))], vec![])?;
    Ok((r, stats))
}

pub fn sample(ctx: &Ctx, req: SampleRequest) -> Result<OpOutput> {
    let imf = req.imf.build()?;
    if req.count == 0 || req.count > ctx.exec.max_stars {
        return Err(Error::new(artpop_core::ErrorCode::ResourceLimit, "count must be between 1 and execution.max_stars.").at("/count").with_details(json!({ "max_stars": ctx.exec.max_stars })));
    }
    let seed = ctx.seed()?;
    let sampler = imf::Sampler::new(&imf, req.m_min, req.m_max, req.algorithm, req.num_mass_edges)?;
    let masses = sampler.sample(req.count as usize, seed, "imf.sample", &["mass"]);
    let (r, stats) = masses_artifact(ctx, &masses, json!({ "imf": imf, "algorithm": req.algorithm, "num_mass_edges": req.num_mass_edges, "mass_range_msun": [req.m_min, req.m_max] }))?;
    Ok(OpOutput::new(json!({ "masses_artifact_id": r.artifact_id, "statistics": stats })).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    #[serde(default)]
    pub imf: ImfSpec,
    pub target_initial_mass: Quantity,
    /// Stars drawn per batch (ArtPop default 1e5).
    #[serde(default = "batch")]
    pub batch_size: u64,
    #[serde(default = "whole")]
    pub stopping: StoppingPolicy,
    #[serde(default = "m_lo")]
    pub m_min: f64,
    #[serde(default = "m_hi")]
    pub m_max: f64,
    #[serde(default = "default_algorithm")]
    pub algorithm: SamplingAlgorithm,
    #[serde(default = "edges")]
    pub num_mass_edges: usize,
}

fn batch() -> u64 {
    100_000
}
fn whole() -> StoppingPolicy {
    StoppingPolicy::WholeBatch
}

pub fn build(ctx: &Ctx, req: BuildRequest) -> Result<OpOutput> {
    let imf = req.imf.build()?;
    let target = crate::units::mass_msun(&req.target_initial_mass, "/target_initial_mass")?;
    let seed = ctx.seed()?;
    let sampler = imf::Sampler::new(&imf, req.m_min, req.m_max, req.algorithm, req.num_mass_edges)?;
    let out = imf::build_to_mass(&sampler, target, req.batch_size, req.stopping, ctx.exec.max_stars, seed, "imf.build", &["mass"])?;
    let (r, stats) = masses_artifact(ctx, &out.masses, json!({ "imf": imf, "algorithm": req.algorithm, "stopping": req.stopping, "target_initial_mass_msun": target, "batch_size": out.batch_size }))?;
    Ok(OpOutput::new(json!({
        "masses_artifact_id": r.artifact_id,
        "target_initial_mass_msun": target,
        "realized_initial_mass_msun": out.realized_mass,
        "overshoot_msun": out.realized_mass - target,
        "draws": out.draws,
        "batches": out.batches,
        "stopping": req.stopping,
        "statistics": stats,
    }))
    .with(r))
}
