//! Filter catalogs, custom throughput curves, and magnitude-system conversion.

use crate::store::Member;
use crate::types::Quantity;
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::filters::{FilterCurve, MagSystem, catalog};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    /// Photometric systems to list (default: all 12).
    #[serde(default)]
    pub systems: Vec<String>,
    /// Look up the system of one filter name.
    #[serde(default)]
    pub filter: Option<String>,
}

fn filter_entry(name: &str) -> Value {
    let c = catalog();
    let f = c.get(name);
    let native = c.native_system(name).ok().map(|s| s.name());
    json!({
        "name": name,
        "system": f.map(|f| f.system.clone()),
        "native_magnitude_system": native,
        "dlam_angstrom": f.map(|f| f.dlam_angstrom),
        "lam_eff_angstrom": f.map(|f| f.lam_eff_angstrom),
        "conversions": if native.is_some() { json!(["AB", "Vega", "ST"]) } else { json!([]) },
    })
}

pub fn list(_ctx: &Ctx, req: ListRequest) -> Result<OpOutput> {
    let c = catalog();
    if let Some(name) = &req.filter {
        if c.get(name).is_none() {
            return Err(Error::unknown_filter("/filter", name, &c.filters.iter().map(|f| f.name.clone()).collect::<Vec<_>>()));
        }
        return Ok(OpOutput::new(json!({ "filter": filter_entry(name) })));
    }
    let systems: Vec<String> = if req.systems.is_empty() { c.systems.clone() } else { req.systems.clone() };
    let mut out = Vec::new();
    for (i, s) in systems.iter().enumerate() {
        let fs = c.system_filters(s).ok_or_else(|| Error::invalid(&format!("/systems/{i}"), format!("Unknown photometric system '{s}'.")).with_details(json!({ "available": c.systems })))?;
        out.push(json!({ "system": c.canonical_system(s), "filters": fs.iter().map(|f| filter_entry(&f.name)).collect::<Vec<_>>() }));
    }
    Ok(OpOutput::new(json!({ "systems": out, "filter_count": out.iter().map(|s| s["filters"].as_array().map_or(0, |a| a.len())).sum::<usize>() })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub filters: Vec<String>,
    /// Custom filter catalog artifact from `filters.import`.
    #[serde(default)]
    pub catalog_artifact_id: Option<String>,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let mut out = Vec::new();
    if let Some(id) = &req.catalog_artifact_id {
        let l = ctx.store().load(id, "/catalog_artifact_id", &["filters"])?;
        let entries: Vec<Value> = l.meta("filters")?;
        for (i, f) in req.filters.iter().enumerate() {
            let e = entries.iter().find(|e| e["name"] == f.as_str()).ok_or_else(|| Error::unknown_filter(&format!("/filters/{i}"), f, &entries.iter().filter_map(|e| e["name"].as_str().map(String::from)).collect::<Vec<_>>()))?;
            out.push(e.clone());
        }
    } else {
        let c = catalog();
        for (i, f) in req.filters.iter().enumerate() {
            if c.get(f).is_none() {
                return Err(Error::unknown_filter(&format!("/filters/{i}"), f, &[]));
            }
            out.push(filter_entry(f));
        }
    }
    Ok(OpOutput::new(json!({ "filters": out, "source": if req.catalog_artifact_id.is_some() { "custom curves" } else { "bundled ArtPop 0.1.2 filter properties" } })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurveInput {
    pub name: String,
    /// CSV/whitespace file with wavelength and transmission columns.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub wavelength: Option<Vec<f64>>,
    #[serde(default)]
    pub transmission: Option<Vec<f64>>,
    #[serde(default = "wave")]
    pub wavelength_column: String,
    #[serde(default = "trans")]
    pub transmission_column: String,
    #[serde(default = "angstrom")]
    pub wavelength_unit: String,
}

fn wave() -> String {
    "wave".into()
}
fn trans() -> String {
    "trans".into()
}
fn angstrom() -> String {
    "Angstrom".into()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub curves: Vec<CurveInput>,
    /// Magnitude system of magnitudes that will use these filters.
    #[serde(default = "ab")]
    pub magnitude_system: String,
}

fn ab() -> String {
    "AB".into()
}

pub fn load_curve(ctx: &Ctx, c: &CurveInput, pointer: &str) -> Result<FilterCurve> {
    let factor = crate::units::wavelength_angstrom(&Quantity::new(1.0, &c.wavelength_unit), &format!("{pointer}/wavelength_unit"))?;
    let (w, t) = match (&c.path, &c.wavelength, &c.transmission) {
        (Some(p), None, None) => {
            let text = ctx.read_text(p, &format!("{pointer}/path"))?;
            let table = artpop_io::csv::parse(&text, None).map_err(|e| e.under(pointer))?;
            (table.require(&c.wavelength_column, &format!("{pointer}/wavelength_column"))?.to_vec(), table.require(&c.transmission_column, &format!("{pointer}/transmission_column"))?.to_vec())
        }
        (None, Some(w), Some(t)) => (w.clone(), t.clone()),
        _ => return Err(Error::invalid(pointer, "Give exactly one of `path` or inline `wavelength` + `transmission`.")),
    };
    let curve = FilterCurve { name: c.name.clone(), wavelength_angstrom: w.iter().map(|x| x * factor).collect(), transmission: t };
    curve.validate().map_err(|e| e.under(pointer))?;
    Ok(curve)
}

pub fn import(ctx: &Ctx, req: ImportRequest) -> Result<OpOutput> {
    if req.curves.is_empty() {
        return Err(Error::invalid("/curves", "At least one curve is required."));
    }
    MagSystem::parse(&req.magnitude_system, "/magnitude_system")?;
    let mut entries = Vec::new();
    let mut members = Vec::new();
    for (i, c) in req.curves.iter().enumerate() {
        let p = format!("/curves/{i}");
        if req.curves[..i].iter().any(|o| o.name == c.name) {
            return Err(Error::invalid(&format!("{p}/name"), "Duplicate filter name."));
        }
        let curve = load_curve(ctx, c, &p)?;
        let props = curve.properties()?;
        entries.push(json!({ "name": c.name, "dlam_angstrom": props.dlam_angstrom, "lam_eff_angstrom": props.lam_eff_angstrom, "lam_pivot_angstrom": props.lam_pivot_angstrom, "effective_throughput": props.effective_throughput, "points": curve.wavelength_angstrom.len() }));
        members.push(Member::column(&format!("wavelength:{}", c.name), &curve.wavelength_angstrom, Some("Angstrom")));
        members.push(Member::column(&format!("transmission:{}", c.name), &curve.transmission, None));
    }
    let r = ctx.put("filters", json!({ "filters": entries, "magnitude_system": req.magnitude_system, "definitions": "Fukugita et al. (1996) trapezoidal integrals as ArtPop FilterSystem" }), members, vec![])?;
    Ok(OpOutput::new(json!({ "catalog_artifact_id": r.artifact_id, "filters": entries })).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertRequest {
    /// Filter for magnitude conversion (or the blue filter of a color).
    pub filter: String,
    /// Red filter: when present, `values` are colors `filter - red_filter`.
    #[serde(default)]
    pub red_filter: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub values: Vec<f64>,
}

pub fn convert(_ctx: &Ctx, req: ConvertRequest) -> Result<OpOutput> {
    let c = catalog();
    let from = MagSystem::parse(&req.from, "/from")?;
    let to = MagSystem::parse(&req.to, "/to")?;
    let off_blue = c.offset(&req.filter, from, to).map_err(|e| e.at("/filter"))?;
    let offset = match &req.red_filter {
        Some(red) => off_blue - c.offset(red, from, to).map_err(|e| e.at("/red_filter"))?,
        None => off_blue,
    };
    if req.values.iter().any(|v| !v.is_finite()) {
        return Err(Error::new(ErrorCode::NonfiniteValue, "Values must be finite.").at("/values"));
    }
    Ok(OpOutput::new(json!({
        "filter": req.filter,
        "red_filter": req.red_filter,
        "from": from.name(),
        "to": to.name(),
        "offset_mag": offset,
        "converted": req.values.iter().map(|v| v + offset).collect::<Vec<_>>(),
        "table": "MIST zeropoints.txt (bundled with ArtPop 0.1.2)",
    })))
}
