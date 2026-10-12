//! Population construction, composition, photometry, and queries.

use crate::codec::{load_isochrone, load_population, population_bundle};
use crate::ops::isochrone::{MistRequest, mist_isochrone, put_isochrone};
use crate::ops::{mag_or_null, pick};
use crate::store::Member;
use crate::types::{ArtifactInput, CountOrMass, Extinction, ImfSpec, MagLimitSpec, Page, Quantity, Selection};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::imf::{DEFAULT_NUM_EDGES, SamplingAlgorithm, StoppingPolicy};
use artpop_core::isochrone::{Isochrone, RemnantModel};
use artpop_core::population::{self as pop, MagLimit, Normalization, Population};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// An isochrone artifact, or an inline MIST selection (`"kind": "mist"`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IsochroneInput {
    Artifact(ArtifactInput),
    Mist(MistInline),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MistKind {
    Mist,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MistInline {
    pub kind: MistKind,
    #[serde(default = "v12")]
    pub version: String,
    #[serde(default = "rot")]
    pub rotation: f64,
    #[serde(default)]
    pub alpha_enhancement: f64,
    pub log_age_yr: f64,
    pub feh: f64,
    pub photometric_systems: Vec<String>,
    #[serde(default = "ab")]
    pub magnitude_system: String,
    #[serde(default)]
    pub missing_zeropoint: crate::ops::isochrone::MissingZeropoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

fn v12() -> String {
    "1.2".into()
}
fn rot() -> f64 {
    0.4
}
fn ab() -> String {
    "AB".into()
}

/// Resolve an isochrone input; returns the isochrone and its artifact reference.
pub fn resolve_isochrone(ctx: &Ctx, input: &IsochroneInput, pointer: &str) -> Result<(Isochrone, String, Option<crate::store::ArtifactRef>)> {
    match input {
        IsochroneInput::Artifact(a) => Ok((load_isochrone(ctx, &a.artifact_id, &format!("{pointer}/artifact_id"))?, a.artifact_id.clone(), None)),
        IsochroneInput::Mist(m) => {
            let req = MistRequest {
                version: m.version.clone(),
                rotation: m.rotation,
                alpha_enhancement: m.alpha_enhancement,
                log_age_yr: m.log_age_yr,
                feh: m.feh,
                photometric_systems: m.photometric_systems.clone(),
                magnitude_system: m.magnitude_system.clone(),
                missing_zeropoint: m.missing_zeropoint,
                data_dir: m.data_dir.clone(),
            };
            let iso = mist_isochrone(ctx, &req).map_err(|e| if e.path.as_deref().is_some_and(|p| p.starts_with(pointer)) { e } else { e.under(pointer) })?;
            let r = put_isochrone(ctx, &iso)?;
            Ok((iso, r.artifact_id.clone(), Some(r)))
        }
    }
}

fn ten_pc() -> Quantity {
    Quantity::new(10.0, "pc")
}
fn yes() -> bool {
    true
}
fn tol() -> f64 {
    0.01
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SamplingOptions {
    /// `tabulated_inverse` reproduces ArtPop's discretized CDF; `analytic_inverse` is exact.
    pub algorithm: SamplingAlgorithm,
    pub num_mass_edges: usize,
    /// Mass normalization only: `whole_batch` (ArtPop) or `nearest_target`.
    pub stopping: StoppingPolicy,
}

impl Default for SamplingOptions {
    fn default() -> Self {
        SamplingOptions { algorithm: SamplingAlgorithm::TabulatedInverse, num_mass_edges: DEFAULT_NUM_EDGES, stopping: StoppingPolicy::WholeBatch }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    pub isochrone: IsochroneInput,
    pub normalization: CountOrMass,
    #[serde(default)]
    pub imf: ImfSpec,
    #[serde(default = "ten_pc")]
    pub distance: Quantity,
    /// Magnitudes of extinction: one number for all bands or a per-band map.
    #[serde(default)]
    pub extinction_mag: Extinction,
    /// Sample only stars brighter than this; fainter light becomes unresolved moments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mag_limit: Option<MagLimitSpec>,
    #[serde(default = "yes")]
    pub add_remnants: bool,
    #[serde(default)]
    pub remnant_model: RemnantModel,
    /// Batch sizing for mass normalization (ArtPop `SSP` default 0.01).
    #[serde(default = "tol")]
    pub mass_tolerance: f64,
    #[serde(default)]
    pub sampling: SamplingOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

pub fn build_population(ctx: &Ctx, req: &BuildRequest, stage_key: &str) -> Result<(Population, String, Option<crate::store::ArtifactRef>)> {
    let (iso, iso_id, iso_ref) = resolve_isochrone(ctx, &req.isochrone, "/isochrone")?;
    let imf = req.imf.build()?;
    let distance_pc = crate::units::length_pc(&req.distance, "/distance")?;
    artpop_core::error::require_positive(distance_pc, "/distance/value", "Distance")?;
    let extinction = req.extinction_mag.resolve(&iso.filters, "/extinction_mag")?;
    let normalization = match &req.normalization {
        CountOrMass::Count { value } => Normalization::Count { value: *value },
        CountOrMass::CurrentMass { value, unit } => Normalization::CurrentMass { value: crate::units::mass_msun(&Quantity::new(*value, unit), "/normalization")? },
    };
    let mag_limit = match &req.mag_limit {
        Some(m) => {
            iso.filter_index(&m.band).map_err(|e| e.at("/mag_limit/band"))?;
            if !m.magnitude.is_finite() {
                return Err(Error::nonfinite("/mag_limit/magnitude", "Magnitude limit must be finite."));
            }
            Some(MagLimit { magnitude: m.magnitude, band: m.band.clone(), frame: m.frame })
        }
        None => None,
    };
    let label = req.label.clone().unwrap_or_else(|| "ssp".into());
    let spec = pop::BuildSpec {
        isochrone: &iso,
        isochrone_identity: json!({ "artifact_id": iso_id, "model": iso.metadata }),
        normalization,
        imf,
        distance_pc,
        extinction,
        mag_limit,
        add_remnants: req.add_remnants,
        remnant_model: req.remnant_model,
        mass_tolerance: req.mass_tolerance,
        algorithm: req.sampling.algorithm,
        num_edges: req.sampling.num_mass_edges,
        stopping: req.sampling.stopping,
        max_stars: ctx.exec.max_stars,
        label,
    };
    let seed = ctx.seed()?;
    let p = pop::build(&spec, seed, stage_key)?;
    let c = &p.components[0];
    if c.sample_fraction.regime == pop::SamplingRegime::Unresolved {
        ctx.warn("ALL_UNRESOLVED", "The magnitude limit is brighter than every isochrone star; the population is entirely unresolved light.");
    }
    Ok((p, iso_id, iso_ref))
}

pub fn put_population(ctx: &Ctx, p: &Population, inputs: Vec<String>) -> Result<crate::store::ArtifactRef> {
    let (meta, members) = population_bundle(p);
    ctx.put("population", meta, members, inputs)
}

fn population_summary(p: &Population) -> Value {
    summary_from(&p.filters, &p.magnitude_system, &p.components, p.num_stars(), p.catalog.eep.is_some())
}

/// Summary from metadata alone (no catalog members are read).
fn summary_from(filters: &[String], magnitude_system: &str, components: &[pop::Component], num_stars: usize, has_eep: bool) -> Value {
    let p = PopMeta { filters, components };
    let comps: Vec<Value> = p
        .components
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "label": c.label,
                "distance_pc": c.distance_pc,
                "distance_modulus": c.distance_modulus(),
                "extinction_mag": p.filters.iter().cloned().zip(c.extinction.iter().cloned()).collect::<IndexMap<_, _>>(),
                "imf": c.imf,
                "regime": c.sample_fraction.regime,
                "mass_limit_msun": c.sample_fraction.mass_limit,
                "frac_num_sampled": c.sample_fraction.frac_num_sampled,
                "frac_mass_sampled": c.sample_fraction.frac_mass_sampled,
                "mag_limit": c.mag_limit,
                "normalization": c.normalization,
                "total_mass_msun": c.total_mass,
                "live_star_mass_msun": c.live_star_mass,
                "sampled_mass_msun": c.sampled_mass,
                "discrete_initial_mass_msun": c.discrete_initial_mass,
                "num_stars_discrete": c.num_discrete,
                "num_stars_unresolved_expected": c.unresolved.expected_count,
                "unresolved_current_mass_msun": c.unresolved.current_mass,
                "remnants_factor": c.remnants_factor,
                "add_remnants": c.add_remnants,
                "isochrone": c.isochrone.get("artifact_id"),
                "sampling": c.sampling,
            })
        })
        .collect();
    let single_distance = p.components.iter().all(|c| (c.distance_pc - p.components[0].distance_pc).abs() <= 1e-9 * p.components[0].distance_pc);
    json!({
        "filters": p.filters,
        "magnitude_system": magnitude_system,
        "num_components": p.components.len(),
        "num_stars_discrete": num_stars,
        "num_stars_unresolved_expected": p.components.iter().map(|c| c.unresolved.expected_count).sum::<f64>(),
        "total_mass_msun": p.components.iter().map(|c| c.total_mass).sum::<f64>(),
        "live_star_mass_msun": p.components.iter().map(|c| c.live_star_mass).sum::<f64>(),
        "sampled_mass_msun": p.components.iter().map(|c| c.sampled_mass).sum::<f64>(),
        "discrete_initial_mass_msun": p.components.iter().map(|c| c.discrete_initial_mass).sum::<f64>(),
        "distance_pc": if single_distance { json!(p.components[0].distance_pc) } else { Value::Null },
        "distance_modulus": if single_distance { json!(p.components[0].distance_modulus()) } else { Value::Null },
        "has_unresolved_light": p.components.iter().any(|c| !c.unresolved.is_empty()),
        "has_eep": has_eep,
        "components": comps,
    })
}

struct PopMeta<'a> {
    filters: &'a [String],
    components: &'a [pop::Component],
}

pub fn build(ctx: &Ctx, req: BuildRequest) -> Result<OpOutput> {
    let (p, iso_id, iso_ref) = build_population(ctx, &req, "component-0")?;
    let r = put_population(ctx, &p, vec![iso_id.clone()])?;
    let mut res = population_summary(&p);
    res["population_artifact_id"] = json!(r.artifact_id);
    res["isochrone_artifact_id"] = json!(iso_id);
    let mut out = OpOutput::new(res);
    if let Some(i) = iso_ref {
        out = out.with(i);
    }
    Ok(out.with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeInput {
    pub artifact_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

fn yes2() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeRequest {
    pub populations: Vec<ComposeInput>,
    /// ArtPop semantics: populations must share a distance (use scenes otherwise).
    #[serde(default = "yes2")]
    pub require_equal_distance: bool,
}

pub fn compose(ctx: &Ctx, req: ComposeRequest) -> Result<OpOutput> {
    if req.populations.len() < 2 {
        return Err(Error::invalid("/populations", "Composition needs at least two populations."));
    }
    let pops: Vec<Population> = req.populations.iter().enumerate().map(|(i, p)| load_population(ctx, &p.artifact_id, &format!("/populations/{i}/artifact_id"))).collect::<Result<_>>()?;
    let parts: Vec<(&Population, Option<String>)> = pops.iter().zip(&req.populations).map(|(p, r)| (p, r.label.clone())).collect();
    let c = pop::compose(&parts, req.require_equal_distance)?;
    let r = put_population(ctx, &c, req.populations.iter().map(|p| p.artifact_id.clone()).collect())?;
    let total_n: f64 = c.components.iter().map(|x| x.num_discrete as f64 + x.unresolved.expected_count).sum();
    let mut res = population_summary(&c);
    res["population_artifact_id"] = json!(r.artifact_id);
    res["number_fractions"] = json!(c.components.iter().map(|x| (x.num_discrete as f64 + x.unresolved.expected_count) / total_n).collect::<Vec<_>>());
    res["mass_fractions"] = json!(c.components.iter().map(|x| x.total_mass / c.total_mass()).collect::<Vec<_>>());
    Ok(OpOutput::new(res).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub artifact_id: String,
    /// Subset of summary fields (default: all).
    #[serde(default)]
    pub fields: Vec<String>,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let l = ctx.store().load(&req.artifact_id, "/artifact_id", &["population"])?;
    let (filters, system, components, n, has_eep): (Vec<String>, String, Vec<pop::Component>, usize, bool) = (l.meta("filters")?, l.meta("magnitude_system")?, l.meta("components")?, l.meta("num_stars")?, l.meta("has_eep")?);
    Ok(OpOutput::new(pick(summary_from(&filters, &system, &components, n, has_eep), &req.fields, "/fields")?))
}

/// Resolve a discrete-star selection mask (None = all stars).
pub fn population_mask(ctx: &Ctx, p: &Population, sel: &Selection, pointer: &str) -> Result<Option<Vec<bool>>> {
    let n = p.num_stars();
    Ok(match sel {
        Selection::All => None,
        Selection::Components { ids } => {
            for id in ids {
                if *id as usize >= p.components.len() {
                    return Err(Error::invalid(pointer, format!("Unknown component id {id}.")));
                }
            }
            Some(p.catalog.component.iter().map(|c| ids.contains(c)).collect())
        }
        Selection::Phases { phases } => {
            let eep = p.catalog.eep.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "Phase selection needs EEP values (MIST isochrones or an eep_column).").at(pointer))?;
            for ph in phases {
                pop::in_phase(ph, 0.0).map_err(|e| e.at(pointer))?;
            }
            Some(eep.iter().map(|e| phases.iter().any(|ph| pop::in_phase(ph, *e).unwrap_or(false))).collect())
        }
        Selection::Rows { start, stop, step } => {
            if *step == 0 {
                return Err(Error::invalid(pointer, "Row step must be positive."));
            }
            let stop = stop.unwrap_or(n).min(n);
            let mut m = vec![false; n];
            for i in (*start..stop).step_by(*step) {
                m[i] = true;
            }
            Some(m)
        }
        Selection::Mask { artifact_id } => {
            let l = ctx.store().load(artifact_id, pointer, &["mask"])?;
            let m = l.bools("mask")?;
            if m.len() != n {
                return Err(Error::new(ErrorCode::ShapeMismatch, "Mask length differs from the population size.").at(pointer));
            }
            Some(m)
        }
        Selection::Labels { .. } => return Err(Error::invalid(pointer, "Label selections apply to sources; use components, phases, rows, or a mask for populations.")),
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MagFrame {
    #[default]
    Apparent,
    Absolute,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub artifact_id: String,
    /// star_id, component, initial_mass, current_mass, any band, eep, log_L, log_Teff, phase.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub magnitude_frame: MagFrame,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default)]
    pub page: Page,
}

/// Column values for a population catalog (`None` for string columns).
pub fn population_column(p: &Population, name: &str, frame: MagFrame, pointer: &str) -> Result<Vec<Value>> {
    let c = &p.catalog;
    let f = |v: &[f64]| v.iter().map(|x| artpop_core::error::finite_or_null(*x)).collect::<Vec<_>>();
    Ok(match name {
        "star_id" => c.star_id.iter().map(|v| json!(v)).collect(),
        "component" => c.component.iter().map(|v| json!(v)).collect(),
        "initial_mass" => f(&c.initial_mass),
        "current_mass" | "star_mass" => f(&c.current_mass),
        "eep" => f(c.eep.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This population has no EEP column.").at(pointer))?),
        "log_L" => f(c.log_l.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This population has no log_L column.").at(pointer))?),
        "log_Teff" => f(c.log_teff.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This population has no log_Teff column.").at(pointer))?),
        "phase" => c.eep.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "Phases need EEP values.").at(pointer))?.iter().map(|e| json!(pop::phase_label(*e))).collect(),
        band => {
            let b = p.filter_index(band).map_err(|e| e.at(pointer))?;
            match frame {
                MagFrame::Absolute => f(&c.abs_mags[b]),
                MagFrame::Apparent => f(&p.apparent_mags(b)),
            }
        }
    })
}

/// Numeric column (apparent or absolute magnitudes; masses; ids as f64).
pub fn population_numeric(p: &Population, name: &str, frame: MagFrame, pointer: &str) -> Result<Vec<f64>> {
    let c = &p.catalog;
    Ok(match name {
        "star_id" => c.star_id.iter().map(|v| *v as f64).collect(),
        "component" => c.component.iter().map(|v| *v as f64).collect(),
        "initial_mass" => c.initial_mass.clone(),
        "current_mass" | "star_mass" => c.current_mass.clone(),
        "eep" | "log_L" | "log_Teff" => {
            let col = match name {
                "eep" => &c.eep,
                "log_L" => &c.log_l,
                _ => &c.log_teff,
            };
            col.clone().ok_or_else(|| Error::new(ErrorCode::DataMissing, format!("This population has no {name} column.")).at(pointer))?
        }
        band => {
            let b = p.filter_index(band).map_err(|e| e.at(pointer))?;
            match frame {
                MagFrame::Absolute => c.abs_mags[b].clone(),
                MagFrame::Apparent => p.apparent_mags(b),
            }
        }
    })
}

pub fn query(ctx: &Ctx, req: QueryRequest) -> Result<OpOutput> {
    let p = load_population(ctx, &req.artifact_id, "/artifact_id")?;
    let mask = population_mask(ctx, &p, &req.selection, "/selection")?;
    let (start, limit) = req.page.resolve(ctx.config().limits.max_page_rows)?;
    let mut cols = req.columns.clone();
    if cols.is_empty() {
        cols = vec!["star_id".into(), "component".into(), "initial_mass".into(), "current_mass".into()];
        cols.extend(p.filters.iter().cloned());
    }
    let data: Vec<Vec<Value>> = cols.iter().enumerate().map(|(i, c)| population_column(&p, c, req.magnitude_frame, &format!("/columns/{i}"))).collect::<Result<_>>()?;
    let idx: Vec<usize> = (0..p.num_stars()).filter(|i| mask.as_ref().is_none_or(|m| m[*i])).collect();
    let end = (start + limit).min(idx.len());
    let rows: Vec<Vec<Value>> = idx[start.min(end)..end].iter().map(|r| data.iter().map(|c| c[*r].clone()).collect()).collect();
    Ok(OpOutput::new(json!({
        "columns": cols,
        "magnitude_frame": req.magnitude_frame,
        "rows": rows,
        "selected": idx.len(),
        "start": start,
        "next_cursor": if end < idx.len() { json!(format!("row:{end}")) } else { Value::Null },
    })))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedPolicy {
    /// Include integrated light only when every discrete star is selected (ArtPop).
    #[default]
    Auto,
    /// Include the unresolved light of selected components.
    Include,
    Exclude,
}

fn default_phot_metrics() -> Vec<String> {
    vec!["total_mag".into(), "mean_mag".into(), "sbf_mag".into()]
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhotometryRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub bands: Vec<String>,
    /// total_mag, mean_mag, sbf_mag.
    #[serde(default = "default_phot_metrics")]
    pub metrics: Vec<String>,
    /// Pairs `[blue, red]` for integrated colors.
    #[serde(default)]
    pub colors: Vec<[String; 2]>,
    #[serde(default)]
    pub selection: Selection,
    #[serde(default)]
    pub unresolved: UnresolvedPolicy,
}

pub fn photometry(ctx: &Ctx, req: PhotometryRequest) -> Result<OpOutput> {
    let p = load_population(ctx, &req.artifact_id, "/artifact_id")?;
    let mask = population_mask(ctx, &p, &req.selection, "/selection")?;
    let comps: Vec<u32> = match (&req.unresolved, &req.selection) {
        (UnresolvedPolicy::Exclude, _) => vec![],
        (UnresolvedPolicy::Auto, s) if !s.is_all() => vec![],
        (_, Selection::Components { ids }) => ids.clone(),
        _ => (0..p.components.len() as u32).collect(),
    };
    let bands = if req.bands.is_empty() { p.filters.clone() } else { req.bands.clone() };
    let mut sums = IndexMap::new();
    for (i, b) in bands.iter().enumerate() {
        let bi = p.filter_index(b).map_err(|e| e.at(&format!("/bands/{i}")))?;
        sums.insert(b.clone(), p.band_sums(bi, mask.as_deref(), &comps));
    }
    let mut out = serde_json::Map::new();
    for m in &req.metrics {
        let f: fn(&pop::BandSums) -> f64 = match m.as_str() {
            "total_mag" => |s| s.flux(),
            "mean_mag" => |s| s.flux() / (s.discrete_count + s.unresolved_count),
            "sbf_mag" => |s| (s.discrete_flux2 + s.unresolved_flux2) / s.flux(),
            other => return Err(Error::invalid("/metrics", format!("Unknown metric '{other}'.")).with_details(json!({ "available": default_phot_metrics() }))),
        };
        let vals: IndexMap<String, Value> = sums.iter().map(|(b, s)| (b.clone(), mag_or_null(f(s)))).collect();
        out.insert(m.clone(), json!(vals));
    }
    if !req.colors.is_empty() {
        let mut c = IndexMap::new();
        for (i, [blue, red]) in req.colors.iter().enumerate() {
            let fb = p.band_sums(p.filter_index(blue).map_err(|e| e.at(&format!("/colors/{i}/0")))?, mask.as_deref(), &comps).flux();
            let fr = p.band_sums(p.filter_index(red).map_err(|e| e.at(&format!("/colors/{i}/1")))?, mask.as_deref(), &comps).flux();
            c.insert(format!("{blue}-{red}"), if fb > 0.0 && fr > 0.0 { json!(-2.5 * (fb / fr).log10()) } else { Value::Null });
        }
        out.insert("colors".into(), json!(c));
    }
    let no_flux: Vec<&String> = sums.iter().filter(|(_, s)| !(s.flux() > 0.0)).map(|(b, _)| b).collect();
    out.insert("status".into(), json!({ "no_flux_bands": no_flux }));
    out.insert("moments".into(), json!(sums));
    out.insert("unresolved_components_included".into(), json!(comps));
    out.insert("magnitude_system".into(), json!(p.magnitude_system));
    Ok(OpOutput::new(Value::Object(out)))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub distance: Option<Quantity>,
    #[serde(default)]
    pub extinction_mag: Option<Extinction>,
    /// Restrict the change to these component ids (default: all).
    #[serde(default)]
    pub components: Option<Vec<u32>>,
}

pub fn update(ctx: &Ctx, req: UpdateRequest) -> Result<OpOutput> {
    let p = load_population(ctx, &req.artifact_id, "/artifact_id")?;
    if req.distance.is_none() && req.extinction_mag.is_none() {
        return Err(Error::invalid("", "Give distance and/or extinction_mag."));
    }
    let d = req.distance.as_ref().map(|q| crate::units::length_pc(q, "/distance")).transpose()?;
    let e = req.extinction_mag.as_ref().map(|x| x.resolve(&p.filters, "/extinction_mag")).transpose()?;
    let new = p.with_observing_conditions(d, e.as_deref(), req.components.as_deref())?;
    if new.components.iter().any(|c| c.mag_limit.is_some()) {
        ctx.warn("CUTOFF_UNCHANGED", "Discrete/unresolved membership was decided at build time; changing distance or extinction does not re-apply the magnitude limit.");
    }
    let r = put_population(ctx, &new, vec![req.artifact_id.clone()])?;
    let mut res = population_summary(&new);
    res["population_artifact_id"] = json!(r.artifact_id);
    Ok(OpOutput::new(res).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleFractionRequest {
    pub isochrone: IsochroneInput,
    #[serde(default)]
    pub imf: ImfSpec,
    pub mag_limit: MagLimitSpec,
    #[serde(default = "ten_pc")]
    pub distance: Quantity,
    #[serde(default)]
    pub extinction_mag: Extinction,
}

pub fn sample_fraction(ctx: &Ctx, req: SampleFractionRequest) -> Result<OpOutput> {
    let (iso, iso_id, iso_ref) = resolve_isochrone(ctx, &req.isochrone, "/isochrone")?;
    let imf = req.imf.build()?;
    let d = crate::units::length_pc(&req.distance, "/distance")?;
    let ext = req.extinction_mag.resolve(&iso.filters, "/extinction_mag")?;
    let lim = MagLimit { magnitude: req.mag_limit.magnitude, band: req.mag_limit.band.clone(), frame: req.mag_limit.frame };
    let f = pop::sample_fraction(&iso, &imf, Some(&lim), d, &ext)?;
    let mut out = OpOutput::new(json!({ "isochrone_artifact_id": iso_id, "sample_fraction": f, "distance_modulus": pop::distance_modulus(d) }));
    if let Some(r) = iso_ref {
        out = out.with(r);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseMode {
    #[default]
    Counts,
    Labels,
    Mask,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhasesRequest {
    #[serde(default)]
    pub artifact_id: Option<String>,
    /// EEP values to classify instead of a population.
    #[serde(default)]
    pub eep: Option<Vec<f64>>,
    #[serde(default)]
    pub mode: PhaseMode,
    /// Phase for `mask` mode (PMS, MS, giants, RGB, CHeB, AGB, EAGB, TPAGB, postAGB, WDCS, all).
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub page: Page,
}

pub fn phases(ctx: &Ctx, req: PhasesRequest) -> Result<OpOutput> {
    let eep = match (&req.artifact_id, &req.eep) {
        (Some(id), None) => {
            let p = load_population(ctx, id, "/artifact_id")?;
            p.catalog.eep.ok_or_else(|| Error::new(ErrorCode::DataMissing, "This population has no EEP values (requires MIST or an eep_column).").at("/artifact_id"))?
        }
        (None, Some(e)) => e.clone(),
        _ => return Err(Error::invalid("/artifact_id", "Give exactly one of artifact_id or eep.")),
    };
    match req.mode {
        PhaseMode::Counts => {
            let mut counts = IndexMap::new();
            for ph in pop::PHASES {
                counts.insert(ph, eep.iter().filter(|e| pop::in_phase(ph, **e).unwrap()).count());
            }
            Ok(OpOutput::new(json!({ "counts": counts, "total": eep.len(), "available_phases": pop::PHASES, "definition": "MIST EEP intervals (ArtPop select_phase)" })))
        }
        PhaseMode::Labels => {
            let (start, limit) = req.page.resolve(ctx.config().limits.max_page_rows)?;
            let end = (start + limit).min(eep.len());
            let labels: Vec<&str> = eep[start.min(end)..end].iter().map(|e| pop::phase_label(*e)).collect();
            Ok(OpOutput::new(json!({ "labels": labels, "start": start, "next_cursor": if end < eep.len() { json!(format!("row:{end}")) } else { Value::Null } })))
        }
        PhaseMode::Mask => {
            let phase = req.phase.as_ref().ok_or_else(|| Error::invalid("/phase", "mask mode requires phase."))?;
            pop::in_phase(phase, 0.0)?;
            let mask: Vec<bool> = eep.iter().map(|e| pop::in_phase(phase, *e).unwrap()).collect();
            let count = mask.iter().filter(|m| **m).count();
            let r = ctx.put("mask", json!({ "target": req.artifact_id, "target_kind": "population", "phase": phase, "selected": count, "length": mask.len() }), vec![Member::bools("mask", &mask)], req.artifact_id.iter().cloned().collect())?;
            Ok(OpOutput::new(json!({ "mask_artifact_id": r.artifact_id, "phase": phase, "selected": count, "total": mask.len() })).with(r))
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceDensityRequest {
    /// Surface brightness in mag/arcsec^2.
    pub surface_brightness: f64,
    /// Mean absolute stellar magnitude; or give `isochrone` + `band` (+ `imf`) to compute it.
    #[serde(default)]
    pub mean_mag: Option<f64>,
    #[serde(default)]
    pub isochrone: Option<ArtifactInput>,
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default)]
    pub imf: ImfSpec,
    pub distance: Quantity,
    pub pixel_scale_arcsec: f64,
}

pub fn surface_density(ctx: &Ctx, req: SurfaceDensityRequest) -> Result<OpOutput> {
    let mean = match (req.mean_mag, &req.isochrone, &req.band) {
        (Some(m), None, _) => m,
        (None, Some(iso), Some(band)) => {
            let iso = load_isochrone(ctx, &iso.artifact_id, "/isochrone/artifact_id")?;
            iso.ssp_mag(band, &req.imf.build()?, artpop_core::imf::Moment::Number).map_err(|e| e.at("/band"))?
        }
        _ => return Err(Error::invalid("/mean_mag", "Give mean_mag, or isochrone and band.")),
    };
    let d = crate::units::length_pc(&req.distance, "/distance")?;
    artpop_core::error::require_positive(req.pixel_scale_arcsec, "/pixel_scale_arcsec", "Pixel scale")?;
    let per_pix = pop::stars_per_pixel(req.surface_brightness, mean, d, req.pixel_scale_arcsec);
    Ok(OpOutput::new(json!({
        "mean_absolute_mag": mean,
        "stars_per_arcsec2": per_pix / (req.pixel_scale_arcsec * req.pixel_scale_arcsec),
        "stars_per_pixel": per_pix,
        "distance_modulus": pop::distance_modulus(d),
    })))
}
