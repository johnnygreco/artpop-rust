//! Spatial models, weighted probability grids, and position samplers.
//!
//! Grid weights are evaluated directly from integer pixel indices (no x/y mesh
//! arrays) and stored row-major as `w[y * width + x]`. Sampling is categorical
//! with replacement through a cumulative table (`cdf`, O(log P) per draw) or a
//! Vose alias table (`alias`, O(1) per draw); the method is explicit and part
//! of the provenance. The outside-frame policy is applied after every offset.

use crate::error::{Error, ErrorCode, Result};
use crate::mem;
use crate::rng::{Seed, par_blocks, par_blocks2};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Pixel coordinate convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema)]
pub enum Coordinates {
    /// Integer coordinates are pixel centers; frame `[-0.5, W-0.5)`; models
    /// center on `((W-1)/2, (H-1)/2)`.
    #[serde(rename = "pixel-centers-v1")]
    #[default]
    PixelCentersV1,
    /// ArtPop 0.1.2: centers at `W//2` (grid/smooth) or `W/2` (Plummer),
    /// uniform draws on `[0, W)`, histogram bins of width `(W-1)/W`.
    #[serde(rename = "artpop-0.1.2")]
    ArtPop012,
}

impl Coordinates {
    pub fn name(self) -> &'static str {
        match self {
            Coordinates::PixelCentersV1 => "pixel-centers-v1",
            Coordinates::ArtPop012 => "artpop-0.1.2",
        }
    }

    /// Center used by grid samplers and smooth models.
    pub fn model_center(self, width: usize, height: usize) -> (f64, f64) {
        match self {
            Coordinates::PixelCentersV1 => ((width as f64 - 1.0) / 2.0, (height as f64 - 1.0) / 2.0),
            Coordinates::ArtPop012 => ((width / 2) as f64, (height / 2) as f64),
        }
    }

    /// Frame bounds `[lo, hi)` for continuous positions.
    pub fn frame(self, size: usize) -> (f64, f64) {
        match self {
            Coordinates::PixelCentersV1 => (-0.5, size as f64 - 0.5),
            Coordinates::ArtPop012 => (0.0, size as f64),
        }
    }

    pub fn inside(self, x: f64, y: f64, width: usize, height: usize) -> bool {
        let (x0, x1) = self.frame(width);
        let (y0, y1) = self.frame(height);
        x.is_finite() && y.is_finite() && x >= x0 && x < x1 && y >= y0 && y < y1
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Model {
    /// Astropy `Sersic2D` (pixel units, theta counterclockwise in radians).
    Sersic { x0: f64, y0: f64, amplitude: f64, r_eff: f64, n: f64, ellip: f64, theta: f64 },
    /// ArtPop `Plummer2D`: `A / (1 + (r/a)^2)^2`.
    Plummer { x0: f64, y0: f64, amplitude: f64, scale_radius: f64 },
    /// Constant surface brightness.
    Constant { amplitude: f64 },
    /// Astropy `Ring2D`: amplitude for `r_in <= r <= r_in + width`.
    Ring { x0: f64, y0: f64, amplitude: f64, r_in: f64, width: f64 },
    /// Circular Gaussian.
    Gaussian { x0: f64, y0: f64, amplitude: f64, sigma: f64 },
    /// Sum of terms (bounded expression tree, no executable code).
    Sum { terms: Vec<Model> },
}

pub struct PreparedModel<'a> {
    model: &'a Model,
    bn: f64,
    /// `theta.sin_cos()` of a Sérsic model, evaluated once.
    sincos: (f64, f64),
    children: Vec<PreparedModel<'a>>,
}

impl Model {
    pub fn validate(&self, path: &str, depth: usize) -> Result<()> {
        if depth > 8 {
            return Err(Error::invalid(path, "Model expression is nested too deeply (maximum depth 8)."));
        }
        let finite = |vals: &[(&str, f64)]| -> Result<()> {
            for (name, v) in vals {
                if !v.is_finite() {
                    return Err(Error::nonfinite(&format!("{path}/{name}"), format!("{name} must be finite.")));
                }
            }
            Ok(())
        };
        match self {
            Model::Sersic { x0, y0, amplitude, r_eff, n, ellip, theta } => {
                finite(&[("x0", *x0), ("y0", *y0), ("amplitude", *amplitude), ("r_eff", *r_eff), ("n", *n), ("ellip", *ellip), ("theta", *theta)])?;
                crate::error::require_positive(*n, &format!("{path}/n"), "Sersic index")?;
                crate::error::require_positive(*r_eff, &format!("{path}/r_eff"), "Effective radius")?;
                if !(0.0..1.0).contains(ellip) {
                    return Err(Error::invalid(&format!("{path}/ellip"), "Ellipticity must satisfy 0 <= ellip < 1."));
                }
            }
            Model::Plummer { x0, y0, amplitude, scale_radius } => {
                finite(&[("x0", *x0), ("y0", *y0), ("amplitude", *amplitude)])?;
                crate::error::require_positive(*scale_radius, &format!("{path}/scale_radius"), "Scale radius")?;
            }
            Model::Constant { amplitude } => finite(&[("amplitude", *amplitude)])?,
            Model::Ring { x0, y0, amplitude, r_in, width } => {
                finite(&[("x0", *x0), ("y0", *y0), ("amplitude", *amplitude)])?;
                crate::error::require_nonnegative(*r_in, &format!("{path}/r_in"), "Inner radius")?;
                crate::error::require_positive(*width, &format!("{path}/width"), "Ring width")?;
            }
            Model::Gaussian { x0, y0, amplitude, sigma } => {
                finite(&[("x0", *x0), ("y0", *y0), ("amplitude", *amplitude)])?;
                crate::error::require_positive(*sigma, &format!("{path}/sigma"), "Gaussian sigma")?;
            }
            Model::Sum { terms } => {
                if terms.is_empty() || terms.len() > 64 {
                    return Err(Error::invalid(&format!("{path}/terms"), "A sum needs between 1 and 64 terms."));
                }
                for (i, t) in terms.iter().enumerate() {
                    t.validate(&format!("{path}/terms/{i}"), depth + 1)?;
                }
            }
        }
        Ok(())
    }

    pub fn prepare(&self) -> Result<PreparedModel<'_>> {
        self.validate("/model", 0)?;
        self.prepare_inner()
    }

    fn prepare_inner(&self) -> Result<PreparedModel<'_>> {
        let bn = match self {
            Model::Sersic { n, .. } => crate::special::sersic_bn(*n)?,
            _ => 0.0,
        };
        let sincos = match self {
            Model::Sersic { theta, .. } => theta.sin_cos(),
            _ => (0.0, 0.0),
        };
        let children = match self {
            Model::Sum { terms } => terms.iter().map(|t| t.prepare_inner()).collect::<Result<_>>()?,
            _ => Vec::new(),
        };
        Ok(PreparedModel { model: self, bn, sincos, children })
    }

    /// Return a copy with every amplitude multiplied by `factor`.
    pub fn scaled(&self, factor: f64) -> Model {
        let mut m = self.clone();
        match &mut m {
            Model::Sersic { amplitude, .. }
            | Model::Plummer { amplitude, .. }
            | Model::Constant { amplitude }
            | Model::Ring { amplitude, .. }
            | Model::Gaussian { amplitude, .. } => *amplitude *= factor,
            Model::Sum { terms } => {
                for t in terms {
                    *t = t.scaled(factor);
                }
            }
        }
        m
    }
}

impl PreparedModel<'_> {
    #[inline]
    pub fn eval(&self, x: f64, y: f64) -> f64 {
        match *self.model {
            Model::Sersic { x0, y0, amplitude, r_eff, n, ellip, theta: _ } => {
                let (s, c) = self.sincos;
                let x_maj = ((x - x0) * c + (y - y0) * s).abs();
                let x_min = (-(x - x0) * s + (y - y0) * c).abs();
                let b = (1.0 - ellip) * r_eff;
                let z = ((x_maj / r_eff).powf(2.0) + (x_min / b).powf(2.0)).powf(0.5);
                amplitude * (-self.bn * (z.powf(1.0 / n) - 1.0)).exp()
            }
            Model::Plummer { x0, y0, amplitude, scale_radius } => {
                let r = ((x - x0).powi(2) + (y - y0).powi(2)).sqrt();
                amplitude / (1.0 + (r / scale_radius).powi(2)).powi(2)
            }
            Model::Constant { amplitude } => amplitude,
            Model::Ring { x0, y0, amplitude, r_in, width } => {
                let rr = (x - x0).powi(2) + (y - y0).powi(2);
                if rr >= r_in * r_in && rr <= (r_in + width).powi(2) { amplitude } else { 0.0 }
            }
            Model::Gaussian { x0, y0, amplitude, sigma } => amplitude * (-((x - x0).powi(2) + (y - y0).powi(2)) / (2.0 * sigma * sigma)).exp(),
            Model::Sum { .. } => self.children.iter().map(|c| c.eval(x, y)).sum(),
        }
    }
}

/// Validated nonnegative weights on a `width x height` grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightGrid {
    pub width: usize,
    pub height: usize,
    pub weights: Vec<f64>,
    pub total: f64,
    pub positive_cells: usize,
}

pub fn checked_pixels(width: usize, height: usize, max_pixels: u64) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("/width", "Grid dimensions must be positive."));
    }
    let p = (width as u64).checked_mul(height as u64).ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "Grid size overflows."))?;
    if p > max_pixels {
        return Err(Error::new(ErrorCode::ResourceLimit, format!("A {width}x{height} grid ({p} cells) exceeds the configured limit of {max_pixels} cells."))
            .with_details(serde_json::json!({ "cells": p, "max_grid_pixels": max_pixels, "bytes_estimate": p * 8 })));
    }
    Ok(p as usize)
}

impl WeightGrid {
    pub fn from_weights(width: usize, height: usize, weights: Vec<f64>) -> Result<WeightGrid> {
        if weights.len() != width * height {
            return Err(Error::new(ErrorCode::ShapeMismatch, "Weight count does not match width*height.")
                .with_details(serde_json::json!({ "expected": width * height, "received": weights.len() })));
        }
        let mut total = 0.0;
        let mut positive = 0;
        for (i, w) in weights.iter().enumerate() {
            if !w.is_finite() {
                return Err(Error::nonfinite("/weights", format!("Grid weight at cell {i} (x={}, y={}) is not finite.", i % width, i / width)));
            }
            if *w < 0.0 {
                return Err(Error::invalid("/weights", format!("Grid weight at cell {i} (x={}, y={}) is negative.", i % width, i / width)));
            }
            if *w > 0.0 {
                positive += 1;
            }
            total += w;
        }
        if !(total > 0.0) || !total.is_finite() {
            return Err(Error::new(ErrorCode::ZeroWeightGrid, "Probability grid has no positive (finite) total weight."));
        }
        Ok(WeightGrid { width, height, weights, total, positive_cells: positive })
    }

    /// Evaluate a model at every integer pixel `(x, y)`.
    pub fn from_model(model: &Model, width: usize, height: usize, max_pixels: u64) -> Result<WeightGrid> {
        let p = checked_pixels(width, height, max_pixels)?;
        let prepared = model.prepare()?;
        let mut weights = mem::zeroed_f64(p);
        weights.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            for (x, w) in row.iter_mut().enumerate() {
                *w = prepared.eval(x as f64, y as f64);
            }
        });
        WeightGrid::from_weights(width, height, weights)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CategoricalMethod {
    Cdf,
    Alias,
}

pub enum Categorical {
    Cdf { cdf: Vec<f64>, last_positive: usize },
    Alias { prob: Vec<f64>, alias: Vec<u32> },
}

impl Categorical {
    pub fn new(grid: &WeightGrid, method: CategoricalMethod) -> Categorical {
        match method {
            CategoricalMethod::Cdf => {
                let mut cdf = Vec::with_capacity(grid.weights.len());
                let mut acc = 0.0;
                let mut last_positive = 0;
                for (i, w) in grid.weights.iter().enumerate() {
                    acc += w;
                    if *w > 0.0 {
                        last_positive = i;
                    }
                    cdf.push(acc);
                }
                Categorical::Cdf { cdf, last_positive }
            }
            CategoricalMethod::Alias => {
                let n = grid.weights.len();
                let scale = n as f64 / grid.total;
                let mut prob: Vec<f64> = grid.weights.iter().map(|w| w * scale).collect();
                let mut alias = vec![0u32; n];
                let (mut small, mut large): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
                for (i, p) in prob.iter().enumerate() {
                    if *p < 1.0 { small.push(i) } else { large.push(i) }
                }
                while let (Some(s), Some(l)) = (small.pop(), large.last().copied()) {
                    alias[s] = l as u32;
                    prob[l] -= 1.0 - prob[s];
                    if prob[l] < 1.0 {
                        large.pop();
                        small.push(l);
                    }
                }
                for l in large {
                    prob[l] = 1.0;
                }
                for s in small {
                    // Numerical leftovers: only positive cells may self-select.
                    prob[s] = if grid.weights[s] > 0.0 { 1.0 } else { 0.0 };
                    if grid.weights[s] == 0.0 {
                        alias[s] = grid.weights.iter().position(|w| *w > 0.0).unwrap_or(0) as u32;
                    }
                }
                Categorical::Alias { prob, alias }
            }
        }
    }

    #[inline]
    pub fn draw(&self, stream: &mut crate::rng::Stream) -> usize {
        match self {
            Categorical::Cdf { cdf, last_positive } => {
                let t = stream.uniform() * cdf[cdf.len() - 1];
                cdf.partition_point(|c| *c <= t).min(*last_positive)
            }
            Categorical::Alias { prob, alias } => {
                let n = prob.len();
                let i = ((stream.uniform() * n as f64) as usize).min(n - 1);
                if stream.uniform() < prob[i] { i } else { alias[i] as usize }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutsidePolicy {
    /// Remove positions outside the frame.
    Drop,
    /// Keep them, marked invalid; image accumulation excludes them.
    RetainMarked,
    /// Fail if any position falls outside.
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Positions {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub inside: Vec<bool>,
    /// Index of each retained position in the generated sequence; `None`
    /// means every generated position was retained (identity, not stored).
    pub index: Option<Vec<u64>>,
    pub generated: u64,
}

impl Positions {
    pub fn outside_count(&self) -> u64 {
        self.generated - self.inside.iter().filter(|v| **v).count() as u64
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    /// Generated-sequence index of retained position `i`.
    #[inline]
    pub fn index_at(&self, i: usize) -> u64 {
        match &self.index {
            Some(v) => v[i],
            None => i as u64,
        }
    }

    /// The explicit index column (materialized for identity).
    pub fn index_column(&self) -> std::borrow::Cow<'_, [u64]> {
        match &self.index {
            Some(v) => std::borrow::Cow::Borrowed(v),
            None => std::borrow::Cow::Owned((0..self.len() as u64).collect()),
        }
    }
}

/// Apply offsets were already added; now enforce the outside policy.
pub fn apply_outside(xs: Vec<f64>, ys: Vec<f64>, coords: Coordinates, width: usize, height: usize, policy: OutsidePolicy) -> Result<Positions> {
    let n = xs.len();
    let inside: Vec<bool> = xs.par_iter().zip(ys.par_iter()).with_min_len(mem::PAR_CHUNK).map(|(x, y)| coords.inside(*x, *y, width, height)).collect();
    let outside = inside.par_iter().filter(|v| !**v).count();
    match policy {
        OutsidePolicy::Error if outside > 0 => Err(Error::new(ErrorCode::OutOfDomain, format!("{outside} of {n} sampled positions fall outside the {width}x{height} frame."))
            .at("/outside")
            .with_details(serde_json::json!({ "outside": outside, "generated": n, "alternatives": ["drop", "retain_marked"] }))),
        OutsidePolicy::Drop => {
            let mut p = Positions { generated: n as u64, ..Default::default() };
            let mut index = Vec::new();
            for i in 0..n {
                if inside[i] {
                    p.x.push(xs[i]);
                    p.y.push(ys[i]);
                    p.inside.push(true);
                    index.push(i as u64);
                }
            }
            p.index = if index.len() == n { None } else { Some(index) };
            Ok(p)
        }
        _ => Ok(Positions { x: xs, y: ys, inside, index: None, generated: n as u64 }),
    }
}

/// Offset of a grid within the image frame: the grid's cell `(gx, gy)`
/// maps to image coordinate `gx - shift_x`.
pub fn grid_shift(coords: Coordinates, grid_size: usize, image_size: usize) -> f64 {
    let d = grid_size as i64 - image_size as i64;
    match coords {
        Coordinates::PixelCentersV1 => d as f64 / 2.0,
        Coordinates::ArtPop012 => d.div_euclid(2) as f64,
    }
}

/// Random stream identity: draws come from block streams `(stage, keys..., block<k>)`.
#[derive(Clone, Copy)]
pub struct Draws<'a> {
    pub seed: Seed,
    pub stage: &'a str,
    pub keys: &'a [&'a str],
}

#[allow(clippy::too_many_arguments)]
pub fn sample_grid(grid: &WeightGrid, method: CategoricalMethod, count: usize, image: (usize, usize), offset: (f64, f64), coords: Coordinates, policy: OutsidePolicy, draws: Draws) -> Result<Positions> {
    let cat = Categorical::new(grid, method);
    let sx = grid_shift(coords, grid.width, image.0);
    let sy = grid_shift(coords, grid.height, image.1);
    let mut cells: Vec<u32> = mem::zeroed_u32(count);
    par_blocks(draws.seed, draws.stage, draws.keys, &mut cells, |st, chunk| {
        for c in chunk.iter_mut() {
            *c = cat.draw(st) as u32;
        }
    });
    let (mut xs, mut ys) = (mem::zeroed_f64(count), mem::zeroed_f64(count));
    xs.par_chunks_mut(mem::PAR_CHUNK).zip(ys.par_chunks_mut(mem::PAR_CHUNK)).zip(cells.par_chunks(mem::PAR_CHUNK)).for_each(|((cx, cy), cc)| {
        for ((x, y), k) in cx.iter_mut().zip(cy.iter_mut()).zip(cc) {
            let k = *k as usize;
            *x = (k % grid.width) as f64 - sx + offset.0;
            *y = (k / grid.width) as f64 - sy + offset.1;
        }
    });
    apply_outside(xs, ys, coords, image.0, image.1, policy)
}

/// Projected Plummer positions from the 3D radius `a / sqrt(u^(-2/3) - 1)`
/// with isotropic directions (ArtPop `plummer_xy`).
pub fn sample_plummer(count: usize, scale_radius_pix: f64, image: (usize, usize), offset: (f64, f64), coords: Coordinates, policy: OutsidePolicy, draws: Draws) -> Result<Positions> {
    crate::error::require_positive(scale_radius_pix, "/scale_radius", "Plummer scale radius")?;
    let (cx, cy) = match coords {
        Coordinates::PixelCentersV1 => coords.model_center(image.0, image.1),
        Coordinates::ArtPop012 => (image.0 as f64 / 2.0, image.1 as f64 / 2.0),
    };
    let (mut xs, mut ys) = (mem::zeroed_f64(count), mem::zeroed_f64(count));
    par_blocks2(draws.seed, draws.stage, draws.keys, &mut xs, &mut ys, |st, bx, by| {
        for (x, y) in bx.iter_mut().zip(by.iter_mut()) {
            let s = st.uniform();
            let r = scale_radius_pix / (s.powf(-2.0 / 3.0) - 1.0).sqrt();
            let phi = st.uniform_range(0.0, 2.0 * std::f64::consts::PI);
            let theta = (2.0 * st.uniform() - 1.0).acos();
            *x = r * phi.cos() * theta.sin() + cx + offset.0;
            *y = r * phi.sin() * theta.sin() + cy + offset.1;
        }
    });
    apply_outside(xs, ys, coords, image.0, image.1, policy)
}

pub fn sample_uniform(count: usize, image: (usize, usize), coords: Coordinates, draws: Draws) -> Positions {
    let (x0, x1) = coords.frame(image.0);
    let (y0, y1) = coords.frame(image.1);
    let (mut xs, mut ys) = (mem::zeroed_f64(count), mem::zeroed_f64(count));
    par_blocks2(draws.seed, draws.stage, draws.keys, &mut xs, &mut ys, |st, cx, cy| {
        for (x, y) in cx.iter_mut().zip(cy.iter_mut()) {
            *x = st.uniform_range(x0, x1);
            *y = st.uniform_range(y0, y1);
        }
    });
    let n = xs.len();
    Positions { x: xs, y: ys, inside: vec![true; n], index: None, generated: n as u64 }
}

/// Side of ArtPop's square Sérsic sampling grid: `2 ceil(r_pix * num_r_eff) + 1`.
pub fn sersic_grid_side(r_eff_pix: f64, num_r_eff: f64) -> Result<usize> {
    let half = (r_eff_pix * num_r_eff).ceil();
    if !half.is_finite() || half < 0.0 || half > 1.0e9 {
        return Err(Error::new(ErrorCode::ResourceLimit, "Sérsic sampling grid is unreasonably large.")
            .with_details(serde_json::json!({ "r_eff_pixels": r_eff_pix, "num_r_eff": num_r_eff })));
    }
    Ok(2 * half as usize + 1)
}

/// Angular size in arcsec of a physical length at a distance (both in pc).
pub fn angular_size_arcsec(length_pc: f64, distance_pc: f64) -> f64 {
    length_pc.atan2(distance_pc) * (180.0 / std::f64::consts::PI) * 3600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asymmetric_grid_preserves_axes() {
        // One hot cell at (x=1, y=3) on a 4x5 grid.
        let mut w = vec![0.0; 20];
        w[3 * 4 + 1] = 2.0;
        let g = WeightGrid::from_weights(4, 5, w).unwrap();
        for method in [CategoricalMethod::Cdf, CategoricalMethod::Alias] {
            let p = sample_grid(&g, method, 50, (4, 5), (0.0, 0.0), Coordinates::PixelCentersV1, OutsidePolicy::Error, Draws { seed: Seed(1), stage: "p", keys: &[] }).unwrap();
            assert!(p.x.iter().all(|x| *x == 1.0) && p.y.iter().all(|y| *y == 3.0));
        }
    }

    #[test]
    fn invalid_grids_are_rejected() {
        assert_eq!(WeightGrid::from_weights(2, 1, vec![0.0, 0.0]).unwrap_err().code, ErrorCode::ZeroWeightGrid);
        assert_eq!(WeightGrid::from_weights(2, 1, vec![1.0, f64::NAN]).unwrap_err().code, ErrorCode::NonfiniteValue);
        assert_eq!(WeightGrid::from_weights(2, 1, vec![1.0, -1.0]).unwrap_err().code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn fractional_offsets_and_outside_policy() {
        let g = WeightGrid::from_weights(5, 5, vec![1.0; 25]).unwrap();
        let p = sample_grid(&g, CategoricalMethod::Cdf, 1000, (5, 5), (0.5, 0.0), Coordinates::PixelCentersV1, OutsidePolicy::RetainMarked, Draws { seed: Seed(2), stage: "p", keys: &[] }).unwrap();
        assert!(p.x.iter().all(|x| x.fract() == 0.5));
        // Column x = 4.5 is outside [-0.5, 4.5).
        let outside = p.x.iter().filter(|x| **x >= 4.5).count() as u64;
        assert_eq!(outside, p.outside_count());
        assert!(sample_grid(&g, CategoricalMethod::Cdf, 1000, (5, 5), (0.5, 0.0), Coordinates::PixelCentersV1, OutsidePolicy::Error, Draws { seed: Seed(2), stage: "p", keys: &[] }).is_err());
        let d = sample_grid(&g, CategoricalMethod::Cdf, 1000, (5, 5), (0.5, 0.0), Coordinates::PixelCentersV1, OutsidePolicy::Drop, Draws { seed: Seed(2), stage: "p", keys: &[] }).unwrap();
        assert_eq!(d.x.len() as u64 + outside, 1000);
    }

    #[test]
    fn categorical_frequencies() {
        let w = vec![1.0, 0.0, 3.0, 6.0];
        let g = WeightGrid::from_weights(4, 1, w.clone()).unwrap();
        for method in [CategoricalMethod::Cdf, CategoricalMethod::Alias] {
            let c = Categorical::new(&g, method);
            let mut st = Seed(9).stream("c", &[]);
            let mut counts = [0usize; 4];
            let n = 200_000;
            for _ in 0..n {
                counts[c.draw(&mut st)] += 1;
            }
            assert_eq!(counts[1], 0);
            for k in [0, 2, 3] {
                let p = w[k] / 10.0;
                let sd = (n as f64 * p * (1.0 - p)).sqrt();
                assert!((counts[k] as f64 - n as f64 * p).abs() < 6.0 * sd, "{method:?} {k} {counts:?}");
            }
        }
    }

    #[test]
    fn sersic_matches_astropy_value() {
        // astropy Sersic2D(amplitude=1, r_eff=5, n=1.5, x_0=0, y_0=0, ellip=0.3, theta=0.7)(3.0, -2.0)
        let m = Model::Sersic { x0: 0.0, y0: 0.0, amplitude: 1.0, r_eff: 5.0, n: 1.5, ellip: 0.3, theta: 0.7 };
        let v = m.prepare().unwrap().eval(3.0, -2.0);
        assert!((v - 0.9832393233479823).abs() < 1e-12, "{v}");
    }
}
