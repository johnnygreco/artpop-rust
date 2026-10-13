//! Conversions between scientific types and stored artifact bundles.

use crate::Ctx;
use crate::store::{Loaded, Member};
use crate::types::Geometry;
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::Image;
use artpop_core::isochrone::Isochrone;
use artpop_core::population::{Catalog, Component, Population};
use artpop_core::spatial::{Model, Positions};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn is_identity(v: &[u64]) -> bool {
    v.iter().enumerate().all(|(i, x)| *x == i as u64)
}

/// Read an optional u64 column, defaulting to the identity `0..n`.
fn u64_or_identity(l: &Loaded, name: &str, n: usize) -> Result<Vec<u64>> {
    if l.has(name) { l.u64s(name) } else { Ok((0..n as u64).collect()) }
}

/// Read an optional u32 column, defaulting to all zeros.
fn u32_or_zero(l: &Loaded, name: &str, n: usize) -> Result<Vec<u32>> {
    if l.has(name) { l.u32s(name) } else { Ok(vec![0; n]) }
}

pub fn image_member(name: &str, img: &Image, unit: &str) -> Member {
    Member::f64(name, &img.data, vec![img.height, img.width], Some(unit))
}

/// Move an image into a member without copying its pixels.
pub fn image_member_owned(name: &str, img: Image, unit: &str) -> Member {
    let shape = vec![img.height, img.width];
    Member::f64_owned(name, img.data, shape, Some(unit))
}

pub fn read_image(l: &Loaded, name: &str) -> Result<Image> {
    let info = l.info(name)?;
    if info.shape.len() != 2 {
        return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{name}' is not two-dimensional.")));
    }
    let data = l.f64s(name)?;
    Image::new(info.shape[1], info.shape[0], data)
}

// ---------------------------------------------------------------- isochrone

pub fn isochrone_bundle(iso: &Isochrone) -> (Value, Vec<Member>) {
    let mut members = vec![Member::column("initial_mass", &iso.initial_mass, Some("Msun")), Member::column("current_mass", &iso.current_mass, Some("Msun"))];
    for (f, col) in iso.filters.iter().zip(&iso.mags) {
        members.push(Member::column(&format!("mag:{f}"), col, Some("mag")));
    }
    for (name, col) in [("eep", &iso.eep), ("log_L", &iso.log_l), ("log_Teff", &iso.log_teff)] {
        if let Some(c) = col {
            members.push(Member::column(name, c, None));
        }
    }
    for (name, col) in &iso.extra {
        members.push(Member::column(&format!("extra:{name}"), col, None));
    }
    let meta = json!({
        "filters": iso.filters,
        "magnitude_system": iso.magnitude_system,
        "rows": iso.len(),
        "mass_range_msun": [iso.m_min(), iso.m_max()],
        "has_eep": iso.eep.is_some(),
        "has_log_L": iso.log_l.is_some(),
        "has_log_Teff": iso.log_teff.is_some(),
        "extra_columns": iso.extra.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        "model": iso.metadata,
    });
    (meta, members)
}

pub fn load_isochrone(ctx: &Ctx, id: &str, pointer: &str) -> Result<Isochrone> {
    let l = ctx.store().load(id, pointer, &["isochrone"])?;
    let filters: Vec<String> = l.meta("filters")?;
    let extra: Vec<String> = l.meta("extra_columns")?;
    let opt = |name: &str| -> Result<Option<Vec<f64>>> { if l.has(name) { Ok(Some(l.f64s(name)?)) } else { Ok(None) } };
    let mut names = vec!["initial_mass".to_string(), "current_mass".to_string()];
    names.extend(filters.iter().map(|f| format!("mag:{f}")));
    let mut cols = l.f64s_many(&names)?.into_iter();
    let iso = Isochrone {
        initial_mass: cols.next().unwrap(),
        current_mass: cols.next().unwrap(),
        mags: cols.collect(),
        filters,
        magnitude_system: l.meta("magnitude_system")?,
        eep: opt("eep")?,
        log_l: opt("log_L")?,
        log_teff: opt("log_Teff")?,
        extra: extra.iter().map(|n| Ok((n.clone(), l.f64s(&format!("extra:{n}"))?))).collect::<Result<_>>()?,
        metadata: l.manifest.metadata.get("model").cloned().unwrap_or(Value::Null),
    };
    iso.validate()?;
    Ok(iso)
}

// ---------------------------------------------------------------- population

pub fn population_bundle(p: &Population) -> (Value, Vec<Member>) {
    let c = &p.catalog;
    // Identity star IDs and all-zero component columns are implied, not stored.
    let mut members = vec![Member::column("initial_mass", &c.initial_mass, Some("Msun")), Member::column("current_mass", &c.current_mass, Some("Msun"))];
    if !is_identity(&c.star_id) {
        members.push(Member::u64("star_id", &c.star_id));
    }
    if c.component.iter().any(|v| *v != 0) {
        members.push(Member::u32("component", &c.component));
    }
    for (f, col) in p.filters.iter().zip(&c.abs_mags) {
        members.push(Member::column(&format!("abs_mag:{f}"), col, Some("mag")));
    }
    for (name, col) in [("eep", &c.eep), ("log_L", &c.log_l), ("log_Teff", &c.log_teff)] {
        if let Some(v) = col {
            members.push(Member::column(name, v, None));
        }
    }
    let meta = json!({
        "filters": p.filters,
        "magnitude_system": p.magnitude_system,
        "num_stars": c.len(),
        "components": p.components,
        "has_eep": c.eep.is_some(),
    });
    (meta, members)
}

/// Store a population, moving its columns into the members (no copies).
pub fn population_bundle_owned(p: Population) -> (Value, Vec<Member>) {
    let Population { filters, magnitude_system, components, catalog } = p;
    let n = catalog.len();
    let meta = json!({
        "filters": filters,
        "magnitude_system": magnitude_system,
        "num_stars": n,
        "components": components,
        "has_eep": catalog.eep.is_some(),
    });
    let Catalog { star_id, component, initial_mass, current_mass, abs_mags, eep, log_l, log_teff } = catalog;
    let mut members = vec![Member::column_owned("initial_mass", initial_mass, Some("Msun")), Member::column_owned("current_mass", current_mass, Some("Msun"))];
    if !is_identity(&star_id) {
        members.push(Member::u64("star_id", &star_id));
    }
    if component.iter().any(|v| *v != 0) {
        members.push(Member::u32("component", &component));
    }
    for (f, col) in filters.iter().zip(abs_mags) {
        members.push(Member::column_owned(&format!("abs_mag:{f}"), col, Some("mag")));
    }
    for (name, col) in [("eep", eep), ("log_L", log_l), ("log_Teff", log_teff)] {
        if let Some(v) = col {
            members.push(Member::column_owned(name, v, None));
        }
    }
    (meta, members)
}

/// Population metadata without any catalog column.
#[derive(Debug, Clone)]
pub struct PopulationHeader {
    pub filters: Vec<String>,
    pub magnitude_system: String,
    pub components: Vec<Component>,
    pub num_stars: usize,
    pub has_eep: bool,
}

impl PopulationHeader {
    pub fn filter_index(&self, band: &str, pointer: &str) -> Result<usize> {
        self.filters.iter().position(|f| f == band).ok_or_else(|| Error::unknown_filter(pointer, band, &self.filters))
    }
}

/// A population whose columns are read on demand (each read is digest-verified).
pub struct LazyPopulation {
    pub loaded: Loaded,
    pub header: PopulationHeader,
}

impl LazyPopulation {
    pub fn load(ctx: &Ctx, id: &str, pointer: &str) -> Result<LazyPopulation> {
        let l = ctx.store().load(id, pointer, &["population"])?;
        let header = PopulationHeader { filters: l.meta("filters")?, magnitude_system: l.meta("magnitude_system")?, components: l.meta("components")?, num_stars: l.meta("num_stars")?, has_eep: l.meta("has_eep")? };
        Ok(LazyPopulation { loaded: l, header })
    }

    pub fn len(&self) -> usize {
        self.header.num_stars
    }

    pub fn is_empty(&self) -> bool {
        self.header.num_stars == 0
    }

    /// Absolute magnitudes of one band.
    pub fn abs_mags(&self, band: usize) -> Result<Vec<f64>> {
        let v = self.loaded.f64s(&format!("abs_mag:{}", self.header.filters[band]))?;
        self.check_len(v.len())?;
        Ok(v)
    }

    /// Component ids (None when every star belongs to component 0).
    pub fn component(&self) -> Result<Option<Vec<u32>>> {
        if self.loaded.has("component") {
            let v = self.loaded.u32s("component")?;
            self.check_len(v.len())?;
            if v.iter().any(|c| *c as usize >= self.header.components.len()) {
                return Err(Error::new(ErrorCode::DataCorrupt, "Population catalog references an unknown component."));
            }
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }

    pub fn eep(&self) -> Result<Option<Vec<f64>>> {
        if self.loaded.has("eep") {
            let v = self.loaded.f64s("eep")?;
            self.check_len(v.len())?;
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }

    fn check_len(&self, n: usize) -> Result<()> {
        if n != self.header.num_stars {
            return Err(Error::new(ErrorCode::DataCorrupt, "Population catalog columns have inconsistent lengths."));
        }
        Ok(())
    }
}

pub fn load_population(ctx: &Ctx, id: &str, pointer: &str) -> Result<Population> {
    let l = ctx.store().load(id, pointer, &["population"])?;
    let filters: Vec<String> = l.meta("filters")?;
    let components: Vec<Component> = l.meta("components")?;
    let opt = |name: &str| -> Result<Option<Vec<f64>>> { if l.has(name) { Ok(Some(l.f64s(name)?)) } else { Ok(None) } };
    let n: usize = l.meta("num_stars")?;
    let mut names = vec!["initial_mass".to_string(), "current_mass".to_string()];
    names.extend(filters.iter().map(|f| format!("abs_mag:{f}")));
    let mut cols = l.f64s_many(&names)?.into_iter();
    let catalog = Catalog {
        star_id: u64_or_identity(&l, "star_id", n)?,
        component: u32_or_zero(&l, "component", n)?,
        initial_mass: cols.next().unwrap(),
        current_mass: cols.next().unwrap(),
        abs_mags: cols.collect(),
        eep: opt("eep")?,
        log_l: opt("log_L")?,
        log_teff: opt("log_Teff")?,
    };
    let n = catalog.initial_mass.len();
    if catalog.star_id.len() != n || catalog.component.len() != n || catalog.current_mass.len() != n || catalog.abs_mags.iter().any(|c| c.len() != n) {
        return Err(Error::new(ErrorCode::DataCorrupt, "Population catalog columns have inconsistent lengths."));
    }
    if catalog.component.iter().any(|c| *c as usize >= components.len()) {
        return Err(Error::new(ErrorCode::DataCorrupt, "Population catalog references an unknown component."));
    }
    Ok(Population { filters, magnitude_system: l.meta("magnitude_system")?, components, catalog })
}

// ---------------------------------------------------------------- positions

pub fn positions_bundle(p: Positions, geometry: &Geometry, extra: Value) -> (Value, Vec<Member>) {
    let meta = json!({
        "geometry": geometry,
        "generated": p.generated,
        "retained": p.x.len(),
        "outside": p.outside_count(),
        "sampling": extra,
    });
    let n = p.x.len();
    let mut members = vec![Member::bools("inside", &p.inside)];
    if let Some(index) = &p.index
        && !is_identity(index)
    {
        members.push(Member::u64("index", index));
    }
    members.push(Member::f64_owned("x", p.x, vec![n], Some("pixel")));
    members.push(Member::f64_owned("y", p.y, vec![n], Some("pixel")));
    (meta, members)
}

pub fn load_positions(ctx: &Ctx, id: &str, pointer: &str) -> Result<(Positions, Geometry)> {
    let l = ctx.store().load(id, pointer, &["positions"])?;
    let mut xy = l.f64s_many(&["x".into(), "y".into()])?.into_iter();
    let (x, y) = (xy.next().unwrap(), xy.next().unwrap());
    let index = if l.has("index") { Some(l.u64s("index")?) } else { None };
    let inside = l.bools("inside")?;
    if y.len() != x.len() || inside.len() != x.len() || index.as_ref().is_some_and(|v| v.len() != x.len()) {
        return Err(Error::new(ErrorCode::DataCorrupt, "Positions columns have inconsistent lengths."));
    }
    let p = Positions { y, inside, index, generated: l.meta("generated")?, x };
    Ok((p, l.meta("geometry")?))
}

// ---------------------------------------------------------------- source

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SmoothProfile {
    Sersic { r_eff_arcsec: f64, n: f64, ellip: f64, theta_rad: f64 },
    Plummer { scale_radius_arcsec: f64 },
    Constant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmoothComponent {
    pub component_id: u32,
    pub label: String,
    pub profile: SmoothProfile,
    /// Unit-amplitude model in pixel coordinates (amplitude set at render time).
    pub shape: Model,
    /// Apparent integrated magnitude of the unresolved light, per band.
    pub apparent_mag: IndexMap<String, Option<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Labels {
    Numeric { name: String },
    Categorical { name: String, categories: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct SourceData {
    pub geometry: Geometry,
    pub bands: Vec<String>,
    pub magnitude_system: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub inside: Vec<bool>,
    /// Apparent magnitudes `[band][star]`.
    pub mags: Vec<Vec<f64>>,
    pub component: Vec<u32>,
    pub star_index: Vec<u64>,
    pub labels: Option<(Labels, Vec<f64>)>,
    pub smooth: Vec<SmoothComponent>,
    pub components: Vec<Value>,
    pub provenance: Value,
}

impl SourceData {
    pub fn band_index(&self, band: &str) -> Result<usize> {
        self.bands.iter().position(|b| b == band).ok_or_else(|| Error::unknown_filter("/bandpass", band, &self.bands))
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }
}

pub fn source_bundle(s: SourceData) -> (Value, Vec<Member>) {
    let mut members = vec![Member::bools("inside", &s.inside)];
    if s.component.iter().any(|v| *v != 0) {
        members.push(Member::u32("component", &s.component));
    }
    // Identity (including the implied empty form) is never stored.
    if !is_identity(&s.star_index) {
        members.push(Member::u64("star_index", &s.star_index));
    }
    if let Some((_, v)) = &s.labels {
        members.push(Member::column("label", v, None));
    }
    let meta = json!({
        "geometry": s.geometry,
        "bands": s.bands,
        "magnitude_system": s.magnitude_system,
        "num_stars": s.len(),
        "inside": s.inside.iter().filter(|v| **v).count(),
        "labels": s.labels.as_ref().map(|l| &l.0),
        "smooth_components": s.smooth,
        "components": s.components,
        "provenance": s.provenance,
    });
    let n = s.x.len();
    members.push(Member::f64_owned("x", s.x, vec![n], Some("pixel")));
    members.push(Member::f64_owned("y", s.y, vec![n], Some("pixel")));
    for (b, col) in s.bands.iter().zip(s.mags) {
        members.push(Member::f64_owned(&format!("mag:{b}"), col, vec![n], Some("mag")));
    }
    (meta, members)
}

impl SourceData {
    /// Magnitudes of one band; fails if the band was not loaded.
    pub fn band_column(&self, band: &str, pointer: &str) -> Result<&[f64]> {
        let b = self.band_index(band).map_err(|e| e.at(pointer))?;
        let col = &self.mags[b];
        if col.len() != self.x.len() {
            return Err(Error::internal(format!("Band {band} was not loaded for this source.")));
        }
        Ok(col)
    }
}

/// Which source columns to read. Columns that are not requested are left
/// empty, so partial loads must only be used by code that never touches them.
#[derive(Debug, Clone, Default)]
pub struct SourceLoad {
    /// Bands whose magnitudes are needed (None = all).
    pub bands: Option<Vec<String>>,
    pub component: bool,
    pub star_index: bool,
    pub labels: bool,
}

impl SourceLoad {
    pub fn all() -> SourceLoad {
        SourceLoad { bands: None, component: true, star_index: true, labels: true }
    }

    /// Columns needed to render `band` under `selection`.
    pub fn for_render(band: &str, selection: &crate::types::Selection) -> SourceLoad {
        use crate::types::Selection;
        SourceLoad {
            bands: Some(vec![band.to_string()]),
            component: matches!(selection, Selection::Components { .. }),
            star_index: false,
            labels: matches!(selection, Selection::Labels { .. } | Selection::Phases { .. }),
        }
    }
}

pub fn load_source(ctx: &Ctx, id: &str, pointer: &str) -> Result<SourceData> {
    load_source_with(ctx, id, pointer, &SourceLoad::all())
}

/// Load a source reading only the requested columns.
pub fn load_source_with(ctx: &Ctx, id: &str, pointer: &str, load: &SourceLoad) -> Result<SourceData> {
    let l = ctx.store().load(id, pointer, &["source"])?;
    let bands: Vec<String> = l.meta("bands")?;
    let labels: Option<Labels> = l.meta("labels")?;
    let labels = match (labels, load.labels) {
        (Some(lab), true) => Some((lab, l.f64s("label")?)),
        _ => None,
    };
    let n: usize = l.meta("num_stars")?;
    // Unknown requested bands are simply not loaded; handlers report them at
    // their own request pointer.
    let wanted: Vec<bool> = bands.iter().map(|b| load.bands.as_ref().is_none_or(|w| w.contains(b))).collect();
    let mut names = vec!["x".to_string(), "y".to_string()];
    names.extend(bands.iter().zip(&wanted).filter(|(_, w)| **w).map(|(b, _)| format!("mag:{b}")));
    let mut cols = l.f64s_many(&names)?.into_iter();
    let x = cols.next().unwrap();
    let y = cols.next().unwrap();
    let mags: Vec<Vec<f64>> = wanted.iter().map(|w| if *w { cols.next().unwrap() } else { Vec::new() }).collect();
    let s = SourceData {
        geometry: l.meta("geometry")?,
        magnitude_system: l.meta("magnitude_system")?,
        x,
        y,
        inside: l.bools("inside")?,
        mags,
        component: if load.component { u32_or_zero(&l, "component", n)? } else { Vec::new() },
        star_index: if load.star_index { u64_or_identity(&l, "star_index", n)? } else { Vec::new() },
        bands,
        labels,
        smooth: l.meta("smooth_components")?,
        components: l.meta("components")?,
        provenance: l.manifest.metadata.get("provenance").cloned().unwrap_or(Value::Null),
    };
    let n = s.x.len();
    if s.y.len() != n || s.inside.len() != n || (load.component && s.component.len() != n) || (load.star_index && s.star_index.len() != n) || s.mags.iter().zip(&wanted).any(|(m, w)| *w && m.len() != n) {
        return Err(Error::new(ErrorCode::DataCorrupt, "Source columns have inconsistent lengths."));
    }
    Ok(s)
}

/// Label value as JSON (category name or number).
pub fn label_json(labels: &Labels, v: f64) -> Value {
    match labels {
        Labels::Numeric { .. } => artpop_core::error::finite_or_null(v),
        Labels::Categorical { categories, .. } => categories.get(v as usize).map_or(Value::Null, |c| Value::from(c.clone())),
    }
}
