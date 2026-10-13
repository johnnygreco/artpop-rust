//! Stellar populations: discrete sampled stars plus unresolved moments.
//!
//! A population is a list of components (one per simple stellar population)
//! and a structure-of-arrays catalog of discrete stars. Each component keeps
//! its own distance, per-band extinction, and unresolved light as expected
//! count, current mass, and per-band first/second **absolute** flux moments
//! (flux at 10 pc, magnitude zero point 0). Apparent first moments scale by
//! `10^(-0.4 (DM + A_b))` and second moments by its square, so surface
//! brightness fluctuations respond to extinction consistently.

use crate::error::{Error, ErrorCode, Result};
use crate::imf::{self, Imf, Moment, SamplingAlgorithm, StoppingPolicy};
use crate::isochrone::{Isochrone, RemnantModel};
use crate::rng::Seed;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CutoffFrame {
    /// Apparent magnitude including extinction (rust-v1 default).
    Observed,
    /// Apparent magnitude without extinction (ArtPop 0.1.2 convention).
    IntrinsicApparent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MagLimit {
    pub magnitude: f64,
    pub band: String,
    pub frame: CutoffFrame,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Normalization {
    Count { value: u64 },
    CurrentMass { value: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingRegime {
    /// Every star sampled individually.
    Discrete,
    /// Bright stars sampled; faint stars integrated.
    Partial,
    /// The limit is brighter than every star: everything is unresolved.
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampleFraction {
    pub regime: SamplingRegime,
    pub mass_limit: f64,
    pub frac_num_sampled: f64,
    pub frac_mass_sampled: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Unresolved {
    /// Expected number of stars represented by the moments.
    pub expected_count: f64,
    pub current_mass: f64,
    /// Per band: sum of absolute fluxes, `Σ 10^(-0.4 M)`.
    pub flux1_abs: Vec<f64>,
    /// Per band: sum of squared absolute fluxes, `Σ 10^(-0.8 M)`.
    pub flux2_abs: Vec<f64>,
}

impl Unresolved {
    pub fn is_empty(&self) -> bool {
        self.expected_count == 0.0 && self.flux1_abs.iter().all(|f| *f == 0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Component {
    pub id: u32,
    pub label: String,
    /// Isochrone identity (artifact id and model metadata) for provenance.
    pub isochrone: serde_json::Value,
    pub imf: Imf,
    pub distance_pc: f64,
    /// Extinction in magnitudes for each population filter.
    pub extinction: Vec<f64>,
    pub mag_limit: Option<MagLimit>,
    pub sample_fraction: SampleFraction,
    pub normalization: Normalization,
    pub add_remnants: bool,
    pub remnant_model: RemnantModel,
    pub remnants_factor: f64,
    /// Formed mass including remnants (ArtPop `total_mass`).
    pub total_mass: f64,
    /// Current mass in living stars, discrete plus unresolved.
    pub live_star_mass: f64,
    /// Current mass of discrete stars.
    pub sampled_mass: f64,
    /// Sum of discrete initial masses (not the full formed mass).
    pub discrete_initial_mass: f64,
    pub num_discrete: u64,
    pub unresolved: Unresolved,
    pub sampling: serde_json::Value,
}

impl Component {
    pub fn distance_modulus(&self) -> f64 {
        5.0 * self.distance_pc.log10() - 5.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Catalog {
    pub star_id: Vec<u64>,
    pub component: Vec<u32>,
    pub initial_mass: Vec<f64>,
    pub current_mass: Vec<f64>,
    /// Absolute magnitudes `[filter][star]`.
    pub abs_mags: Vec<Vec<f64>>,
    pub eep: Option<Vec<f64>>,
    pub log_l: Option<Vec<f64>>,
    pub log_teff: Option<Vec<f64>>,
}

impl Catalog {
    pub fn len(&self) -> usize {
        self.initial_mass.len()
    }

    pub fn is_empty(&self) -> bool {
        self.initial_mass.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Population {
    pub filters: Vec<String>,
    pub magnitude_system: String,
    pub components: Vec<Component>,
    pub catalog: Catalog,
}

pub struct BuildSpec<'a> {
    pub isochrone: &'a Isochrone,
    pub isochrone_identity: serde_json::Value,
    pub normalization: Normalization,
    pub imf: Imf,
    pub distance_pc: f64,
    pub extinction: Vec<f64>,
    pub mag_limit: Option<MagLimit>,
    pub add_remnants: bool,
    pub remnant_model: RemnantModel,
    pub mass_tolerance: f64,
    pub algorithm: SamplingAlgorithm,
    pub num_edges: usize,
    pub stopping: StoppingPolicy,
    pub max_stars: u64,
    pub label: String,
}

pub fn distance_modulus(distance_pc: f64) -> f64 {
    5.0 * distance_pc.log10() - 5.0
}

/// Fraction of stars (by number and by initial mass) brighter than the limit.
pub fn sample_fraction(iso: &Isochrone, imf: &Imf, limit: Option<&MagLimit>, distance_pc: f64, extinction: &[f64]) -> Result<SampleFraction> {
    let (lo, hi) = (iso.m_min(), iso.m_max());
    let full = SampleFraction { regime: SamplingRegime::Discrete, mass_limit: lo, frac_num_sampled: 1.0, frac_mass_sampled: 1.0 };
    let Some(limit) = limit else { return Ok(full) };
    let b = iso.filter_index(&limit.band).map_err(|e| e.at("/mag_limit/band"))?;
    let shift = distance_modulus(distance_pc) + if limit.frame == CutoffFrame::Observed { extinction[b] } else { 0.0 };
    let mags = &iso.mags[b];
    let (min, max) = mags.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, c), v| (a.min(*v), c.max(*v)));
    if limit.magnitude >= max + shift {
        return Ok(full);
    }
    if limit.magnitude <= min + shift {
        return Ok(SampleFraction { regime: SamplingRegime::Unresolved, mass_limit: hi, frac_num_sampled: 0.0, frac_mass_sampled: 0.0 });
    }
    let roots = iso.mag_to_mass(limit.magnitude - shift, &limit.band)?;
    let m_lim = roots.iter().map(|c| c.initial_mass).fold(f64::INFINITY, f64::min);
    Ok(SampleFraction {
        regime: SamplingRegime::Partial,
        mass_limit: m_lim,
        frac_num_sampled: imf.number(m_lim, hi) / imf.number(lo, hi),
        frac_mass_sampled: imf.mass(m_lim, hi) / imf.mass(lo, hi),
    })
}

/// Build one simple stellar population; masses draw from the block streams
/// `("population.mass", key, ...)` of `seed`.
pub fn build(spec: &BuildSpec, seed: Seed, key: &str) -> Result<Population> {
    let iso = spec.isochrone;
    iso.validate()?;
    let nf = iso.filters.len();
    if spec.extinction.len() != nf {
        return Err(Error::new(ErrorCode::ShapeMismatch, "Extinction must have one value per isochrone filter."));
    }
    crate::error::require_positive(spec.distance_pc, "/distance", "Distance")?;
    crate::error::require_finite(&spec.extinction, "/extinction_mag", "extinction")?;
    if !(spec.mass_tolerance > 0.0) || !spec.mass_tolerance.is_finite() {
        return Err(Error::invalid("/mass_tolerance", "mass_tolerance must be a finite positive number."));
    }
    let (iso_lo, iso_hi) = (iso.m_min(), iso.m_max());
    let imf = &spec.imf;
    let remnants_factor = if spec.add_remnants {
        let without = iso.ssp_surviving_mass(imf, None, None, None)?.total;
        let with = iso.ssp_surviving_mass(imf, None, None, Some(&spec.remnant_model))?.total;
        without / with
    } else {
        1.0
    };
    let frac = sample_fraction(iso, imf, spec.mag_limit.as_ref(), spec.distance_pc, &spec.extinction)?;
    let m_lim = frac.mass_limit;
    let mut sampling = serde_json::json!({
        "algorithm": spec.algorithm,
        "num_mass_edges": spec.num_edges,
        "mass_range": [m_lim, iso_hi],
    });

    // Draw discrete initial masses.
    let mut initial: Vec<f64>;
    let mut num_integrated: f64;
    match spec.normalization {
        Normalization::Count { value } => {
            if value == 0 {
                return Err(Error::invalid("/normalization/value", "Star count must be positive."));
            }
            if value > spec.max_stars {
                return Err(Error::new(ErrorCode::ResourceLimit, "Requested star count exceeds the configured limit.")
                    .with_details(serde_json::json!({ "requested": value, "max_stars": spec.max_stars })));
            }
            let n_sample = (value as f64 * frac.frac_num_sampled) as u64;
            initial = if n_sample > 0 {
                let sampler = imf::Sampler::new(imf, m_lim, iso_hi, spec.algorithm, spec.num_edges)?;
                sampler.sample(n_sample as usize, seed, "population.mass", &[key])
            } else {
                Vec::new()
            };
            num_integrated = (value - n_sample) as f64;
            sampling["draws"] = n_sample.into();
        }
        Normalization::CurrentMass { value } => {
            crate::error::require_positive(value, "/normalization/value", "Total mass")?;
            let mass_loss = iso.ssp_surviving_mass(imf, Some(iso_lo), Some(iso_hi), None)?.total;
            let live_initial = value * remnants_factor / mass_loss;
            if frac.regime == SamplingRegime::Unresolved {
                initial = Vec::new();
                num_integrated = live_initial * imf.number(iso_lo, iso_hi) / imf.mass(iso_lo, iso_hi);
                sampling["draws"] = 0.into();
            } else {
                let target = live_initial * frac.frac_mass_sampled;
                let mean_mass = imf.mean_mass(m_lim, iso_hi);
                let batch = ((spec.mass_tolerance * target / mean_mass) as u64).max(1);
                let sampler = imf::Sampler::new(imf, m_lim, iso_hi, spec.algorithm, spec.num_edges)?;
                let out = imf::build_to_mass(&sampler, target, batch, spec.stopping, spec.max_stars, seed, "population.mass", &[key])?;
                sampling["target_initial_mass"] = target.into();
                sampling["realized_initial_mass"] = out.realized_mass.into();
                sampling["batch_size"] = out.batch_size.into();
                sampling["batches"] = out.batches.into();
                sampling["draws"] = out.draws.into();
                sampling["stopping"] = serde_json::to_value(spec.stopping).unwrap();
                sampling["mass_tolerance"] = spec.mass_tolerance.into();
                initial = out.masses;
                let n = initial.len() as f64;
                num_integrated = if frac.frac_num_sampled < 1.0 {
                    (n * (1.0 - frac.frac_num_sampled) / frac.frac_num_sampled).trunc()
                } else {
                    0.0
                };
            }
        }
    }

    // One bracket search per star for every interpolated column.
    let mut cols: Vec<&[f64]> = vec![&iso.current_mass];
    cols.extend(iso.mags.iter().map(|c| c.as_slice()));
    let optional: Vec<&Option<Vec<f64>>> = vec![&iso.eep, &iso.log_l, &iso.log_teff];
    cols.extend(optional.iter().filter_map(|c| c.as_deref()));
    let mut interpolated = iso.interp_columns(&cols, &initial)?.into_iter();
    let mut current = interpolated.next().unwrap();
    let mut abs_mags: Vec<Vec<f64>> = (0..nf).map(|_| interpolated.next().unwrap()).collect();
    let mut extra: Vec<Option<Vec<f64>>> = optional.iter().map(|c| c.as_ref().map(|_| interpolated.next().unwrap())).collect();

    let mut unresolved = Unresolved { flux1_abs: vec![0.0; nf], flux2_abs: vec![0.0; nf], ..Default::default() };
    let sampled_mass: f64;
    let total_mass: f64;
    if frac.regime != SamplingRegime::Discrete {
        let limit = spec.mag_limit.as_ref().expect("limit present");
        let b = iso.filter_index(&limit.band)?;
        let shift = distance_modulus(spec.distance_pc) + if limit.frame == CutoffFrame::Observed { spec.extinction[b] } else { 0.0 };
        // Evolved stars above the mass limit that are still fainter than the
        // limit join the unresolved component.
        let faint: Vec<bool> = abs_mags[b].iter().map(|m| m + shift > limit.magnitude).collect();
        let mut evolved_mass = 0.0;
        let mut evolved = 0u64;
        for (i, f) in faint.iter().enumerate() {
            if *f {
                evolved += 1;
                evolved_mass += current[i];
                for (k, col) in abs_mags.iter().enumerate() {
                    unresolved.flux1_abs[k] += 10f64.powf(-0.4 * col[i]);
                    unresolved.flux2_abs[k] += 10f64.powf(-0.8 * col[i]);
                }
            }
        }
        if evolved > 0 {
            let keep = |v: &mut Vec<f64>| {
                let mut j = 0;
                v.retain(|_| {
                    let k = !faint[j];
                    j += 1;
                    k
                });
            };
            keep(&mut initial);
            keep(&mut current);
            for col in abs_mags.iter_mut().chain(extra.iter_mut().flatten()) {
                keep(col);
            }
        }
        num_integrated += evolved as f64;
        let n_total = num_integrated + initial.len() as f64;
        let w = iso.imf_weights(imf, None, Some(iso_hi), Moment::Number)?;
        let arg = if frac.regime == SamplingRegime::Unresolved { iso.len() } else { iso.nearest(m_lim).1 };
        let row_mass: f64 = (0..arg).map(|r| iso.current_mass[r] * w[r]).sum();
        unresolved.current_mass = n_total * row_mass + evolved_mass;
        unresolved.expected_count = num_integrated;
        for k in 0..nf {
            let col = &iso.mags[k];
            let f1: f64 = (0..arg).map(|r| 10f64.powf(-0.4 * col[r]) * w[r]).sum();
            let f2: f64 = (0..arg).map(|r| 10f64.powf(-0.8 * col[r]) * w[r]).sum();
            unresolved.flux1_abs[k] += n_total * f1;
            unresolved.flux2_abs[k] += n_total * f2;
        }
        sampled_mass = current.iter().sum();
        total_mass = (sampled_mass + unresolved.current_mass) / remnants_factor;
        sampling["evolved_faint_transferred"] = evolved.into();
    } else {
        sampled_mass = current.iter().sum();
        total_mass = sampled_mass / remnants_factor;
    }

    let n = initial.len();
    let mut extra = extra.into_iter();
    let catalog = Catalog {
        star_id: (0..n as u64).collect(),
        component: vec![0; n],
        eep: extra.next().unwrap(),
        log_l: extra.next().unwrap(),
        log_teff: extra.next().unwrap(),
        initial_mass: initial,
        current_mass: current,
        abs_mags,
    };
    let component = Component {
        id: 0,
        label: spec.label.clone(),
        isochrone: spec.isochrone_identity.clone(),
        imf: imf.clone(),
        distance_pc: spec.distance_pc,
        extinction: spec.extinction.clone(),
        mag_limit: spec.mag_limit.clone(),
        sample_fraction: frac,
        normalization: spec.normalization,
        add_remnants: spec.add_remnants,
        remnant_model: spec.remnant_model,
        remnants_factor,
        total_mass,
        live_star_mass: total_mass * remnants_factor,
        sampled_mass,
        discrete_initial_mass: catalog.initial_mass.iter().sum(),
        num_discrete: n as u64,
        unresolved,
        sampling,
    };
    Ok(Population { filters: iso.filters.clone(), magnitude_system: iso.magnitude_system.clone(), components: vec![component], catalog })
}

/// Photometric sums for one band.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BandSums {
    pub discrete_flux: f64,
    pub discrete_flux2: f64,
    pub discrete_count: f64,
    pub unresolved_flux: f64,
    pub unresolved_flux2: f64,
    pub unresolved_count: f64,
}

impl BandSums {
    pub fn flux(&self) -> f64 {
        self.discrete_flux + self.unresolved_flux
    }
    pub fn total_mag(&self) -> f64 {
        -2.5 * self.flux().log10()
    }
    pub fn mean_mag(&self) -> f64 {
        -2.5 * (self.flux() / (self.discrete_count + self.unresolved_count)).log10()
    }
    pub fn sbf_mag(&self) -> f64 {
        -2.5 * ((self.discrete_flux2 + self.unresolved_flux2) / self.flux()).log10()
    }
}

impl Population {
    pub fn filter_index(&self, band: &str) -> Result<usize> {
        self.filters.iter().position(|f| f == band).ok_or_else(|| Error::unknown_filter("/bands", band, &self.filters))
    }

    pub fn num_stars(&self) -> usize {
        self.catalog.len()
    }

    /// `DM + A_b` for a component and band.
    pub fn apparent_offset(&self, component: u32, band: usize) -> f64 {
        let c = &self.components[component as usize];
        c.distance_modulus() + c.extinction[band]
    }

    pub fn apparent_mags(&self, band: usize) -> Vec<f64> {
        use rayon::prelude::*;
        let offsets: Vec<f64> = (0..self.components.len()).map(|c| self.apparent_offset(c as u32, band)).collect();
        let (mags, comp) = (&self.catalog.abs_mags[band], &self.catalog.component);
        let mut out = crate::mem::zeroed_f64(mags.len());
        out.par_chunks_mut(crate::mem::PAR_CHUNK).zip(mags.par_chunks(crate::mem::PAR_CHUNK)).zip(comp.par_chunks(crate::mem::PAR_CHUNK)).for_each(|((o, m), c)| {
            for ((a, m), c) in o.iter_mut().zip(m).zip(c) {
                *a = m + offsets[*c as usize];
            }
        });
        out
    }

    /// Apparent flux sums; `selection` restricts discrete stars, and
    /// `unresolved_components` lists the components whose integrated light is added.
    pub fn band_sums(&self, band: usize, selection: Option<&[bool]>, unresolved_components: &[u32]) -> BandSums {
        let component = if self.components.len() > 1 { Some(self.catalog.component.as_slice()) } else { None };
        band_sums_from_columns(&self.components, band, &self.catalog.abs_mags[band], component, selection, unresolved_components)
    }

    /// Apparent integrated magnitude of one component's unresolved light.
    pub fn unresolved_apparent_mag(&self, component: u32, band: usize) -> Option<f64> {
        let c = &self.components[component as usize];
        let f = c.unresolved.flux1_abs[band];
        if f > 0.0 { Some(-2.5 * f.log10() + self.apparent_offset(component, band)) } else { None }
    }

    pub fn total_mass(&self) -> f64 {
        self.components.iter().map(|c| c.total_mass).sum()
    }

    pub fn live_star_mass(&self) -> f64 {
        self.components.iter().map(|c| c.live_star_mass).sum()
    }

    pub fn sampled_mass(&self) -> f64 {
        self.components.iter().map(|c| c.sampled_mass).sum()
    }

    /// New population with changed distance and/or extinction (no resampling).
    pub fn with_observing_conditions(&self, distance_pc: Option<f64>, extinction: Option<&[f64]>, components: Option<&[u32]>) -> Result<Population> {
        let mut out = self.clone();
        if let Some(d) = distance_pc {
            crate::error::require_positive(d, "/distance", "Distance")?;
        }
        if let Some(e) = extinction {
            if e.len() != self.filters.len() {
                return Err(Error::new(ErrorCode::ShapeMismatch, "Extinction must have one value per filter."));
            }
            crate::error::require_finite(e, "/extinction_mag", "extinction")?;
        }
        for c in &mut out.components {
            if components.is_some_and(|ids| !ids.contains(&c.id)) {
                continue;
            }
            if let Some(d) = distance_pc {
                c.distance_pc = d;
            }
            if let Some(e) = extinction {
                c.extinction = e.to_vec();
            }
        }
        Ok(out)
    }
}

/// [`Population::band_sums`] on raw columns: `abs_mags` holds absolute
/// magnitudes, `component` the component of each star (None = all component
/// 0). Fluxes `10^(-0.4 (M + DM + A))` are evaluated in parallel chunks and
/// accumulated sequentially in star order, so the sums are bitwise identical
/// to a plain loop.
pub fn band_sums_from_columns(components: &[Component], band: usize, abs_mags: &[f64], component: Option<&[u32]>, selection: Option<&[bool]>, unresolved_components: &[u32]) -> BandSums {
    use rayon::prelude::*;
    let offsets: Vec<f64> = components.iter().map(|c| c.distance_modulus() + c.extinction[band]).collect();
    // Fluxes of one super-block are evaluated in parallel into a reused
    // buffer, then accumulated sequentially, so memory stays bounded and the
    // addition order is the star order.
    const CHUNK: usize = 1 << 15;
    const SUPER: usize = 64 * CHUNK;
    let mut buf = vec![0.0f64; SUPER.min(abs_mags.len())];
    let (mut f, mut q, mut n) = (0.0, 0.0, 0.0);
    for start in (0..abs_mags.len()).step_by(SUPER) {
        let end = (start + SUPER).min(abs_mags.len());
        let block = &mut buf[..end - start];
        block.par_chunks_mut(CHUNK).zip(abs_mags[start..end].par_chunks(CHUNK)).enumerate().for_each(|(k, (o, m))| {
            let base = start + k * CHUNK;
            for (j, (v, mag)) in o.iter_mut().zip(m).enumerate() {
                let off = match component {
                    Some(c) => offsets[c[base + j] as usize],
                    None => offsets[0],
                };
                *v = 10f64.powf(-0.4 * (mag + off));
            }
        });
        for (j, fl) in block.iter().enumerate() {
            if selection.is_none_or(|s| s[start + j]) {
                f += fl;
                q += fl * fl;
                n += 1.0;
            }
        }
    }
    let (mut uf, mut uq, mut un) = (0.0, 0.0, 0.0);
    for &cid in unresolved_components {
        let c = &components[cid as usize];
        let scale = 10f64.powf(-0.4 * offsets[cid as usize]);
        uf += c.unresolved.flux1_abs[band] * scale;
        uq += c.unresolved.flux2_abs[band] * scale * scale;
        un += c.unresolved.expected_count;
    }
    BandSums { discrete_flux: f, discrete_flux2: q, discrete_count: n, unresolved_flux: uf, unresolved_flux2: uq, unresolved_count: un }
}

/// Concatenate populations as components, preserving each component's
/// distance, extinction, unresolved moments, and identity.
pub fn compose(parts: &[(&Population, Option<String>)], require_equal_distance: bool) -> Result<Population> {
    let first = parts.first().ok_or_else(|| Error::invalid("/populations", "At least one population is required."))?.0;
    for (i, (p, _)) in parts.iter().enumerate() {
        if p.filters != first.filters {
            return Err(Error::new(ErrorCode::ModelAlignment, "Populations must share the same ordered filter set.")
                .at(&format!("/populations/{i}"))
                .with_details(serde_json::json!({ "expected": first.filters, "received": p.filters })));
        }
        if p.magnitude_system != first.magnitude_system {
            return Err(Error::new(ErrorCode::ModelAlignment, "Populations must share the same magnitude system.").at(&format!("/populations/{i}")));
        }
        if require_equal_distance {
            let d0 = first.components[0].distance_pc;
            if p.components.iter().any(|c| (c.distance_pc - d0).abs() > 1e-9 * d0) {
                return Err(Error::invalid(&format!("/populations/{i}"), "Population composition requires equal distances; compose sources into a scene for different distances."));
            }
        }
    }
    let nf = first.filters.len();
    let has = |f: fn(&Catalog) -> &Option<Vec<f64>>| parts.iter().all(|(p, _)| f(&p.catalog).is_some() || p.catalog.is_empty());
    let (keep_eep, keep_l, keep_t) = (has(|c| &c.eep), has(|c| &c.log_l), has(|c| &c.log_teff));
    let mut out = Population { filters: first.filters.clone(), magnitude_system: first.magnitude_system.clone(), components: Vec::new(), catalog: Catalog::default() };
    out.catalog.abs_mags = vec![Vec::new(); nf];
    out.catalog.eep = keep_eep.then(Vec::new);
    out.catalog.log_l = keep_l.then(Vec::new);
    out.catalog.log_teff = keep_t.then(Vec::new);
    for (p, label) in parts {
        let base = out.components.len() as u32;
        for c in &p.components {
            let mut nc = c.clone();
            nc.id = base + c.id;
            if let Some(l) = label {
                nc.label = if p.components.len() == 1 { l.clone() } else { format!("{l}/{}", c.label) };
            }
            out.components.push(nc);
        }
        let cat = &p.catalog;
        out.catalog.initial_mass.extend_from_slice(&cat.initial_mass);
        out.catalog.current_mass.extend_from_slice(&cat.current_mass);
        out.catalog.component.extend(cat.component.iter().map(|c| c + base));
        for k in 0..nf {
            out.catalog.abs_mags[k].extend_from_slice(&cat.abs_mags[k]);
        }
        for (dst, src) in [(&mut out.catalog.eep, &cat.eep), (&mut out.catalog.log_l, &cat.log_l), (&mut out.catalog.log_teff, &cat.log_teff)] {
            if let (Some(d), Some(s)) = (dst.as_mut(), src.as_ref()) {
                d.extend_from_slice(s);
            }
        }
    }
    out.catalog.star_id = (0..out.catalog.initial_mass.len() as u64).collect();
    Ok(out)
}

/// MIST evolutionary phases (selection by interpolated EEP).
pub const PHASES: [&str; 10] = ["PMS", "MS", "giants", "RGB", "CHeB", "AGB", "EAGB", "TPAGB", "postAGB", "WDCS"];
pub const PHASE_LABELS: [&str; 8] = ["PMS", "MS", "RGB", "CHeB", "EAGB", "TPAGB", "postAGB", "WDCS"];

pub fn in_phase(phase: &str, eep: f64) -> Result<bool> {
    Ok(match phase {
        "all" => true,
        "PMS" => eep < 202.0,
        "MS" => (202.0..454.0).contains(&eep),
        "giants" => (454.0..1409.0).contains(&eep),
        "RGB" => (454.0..=605.0).contains(&eep),
        "CHeB" => eep > 605.0 && eep < 707.0,
        "AGB" => (707.0..1409.0).contains(&eep),
        "EAGB" => (707.0..808.0).contains(&eep),
        "TPAGB" => (808.0..1409.0).contains(&eep),
        "postAGB" => (1409.0..=1710.0).contains(&eep),
        "WDCS" => eep > 1710.0,
        _ => {
            return Err(Error::invalid("/phase", format!("Unknown phase '{phase}'."))
                .with_details(serde_json::json!({ "available": ["all", "PMS", "MS", "giants", "RGB", "CHeB", "AGB", "EAGB", "TPAGB", "postAGB", "WDCS"] })));
        }
    })
}

pub fn phase_label(eep: f64) -> &'static str {
    PHASE_LABELS.iter().copied().find(|p| in_phase(p, eep).unwrap_or(false)).unwrap_or("")
}

/// Expected stars per pixel for a uniform surface brightness (ArtPop
/// `constant_sb_stars_per_pix`).
pub fn stars_per_pixel(sb: f64, mean_mag: f64, distance_pc: f64, pixel_scale_arcsec: f64) -> f64 {
    10f64.powf(0.4 * (mean_mag + distance_modulus(distance_pc) - sb)) * pixel_scale_arcsec * pixel_scale_arcsec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isochrone::tests::fixture;

    fn spec(iso: &Isochrone, norm: Normalization) -> BuildSpec<'_> {
        BuildSpec {
            isochrone: iso,
            isochrone_identity: serde_json::json!({}),
            normalization: norm,
            imf: Imf::kroupa(),
            distance_pc: 10.0,
            extinction: vec![0.0; iso.filters.len()],
            mag_limit: None,
            add_remnants: true,
            remnant_model: RemnantModel::default(),
            mass_tolerance: 0.01,
            algorithm: SamplingAlgorithm::TabulatedInverse,
            num_edges: imf::DEFAULT_NUM_EDGES,
            stopping: StoppingPolicy::WholeBatch,
            max_stars: 100_000_000,
            label: "ssp".into(),
        }
    }

    #[test]
    fn extinction_shifts_total_and_sbf_by_exactly_a() {
        let iso = fixture();
        let mut s = spec(&iso, Normalization::Count { value: 20_000 });
        s.mag_limit = Some(MagLimit { magnitude: 30.0, band: "LSST_i".into(), frame: CutoffFrame::IntrinsicApparent });
        s.distance_pc = 1.0e6;
        let p0 = build(&s, Seed(3), "0").unwrap();
        assert_eq!(p0.components[0].sample_fraction.regime, SamplingRegime::Partial);
        let a = vec![1.0; iso.filters.len()];
        let p1 = p0.with_observing_conditions(None, Some(&a), None).unwrap();
        let b = p0.filter_index("LSST_i").unwrap();
        let all = [0u32];
        let (s0, s1) = (p0.band_sums(b, None, &all), p1.band_sums(b, None, &all));
        assert!((s1.total_mag() - s0.total_mag() - 1.0).abs() < 1e-12);
        assert!((s1.sbf_mag() - s0.sbf_mag() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mass_normalization_and_cutoff_regimes() {
        let iso = fixture();
        let s = spec(&iso, Normalization::CurrentMass { value: 1.0e4 });
        let p = build(&s, Seed(5), "0").unwrap();
        let c = &p.components[0];
        assert!(c.total_mass >= 1.0e4 * 0.97 && c.total_mass < 1.0e4 * 1.05, "{}", c.total_mass);
        let mut bright = spec(&iso, Normalization::Count { value: 1000 });
        bright.mag_limit = Some(MagLimit { magnitude: -20.0, band: "LSST_i".into(), frame: CutoffFrame::Observed });
        let pb = build(&bright, Seed(5), "0").unwrap();
        assert_eq!(pb.num_stars(), 0);
        assert_eq!(pb.components[0].sample_fraction.regime, SamplingRegime::Unresolved);
        assert!(pb.components[0].unresolved.flux1_abs[0] > 0.0);
    }

    #[test]
    fn composition_keeps_moments_in_either_order() {
        let iso = fixture();
        let mut s = spec(&iso, Normalization::Count { value: 5000 });
        let full = build(&s, Seed(1), "0").unwrap();
        s.mag_limit = Some(MagLimit { magnitude: 6.0, band: "LSST_i".into(), frame: CutoffFrame::Observed });
        let part = build(&s, Seed(2), "0").unwrap();
        for order in [[&full, &part], [&part, &full]] {
            let c = compose(&[(order[0], None), (order[1], None)], true).unwrap();
            assert_eq!(c.components.len(), 2);
            assert_eq!(c.num_stars(), full.num_stars() + part.num_stars());
            let b = c.filter_index("LSST_r").unwrap();
            let sum = c.band_sums(b, None, &[0, 1]).flux();
            let parts = full.band_sums(b, None, &[0]).flux() + part.band_sums(b, None, &[0]).flux();
            assert!((sum / parts - 1.0).abs() < 1e-12);
            assert!((c.total_mass() - full.total_mass() - part.total_mass()).abs() < 1e-9 * c.total_mass());
        }
    }

    #[test]
    fn chunked_band_sums_match_a_plain_loop() {
        let iso = fixture();
        let s = spec(&iso, Normalization::Count { value: 150_000 });
        let p = build(&s, Seed(12), "0").unwrap();
        let b = p.filter_index("LSST_g").unwrap();
        let sums = p.band_sums(b, None, &[0]);
        let (mut f, mut q) = (0.0, 0.0);
        let off = p.apparent_offset(0, b);
        for m in &p.catalog.abs_mags[b] {
            let fl = 10f64.powf(-0.4 * (m + off));
            f += fl;
            q += fl * fl;
        }
        assert_eq!(sums.discrete_flux.to_bits(), f.to_bits());
        assert_eq!(sums.discrete_flux2.to_bits(), q.to_bits());
        assert_eq!(sums.discrete_count, 150_000.0);
    }

    #[test]
    fn phase_boundaries() {
        assert_eq!(phase_label(201.999), "PMS");
        assert_eq!(phase_label(202.0), "MS");
        assert_eq!(phase_label(605.0), "RGB");
        assert_eq!(phase_label(605.0001), "CHeB");
        assert_eq!(phase_label(1409.0), "postAGB");
        assert_eq!(phase_label(1710.0), "postAGB");
        assert_eq!(phase_label(1710.5), "WDCS");
        assert!(in_phase("giants", 1408.9).unwrap() && !in_phase("giants", 1409.0).unwrap());
    }
}
