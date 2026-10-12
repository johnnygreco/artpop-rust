//! Isochrone import, MIST selection, and IMF-weighted queries.

use crate::codec::{isochrone_bundle, load_isochrone};
use crate::ops::data;
use crate::types::{ImfSpec, Page, Quantity};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::filters::{MagSystem, catalog};
use artpop_core::imf::Moment;
use artpop_core::interp::{Bounds, Interp1d, InterpKind};
use artpop_core::isochrone::{Isochrone, RemnantModel};
use artpop_core::mist;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsoFormat {
    Csv,
    Parsec,
    Mist,
    Inline,
}

fn init_col() -> String {
    "initial_mass".into()
}
fn cur_col() -> String {
    "star_mass".into()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub format: IsoFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default = "init_col")]
    pub initial_mass_column: String,
    #[serde(default = "cur_col")]
    pub current_mass_column: String,
    /// Band name -> column (csv/inline). Empty for parsec/mist (detected).
    #[serde(default)]
    pub magnitude_columns: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eep_column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_l_column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_teff_column: Option<String>,
    #[serde(default)]
    pub extra_columns: Vec<String>,
    /// Magnitude system label of the imported magnitudes (AB, Vega, ST, or a custom label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnitude_system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    /// PARSEC: select one log10(age/yr) (|dlogAge| < 1e-5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_age: Option<f64>,
    /// PARSEC: select one initial metal fraction (|dZ| < 1e-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zini: Option<f64>,
    /// PARSEC: number of trailing filter columns (default: columns after `mbolmag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_filters: Option<usize>,
    /// MIST `.iso.cmd`: nearest block age.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_age_yr: Option<f64>,
    /// Inline columns by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<IndexMap<String, Vec<f64>>>,
}

struct Columns {
    names: Vec<String>,
    data: Vec<Vec<f64>>,
}

impl Columns {
    fn get(&self, name: &str, pointer: &str) -> Result<Vec<f64>> {
        self.names
            .iter()
            .position(|n| n == name)
            .map(|i| self.data[i].clone())
            .ok_or_else(|| Error::invalid(pointer, format!("Column '{name}' is not present.")).with_details(json!({ "available": self.names })))
    }
}

fn from_columns(cols: &Columns, req: &ImportRequest, mag_cols: &IndexMap<String, String>, system: String, meta: Value) -> Result<Isochrone> {
    let opt = |c: &Option<String>, p: &str| -> Result<Option<Vec<f64>>> { c.as_ref().map(|n| cols.get(n, p)).transpose() };
    let iso = Isochrone {
        initial_mass: cols.get(&req.initial_mass_column, "/initial_mass_column")?,
        current_mass: cols.get(&req.current_mass_column, "/current_mass_column")?,
        filters: mag_cols.keys().cloned().collect(),
        mags: mag_cols.iter().map(|(b, c)| cols.get(c, &format!("/magnitude_columns/{b}"))).collect::<Result<_>>()?,
        magnitude_system: system,
        eep: opt(&req.eep_column, "/eep_column")?,
        log_l: opt(&req.log_l_column, "/log_l_column")?,
        log_teff: opt(&req.log_teff_column, "/log_teff_column")?,
        extra: req.extra_columns.iter().enumerate().map(|(i, n)| Ok((n.clone(), cols.get(n, &format!("/extra_columns/{i}"))?))).collect::<Result<_>>()?,
        metadata: meta,
    };
    if iso.filters.is_empty() {
        return Err(Error::invalid("/magnitude_columns", "At least one magnitude column is required."));
    }
    iso.validate()?;
    Ok(iso)
}

fn store_isochrone(ctx: &Ctx, iso: &Isochrone, extra: Value) -> Result<OpOutput> {
    let (meta, members) = isochrone_bundle(iso);
    let r = ctx.put("isochrone", meta, members, vec![])?;
    let mut res = json!({
        "isochrone_artifact_id": r.artifact_id,
        "rows": iso.len(),
        "filters": iso.filters,
        "magnitude_system": iso.magnitude_system,
        "mass_range_msun": [iso.m_min(), iso.m_max()],
        "columns": iso.column_names(),
    });
    if let (Value::Object(a), Value::Object(b)) = (&mut res, extra) {
        a.extend(b);
    }
    Ok(OpOutput::new(res).with(r))
}

pub fn import(ctx: &Ctx, req: ImportRequest) -> Result<OpOutput> {
    match req.format {
        IsoFormat::Csv | IsoFormat::Inline => {
            let system = req.magnitude_system.clone().ok_or_else(|| Error::invalid("/magnitude_system", "State the magnitude system of the imported magnitudes (e.g. \"AB\")."))?;
            let cols = if req.format == IsoFormat::Csv {
                let path = req.path.as_ref().ok_or_else(|| Error::invalid("/path", "format csv requires path."))?;
                let text = ctx.read_text(path, "/path")?;
                let t = artpop_io::csv::parse(&text, crate::types::delimiter(&req.delimiter, "/delimiter")?)?;
                Columns { names: t.columns, data: t.data }
            } else {
                let c = req.columns.as_ref().ok_or_else(|| Error::invalid("/columns", "format inline requires columns."))?;
                if c.values().map(|v| v.len()).sum::<usize>() > ctx.config().limits.max_inline_values {
                    return Err(Error::new(ErrorCode::ResourceLimit, "Inline columns exceed max_inline_values; use a CSV file.").at("/columns"));
                }
                Columns { names: c.keys().cloned().collect(), data: c.values().cloned().collect() }
            };
            let meta = json!({ "source": format!("{:?}", req.format).to_lowercase(), "path": req.path });
            let iso = from_columns(&cols, &req, &req.magnitude_columns, system, meta)?;
            store_isochrone(ctx, &iso, json!({}))
        }
        IsoFormat::Parsec => {
            let path = req.path.as_ref().ok_or_else(|| Error::invalid("/path", "format parsec requires path."))?;
            let text = ctx.read_text(path, "/path")?;
            let (names, data) = mist::parse_parsec(&text)?;
            let mut keep: Vec<bool> = vec![true; data[0].len()];
            let col = |n: &str| -> Result<&Vec<f64>> { names.iter().position(|x| x == n).map(|i| &data[i]).ok_or_else(|| Error::new(ErrorCode::DataCorrupt, format!("PARSEC table has no '{n}' column."))) };
            if let Some(a) = req.log_age {
                let ages = col("logAge")?;
                for (k, v) in keep.iter_mut().zip(ages) {
                    *k &= (v - a).abs() < 1e-5;
                }
                if !keep.iter().any(|k| *k) {
                    return Err(Error::domain("/log_age", format!("log_age = {a} not found in the PARSEC table.")));
                }
            }
            if let Some(z) = req.zini {
                let zs = col("Zini")?;
                for (k, v) in keep.iter_mut().zip(zs) {
                    *k &= (v - z).abs() < 1e-8;
                }
                if !keep.iter().any(|k| *k) {
                    return Err(Error::domain("/zini", format!("Zini = {z} not found in the PARSEC table.")));
                }
            }
            let filt_idx = match req.num_filters {
                Some(n) => names.len().checked_sub(n).ok_or_else(|| Error::invalid("/num_filters", "num_filters exceeds the column count."))?,
                None => names.iter().position(|n| n == "mbolmag").ok_or_else(|| Error::invalid("/num_filters", "No 'mbolmag' column; give num_filters."))? + 1,
            };
            let sel = |c: &Vec<f64>| -> Vec<f64> { c.iter().zip(&keep).filter(|(_, k)| **k).map(|(v, _)| *v).collect() };
            let cols = Columns { names: names.clone(), data: data.iter().map(sel).collect() };
            let mag_cols: IndexMap<String, String> = names[filt_idx..].iter().map(|n| (n.clone(), n.clone())).collect();
            let mut r2 = ImportRequest { initial_mass_column: "Mini".into(), current_mass_column: "Mass".into(), log_l_column: Some("logL".into()), log_teff_column: Some("logTe".into()), ..clone_req(&req) };
            if !req.magnitude_columns.is_empty() {
                r2.magnitude_columns = req.magnitude_columns.clone();
            }
            let mc = if req.magnitude_columns.is_empty() { mag_cols } else { req.magnitude_columns.clone() };
            let system = req.magnitude_system.clone().ok_or_else(|| Error::invalid("/magnitude_system", "State the PARSEC magnitude system (e.g. \"Vega\" or \"AB\")."))?;
            let meta = json!({ "source": "parsec", "path": path, "log_age": req.log_age, "zini": req.zini });
            let iso = from_columns(&cols, &r2, &mc, system, meta)?;
            store_isochrone(ctx, &iso, json!({ "rows_in_file": data[0].len() }))
        }
        IsoFormat::Mist => {
            let path = req.path.as_ref().ok_or_else(|| Error::invalid("/path", "format mist requires path to an .iso.cmd file."))?;
            let age = req.log_age_yr.ok_or_else(|| Error::invalid("/log_age_yr", "format mist requires log_age_yr."))?;
            let text = ctx.read_text(path, "/path")?;
            let (header, _ages, block) = mist::read_nearest_block(&text, age)?;
            let system_label = native_label(&header.photometric_system);
            let filters: Vec<String> = block.columns.iter().filter(|c| catalog().get(c).is_some()).cloned().collect();
            let iso = mist_block_to_isochrone(&block, &filters, system_label, json!({ "source": "mist_iso_cmd", "path": path, "header": header, "selected_log_age": block.log_age }))?;
            store_isochrone(ctx, &iso, json!({ "selected_log_age": block.log_age, "header": header }))
        }
    }
}

fn clone_req(r: &ImportRequest) -> ImportRequest {
    serde_json::from_value(serde_json::to_value(r).unwrap()).unwrap()
}

fn native_label(photometric_system: &str) -> String {
    // e.g. "LSST (AB)" / "HST_WFC3 (Vega)"
    photometric_system.split('(').nth(1).map(|s| s.trim_end_matches(')').trim().to_string()).unwrap_or_else(|| "native".into())
}

fn mist_block_to_isochrone(block: &mist::Block, filters: &[String], system: String, meta: Value) -> Result<Isochrone> {
    let get = |n: &str| -> Result<Vec<f64>> { block.column(n).map(|c| c.to_vec()).ok_or_else(|| Error::new(ErrorCode::DataCorrupt, format!("MIST block has no '{n}' column."))) };
    let known = ["EEP", "initial_mass", "star_mass", "log_L", "log_Teff"];
    let extra: Vec<(String, Vec<f64>)> = block
        .columns
        .iter()
        .zip(&block.data)
        .filter(|(n, _)| !known.contains(&n.as_str()) && !filters.contains(n))
        .map(|(n, d)| (n.clone(), d.clone()))
        .collect();
    let iso = Isochrone {
        initial_mass: get("initial_mass")?,
        current_mass: get("star_mass")?,
        filters: filters.to_vec(),
        mags: filters.iter().map(|f| get(f)).collect::<Result<_>>()?,
        magnitude_system: system,
        eep: Some(get("EEP")?),
        log_l: Some(get("log_L")?),
        log_teff: Some(get("log_Teff")?),
        extra,
        metadata: meta,
    };
    iso.validate()?;
    Ok(iso)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MissingZeropoint {
    /// Fail with MISSING_ZEROPOINT (default).
    #[default]
    Error,
    /// Keep that filter in its native system and report it.
    KeepNative,
}

fn v12() -> String {
    "1.2".into()
}
fn rot04() -> f64 {
    0.4
}
fn ab() -> String {
    "AB".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MistRequest {
    #[serde(default = "v12")]
    pub version: String,
    /// v/v_crit: 0.4 (default) or 0.0.
    #[serde(default = "rot04")]
    pub rotation: f64,
    #[serde(default)]
    pub alpha_enhancement: f64,
    pub log_age_yr: f64,
    pub feh: f64,
    pub photometric_systems: Vec<String>,
    /// AB (default), Vega, ST, or native.
    #[serde(default = "ab")]
    pub magnitude_system: String,
    #[serde(default)]
    pub missing_zeropoint: MissingZeropoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

pub fn mist(ctx: &Ctx, req: MistRequest) -> Result<OpOutput> {
    let iso = mist_isochrone(ctx, &req)?;
    let (age, index) = (iso.metadata["grid_log_age_yr"].clone(), iso.metadata["grid_age_index"].clone());
    store_isochrone(ctx, &iso, json!({ "grid_log_age_yr": age, "grid_age_index": index, "model": iso.metadata.clone() }))
}

/// Select (and if needed interpolate/join) a MIST isochrone from registered data.
pub fn mist_isochrone(ctx: &Ctx, req: &MistRequest) -> Result<Isochrone> {
    if req.version != "1.2" {
        return Err(Error::invalid("/version", "Only MIST v1.2 is supported (ArtPop parity).").with_details(json!({ "supported": ["1.2"] })));
    }
    if req.alpha_enhancement != 0.0 {
        return Err(Error::invalid("/alpha_enhancement", "Only [a/Fe] = 0.0 grids are supported."));
    }
    if !mist::ROTATIONS.contains(&req.rotation) {
        return Err(Error::invalid("/rotation", "Rotation must be 0.0 or 0.4."));
    }
    if req.photometric_systems.is_empty() {
        return Err(Error::invalid("/photometric_systems", "Give at least one photometric system."));
    }
    let target = if req.magnitude_system.eq_ignore_ascii_case("native") { None } else { Some(MagSystem::parse(&req.magnitude_system, "/magnitude_system")?) };
    let age_index = mist::age_index(req.log_age_yr)?;
    let nominal_age = mist::nominal_age(age_index);
    let grid = mist::feh_grid();
    let (lo, hi) = (grid[0], grid[grid.len() - 1]);
    if !(req.feh >= lo && req.feh <= hi) {
        return Err(Error::domain("/feh", format!("[Fe/H] = {} is outside the supported grid [{lo}, {hi}].", req.feh)).with_details(json!({ "grid": grid })));
    }
    let cat = catalog();
    let mut combined: Option<Isochrone> = None;
    let mut provenance = Vec::new();
    let mut zp_notes = Vec::new();
    for (si, sys) in req.photometric_systems.iter().enumerate() {
        let p = format!("/photometric_systems/{si}");
        let system = cat.canonical_system(sys).ok_or_else(|| Error::invalid(&p, format!("Unknown photometric system '{sys}'.")).with_details(json!({ "available": cat.systems })))?.to_string();
        let manifest = data::load_manifest(ctx, &req.data_dir, req.rotation, &system, &p)?;
        ctx.note_data(manifest.manifest_id.clone());
        let read_block = |feh: f64| -> Result<(mist::Header, mist::Block)> {
            let text = data::read_verified(&manifest, &mist::file_name(feh, req.rotation, &system))?;
            let (h, _ages, b) = mist::read_nearest_block(&text, nominal_age)?;
            if (h.v_vcrit - req.rotation).abs() > 1e-9 {
                return Err(Error::new(ErrorCode::ModelAlignment, "MIST file rotation differs from the request."));
            }
            Ok((h, b))
        };
        let (block, interp) = if grid.contains(&req.feh) {
            let (_, b) = read_block(req.feh)?;
            (b, Value::Null)
        } else {
            let i = grid.partition_point(|g| *g < req.feh);
            let (flo, fhi) = (grid[i - 1], grid[i]);
            let (_, b0) = read_block(flo)?;
            let (_, b1) = read_block(fhi)?;
            let (b, info) = mist::interpolate_feh(&b0, &b1, flo, fhi, req.feh)?;
            if info.eep_mismatched_rows > 0 {
                ctx.warn("EEP_MISALIGNED", format!("{} rows have different EEPs at [Fe/H] {flo} and {fhi}; rows were interpolated by position as in ArtPop 0.1.2.", info.eep_mismatched_rows));
            }
            if info.extrapolated_tail_rows > 0 {
                ctx.warn("EXTRAPOLATED_TAIL", format!("{} rows of the shorter table were extended with the longer table's tail increments.", info.extrapolated_tail_rows));
            }
            (b, serde_json::to_value(info).unwrap())
        };
        let filters: Vec<String> = cat.system_filters(&system).unwrap().iter().map(|f| f.name.clone()).collect();
        let mut iso = mist_block_to_isochrone(&block, &filters, String::new(), Value::Null)?;
        // Magnitude system conversion per filter.
        let mut system_label = String::from("native");
        if let Some(t) = target {
            system_label = t.name().to_string();
            for (fi, f) in iso.filters.clone().iter().enumerate() {
                match cat.native_system(f).and_then(|native| cat.offset(f, native, t)) {
                    Ok(off) => {
                        for v in &mut iso.mags[fi] {
                            *v += off;
                        }
                    }
                    Err(e) => match req.missing_zeropoint {
                        MissingZeropoint::Error => return Err(e.at(&p)),
                        MissingZeropoint::KeepNative => zp_notes.push(f.clone()),
                    },
                }
            }
        }
        iso.magnitude_system = system_label;
        provenance.push(json!({ "photometric_system": system, "dataset_id": manifest.dataset_id, "manifest_id": manifest.manifest_id, "block_log_age": block.log_age, "feh_interpolation": interp }));
        combined = Some(match combined {
            None => iso,
            Some(mut base) => {
                // Join systems only on identical evolutionary points.
                let same = base.len() == iso.len()
                    && base.initial_mass.iter().zip(&iso.initial_mass).all(|(a, b)| (a - b).abs() <= 1e-12 * a.abs().max(1.0))
                    && base.eep.as_ref().zip(iso.eep.as_ref()).is_none_or(|(a, b)| a == b);
                if !same {
                    return Err(Error::new(ErrorCode::ModelAlignment, format!("MIST tables for {} and {} do not share identical EEPs/initial masses.", req.photometric_systems[0], system)).at(&p));
                }
                for (f, m) in iso.filters.into_iter().zip(iso.mags) {
                    if base.filters.contains(&f) {
                        return Err(Error::invalid(&p, format!("Filter {f} appears in more than one requested system.")));
                    }
                    base.filters.push(f);
                    base.mags.push(m);
                }
                base
            }
        });
    }
    if !zp_notes.is_empty() {
        ctx.warn("MISSING_ZEROPOINT", format!("Kept native magnitudes for filters without conversions: {zp_notes:?}."));
    }
    let mut iso = combined.unwrap();
    iso.metadata = json!({
        "source": "mist",
        "version": "1.2",
        "rotation": req.rotation,
        "alpha_enhancement": 0.0,
        "requested_log_age_yr": req.log_age_yr,
        "grid_age_index": age_index,
        "grid_log_age_yr": nominal_age,
        "feh": req.feh,
        "photometric_systems": provenance,
        "magnitude_system": iso.magnitude_system,
        "native_filters_kept": zp_notes,
    });
    Ok(iso)
}

/// Store an isochrone artifact and return its reference (used by other ops).
pub fn put_isochrone(ctx: &Ctx, iso: &Isochrone) -> Result<crate::store::ArtifactRef> {
    let (meta, members) = isochrone_bundle(iso);
    ctx.put("isochrone", meta, members, vec![])
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub artifact_id: String,
    /// Return rows of these columns (paged); omit for a summary only.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub page: Page,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let mut out = json!({
        "rows": iso.len(),
        "filters": iso.filters,
        "magnitude_system": iso.magnitude_system,
        "mass_range_msun": [iso.m_min(), iso.m_max()],
        "columns": iso.column_names(),
        "has_eep": iso.eep.is_some(),
        "model": iso.metadata,
    });
    if !req.columns.is_empty() {
        let (start, limit) = req.page.resolve(ctx.config().limits.max_page_rows)?;
        let cols: Vec<&[f64]> = req
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| iso.column(c).ok_or_else(|| Error::invalid(&format!("/columns/{i}"), format!("Unknown column '{c}'.")).with_details(json!({ "available": iso.column_names() }))))
            .collect::<Result<_>>()?;
        let end = (start + limit).min(iso.len());
        let rows: Vec<Vec<f64>> = (start..end).map(|r| cols.iter().map(|c| c[r]).collect()).collect();
        out["page"] = json!({ "columns": req.columns, "rows": rows, "start": start, "next_cursor": if end < iso.len() { json!(format!("row:{end}")) } else { Value::Null } });
    }
    Ok(OpOutput::new(out))
}

fn linear() -> InterpKind {
    InterpKind::Linear
}
fn err_bounds() -> Bounds {
    Bounds::Error
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterpolateRequest {
    pub artifact_id: String,
    pub y_column: String,
    #[serde(default = "init_col")]
    pub x_column: String,
    pub x_values: Vec<f64>,
    #[serde(default = "linear")]
    pub kind: InterpKind,
    /// Integer spline order (overrides `kind`).
    #[serde(default)]
    pub order: Option<usize>,
    #[serde(default = "err_bounds")]
    pub bounds: Bounds,
    /// Restrict to rows `[start, stop)` with `step`.
    #[serde(default)]
    pub rows: Option<[usize; 3]>,
}

pub fn interpolate(ctx: &Ctx, req: InterpolateRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let get = |c: &str, p: &str| iso.column(c).map(|v| v.to_vec()).ok_or_else(|| Error::invalid(p, format!("Unknown column '{c}'.")).with_details(json!({ "available": iso.column_names() })));
    let (mut x, mut y) = (get(&req.x_column, "/x_column")?, get(&req.y_column, "/y_column")?);
    if let Some([start, stop, step]) = req.rows {
        if step == 0 || start >= stop || stop > x.len() {
            return Err(Error::invalid("/rows", "rows must be [start, stop, step] within the table with step > 0."));
        }
        x = x[start..stop].iter().step_by(step).copied().collect();
        y = y[start..stop].iter().step_by(step).copied().collect();
    }
    if req.x_values.len() > ctx.config().limits.max_inline_values {
        return Err(Error::new(ErrorCode::ResourceLimit, "Too many x_values for an inline request.").at("/x_values"));
    }
    let f = match req.order {
        Some(k) => Interp1d::with_order(&x, &y, k, req.bounds)?,
        None => Interp1d::new(&x, &y, req.kind, req.bounds)?,
    };
    let vals = req.x_values.iter().enumerate().map(|(i, v)| f.eval(*v).map_err(|e| Error { path: Some(format!("/x_values/{i}")), ..e })).collect::<Result<Vec<_>>>()?;
    Ok(OpOutput::new(json!({ "x_column": req.x_column, "y_column": req.y_column, "kind": req.kind, "order": req.order, "values": vals, "domain": [x[0], x[x.len() - 1]] })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvertRequest {
    pub artifact_id: String,
    pub band: String,
    pub magnitudes: Vec<f64>,
}

pub fn invert(ctx: &Ctx, req: InvertRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    iso.filter_index(&req.band).map_err(|e| e.at("/band"))?;
    let mut out = Vec::new();
    for (i, m) in req.magnitudes.iter().enumerate() {
        let roots = iso.mag_to_mass(*m, &req.band).map_err(|e| Error { path: Some(format!("/magnitudes/{i}")), ..e })?;
        out.push(json!({ "magnitude": m, "crossings": roots, "minimum_initial_mass": roots.iter().map(|c| c.initial_mass).fold(f64::INFINITY, f64::min) }));
    }
    Ok(OpOutput::new(json!({ "band": req.band, "results": out })))
}

fn ten_pc() -> Quantity {
    Quantity::new(10.0, "pc")
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagLimitRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub imf: ImfSpec,
    pub band: String,
    #[serde(default)]
    pub frac_num_sampled: Option<f64>,
    #[serde(default)]
    pub frac_mass_sampled: Option<f64>,
    #[serde(default = "ten_pc")]
    pub distance: Quantity,
}

pub fn magnitude_limit(ctx: &Ctx, req: MagLimitRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let imf = req.imf.build()?;
    let (frac, moment) = match (req.frac_num_sampled, req.frac_mass_sampled) {
        (Some(f), None) => (f, Moment::Number),
        (None, Some(f)) => (f, Moment::Mass),
        _ => return Err(Error::invalid("/frac_num_sampled", "Give exactly one of frac_num_sampled or frac_mass_sampled.")),
    };
    let d = crate::units::length_pc(&req.distance, "/distance")?;
    let dm = artpop_core::population::distance_modulus(d);
    let (mag, mass) = iso.magnitude_limit(&imf, &req.band, frac, moment, dm)?;
    Ok(OpOutput::new(json!({ "mag_limit": mag, "mass_limit_msun": mass, "band": req.band, "fraction": frac, "fraction_kind": moment, "distance_modulus": dm, "method": "500-point logarithmic tabulation, linear inversion (ArtPop 0.1.2)" })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NearestRequest {
    pub artifact_id: String,
    pub masses: Vec<f64>,
}

pub fn nearest(ctx: &Ctx, req: NearestRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let out: Vec<Value> = req.masses.iter().map(|m| {
        let (v, i) = iso.nearest(*m);
        json!({ "query": m, "initial_mass": v, "row": i })
    }).collect();
    Ok(OpOutput::new(json!({ "results": out, "tie_rule": "first minimum distance (lowest row)" })))
}

fn mass_norm() -> Moment {
    Moment::Mass
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WeightsRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub imf: ImfSpec,
    #[serde(default = "mass_norm")]
    pub norm_type: Moment,
    #[serde(default)]
    pub m_min_norm: Option<f64>,
    #[serde(default)]
    pub m_max_norm: Option<f64>,
}

pub fn weights(ctx: &Ctx, req: WeightsRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let imf = req.imf.build()?;
    let w = iso.imf_weights(&imf, req.m_min_norm, req.m_max_norm, req.norm_type)?;
    if w.len() > ctx.config().limits.max_inline_values {
        return Err(Error::new(ErrorCode::ResourceLimit, "Too many rows to return inline."));
    }
    let lo = req.m_min_norm.unwrap_or(iso.m_min());
    let hi = req.m_max_norm.unwrap_or(iso.m_max());
    Ok(OpOutput::new(json!({ "weights": w, "norm_type": req.norm_type, "normalization_bounds_msun": [lo, hi], "number_integral": imf.number(lo, hi), "mass_integral": imf.mass(lo, hi), "sum_weights": w.iter().sum::<f64>() })))
}

fn default_metrics() -> Vec<String> {
    vec!["ssp_mag".into(), "sbf_mag".into(), "mean_star_mass".into(), "surviving_mass".into()]
}
fn upper() -> f64 {
    artpop_core::isochrone::DEFAULT_IMF_UPPER_MASS
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegratedRequest {
    pub artifact_id: String,
    #[serde(default)]
    pub imf: ImfSpec,
    #[serde(default)]
    pub bands: Vec<String>,
    /// Pairs `[blue, red]`.
    #[serde(default)]
    pub colors: Vec<[String; 2]>,
    /// Any of ssp_mag, sbf_mag, mean_star_mass, surviving_mass.
    #[serde(default = "default_metrics")]
    pub metrics: Vec<String>,
    /// Normalization of ssp_mag: per unit mass (default) or per star.
    #[serde(default = "mass_norm")]
    pub norm_type: Moment,
    #[serde(default)]
    pub remnants: Option<RemnantModel>,
    /// Upper IMF mass for surviving-mass normalization (ArtPop default 120).
    #[serde(default = "upper")]
    pub imf_upper_mass: f64,
}

pub fn integrated(ctx: &Ctx, req: IntegratedRequest) -> Result<OpOutput> {
    let iso = load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
    let imf = req.imf.build()?;
    let bands = if req.bands.is_empty() { iso.filters.clone() } else { req.bands.clone() };
    for (i, b) in bands.iter().enumerate() {
        iso.filter_index(b).map_err(|e| e.at(&format!("/bands/{i}")))?;
    }
    let mut out = serde_json::Map::new();
    for m in &req.metrics {
        match m.as_str() {
            "ssp_mag" => {
                let v: IndexMap<String, f64> = bands.iter().map(|b| Ok((b.clone(), iso.ssp_mag(b, &imf, req.norm_type)?))).collect::<Result<_>>()?;
                out.insert("ssp_mag".into(), json!(v));
            }
            "sbf_mag" => {
                let v: IndexMap<String, f64> = bands.iter().map(|b| Ok((b.clone(), iso.ssp_sbf_mag(b, &imf)?))).collect::<Result<_>>()?;
                out.insert("sbf_mag".into(), json!(v));
            }
            "mean_star_mass" => {
                out.insert("mean_star_mass_msun".into(), json!(iso.ssp_mean_star_mass(&imf)?));
            }
            "surviving_mass" => {
                let rem = req.remnants.unwrap_or_default();
                let with = iso.ssp_surviving_mass(&imf, None, Some(req.imf_upper_mass), Some(&rem))?;
                let without = iso.ssp_surviving_mass(&imf, None, Some(req.imf_upper_mass), None)?;
                out.insert("surviving_mass".into(), json!({ "with_remnants": with, "live_only": without.total, "remnant_model": rem }));
            }
            other => return Err(Error::invalid("/metrics", format!("Unknown metric '{other}'.")).with_details(json!({ "available": default_metrics() }))),
        }
    }
    if !req.colors.is_empty() {
        let mut c = IndexMap::new();
        for (i, [b, r]) in req.colors.iter().enumerate() {
            let v = iso.ssp_color(b, r, &imf).map_err(|e| e.at(&format!("/colors/{i}")))?;
            c.insert(format!("{b}-{r}"), v);
        }
        out.insert("colors".into(), json!(c));
    }
    out.insert("imf".into(), json!(imf));
    out.insert("norm_type".into(), json!(req.norm_type));
    Ok(OpOutput::new(Value::Object(out)))
}
