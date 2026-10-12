//! Row-major f64 images and weighted stellar histograms.

use crate::error::{Error, ErrorCode, Result};
use crate::spatial::Coordinates;
use serde::{Deserialize, Serialize};

/// `data[y * width + x]`; FITS `NAXIS1 = width`, `NAXIS2 = height`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f64>,
}

impl Image {
    pub fn zeros(width: usize, height: usize) -> Image {
        Image { width, height, data: vec![0.0; width * height] }
    }

    pub fn new(width: usize, height: usize, data: Vec<f64>) -> Result<Image> {
        if width == 0 || height == 0 {
            return Err(Error::invalid("/shape", "Image dimensions must be positive."));
        }
        if data.len() != width * height {
            return Err(Error::new(ErrorCode::ShapeMismatch, "Image data length does not match width*height.")
                .with_details(serde_json::json!({ "width": width, "height": height, "values": data.len() })));
        }
        Ok(Image { width, height, data })
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.width + x]
    }

    pub fn sum(&self) -> f64 {
        // Pairwise summation keeps large-image totals accurate.
        pairwise_sum(&self.data)
    }

    pub fn min_max(&self) -> (f64, f64) {
        self.data.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)))
    }
}

pub fn pairwise_sum(v: &[f64]) -> f64 {
    if v.len() <= 128 {
        let mut s = 0.0;
        for x in v {
            s += x;
        }
        s
    } else {
        let m = v.len() / 2;
        pairwise_sum(&v[..m]) + pairwise_sum(&v[m..])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvalidPolicy {
    Error,
    Drop,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DepositStats {
    pub generated: u64,
    pub selected: u64,
    pub invalid: u64,
    pub outside: u64,
    pub deposited: u64,
    pub selected_signal: f64,
    pub deposited_signal: f64,
    pub outside_signal: f64,
    pub convention: &'static str,
    pub reduction: &'static str,
}

/// Pixel binning rule with per-image constants precomputed.
#[derive(Clone, Copy)]
struct Binner {
    convention: Coordinates,
    width: usize,
    height: usize,
    w_f: f64,
    h_f: f64,
    xmax: f64,
    ymax: f64,
    normx: f64,
    normy: f64,
}

impl Binner {
    fn new(convention: Coordinates, width: usize, height: usize) -> Binner {
        let (w_f, h_f) = (width as f64, height as f64);
        let (xmax, ymax) = (w_f - 1.0, h_f - 1.0);
        // fast-histogram: normx = nx / (xmax - xmin) with xmin = 0.
        Binner { convention, width, height, w_f, h_f, xmax, ymax, normx: w_f / (xmax - 0.0), normy: h_f / (ymax - 0.0) }
    }

    /// Pixel cell of a coordinate (None = outside).
    #[inline]
    fn cell(&self, x: f64, y: f64) -> Option<usize> {
        match self.convention {
            Coordinates::PixelCentersV1 => {
                if x >= -0.5 && y >= -0.5 && x < self.w_f - 0.5 && y < self.h_f - 0.5 {
                    let ix = (x + 0.5).floor() as usize;
                    let iy = (y + 0.5).floor() as usize;
                    (ix < self.width && iy < self.height).then_some(iy * self.width + ix)
                } else {
                    None
                }
            }
            Coordinates::ArtPop012 => {
                // ix = (int)((x - xmin) * normx), with the final-bin clamp.
                if x >= 0.0 && x < self.xmax && y >= 0.0 && y < self.ymax {
                    let mut ix = ((x - 0.0) * self.normx) as usize;
                    let mut iy = ((y - 0.0) * self.normy) as usize;
                    if ix == self.width {
                        ix -= 1;
                    }
                    if iy == self.height {
                        iy -= 1;
                    }
                    Some(iy * self.width + ix)
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
fn cell(convention: Coordinates, width: usize, height: usize, x: f64, y: f64) -> Option<usize> {
    Binner::new(convention, width, height).cell(x, y)
}

const NONE: u32 = u32::MAX;
const INVALID: u32 = u32::MAX - 1;

#[derive(Default)]
struct Partial {
    selected: u64,
    invalid: u64,
    outside: u64,
    deposited: u64,
    selected_signal: f64,
    deposited_signal: f64,
    outside_signal: f64,
    first_invalid: Option<usize>,
}

/// Accumulate weights into pixels.
///
/// `pixel-centers-v1`: accept `[-0.5, W-0.5) x [-0.5, H-0.5)`, pixel index
/// `floor(x + 0.5)`. `artpop-0.1.2`: fast-histogram arithmetic with range
/// `[0, W-1)`, `ix = (int)(x * (W / (W-1)))`, final-bin clamp; the result is
/// the transposed histogram, i.e. also `[H, W]` row-major.
///
/// Every pixel adds its stars in input order starting from zero, so the image
/// is bitwise identical to a serial loop. Large inputs classify stars in
/// parallel blocks, bucket them by cache-sized row tiles with a stable
/// counting sort, and accumulate tiles in parallel. Diagnostic signal sums
/// are reduced in fixed blocks (deterministic for any thread count).
#[allow(clippy::too_many_arguments)]
pub fn deposit(
    convention: Coordinates,
    width: usize,
    height: usize,
    xs: &[f64],
    ys: &[f64],
    weights: Option<&[f64]>,
    selection: Option<&[bool]>,
    invalid: InvalidPolicy,
) -> Result<(Image, DepositStats)> {
    use rayon::prelude::*;
    let n = xs.len();
    if ys.len() != n || weights.is_some_and(|w| w.len() != n) || selection.is_some_and(|s| s.len() != n) {
        return Err(Error::new(ErrorCode::ShapeMismatch, "x, y, weights, and selection must have equal lengths."));
    }
    if convention == Coordinates::ArtPop012 && (width < 2 || height < 2) {
        return Err(Error::invalid("/geometry", "The artpop-0.1.2 histogram convention needs width > 1 and height > 1."));
    }
    if (width * height) as u64 >= INVALID as u64 {
        return Err(Error::new(ErrorCode::ResourceLimit, "Image is too large for 32-bit pixel indices."));
    }
    const BLOCK: usize = 1 << 16;
    let binner = Binner::new(convention, width, height);
    let mut cells = vec![NONE; n];
    let partials: Vec<Partial> = cells
        .par_chunks_mut(BLOCK)
        .enumerate()
        .map(|(b, chunk)| {
            let mut p = Partial::default();
            for (k, c) in chunk.iter_mut().enumerate() {
                let i = b * BLOCK + k;
                if selection.is_some_and(|s| !s[i]) {
                    continue;
                }
                let (x, y) = (xs[i], ys[i]);
                let w = weights.map_or(1.0, |w| w[i]);
                p.selected += 1;
                if !x.is_finite() || !y.is_finite() || !w.is_finite() {
                    p.invalid += 1;
                    p.first_invalid.get_or_insert(i);
                    *c = INVALID;
                    continue;
                }
                p.selected_signal += w;
                match binner.cell(x, y) {
                    Some(idx) => {
                        *c = idx as u32;
                        p.deposited += 1;
                        p.deposited_signal += w;
                    }
                    None => {
                        p.outside += 1;
                        p.outside_signal += w;
                    }
                }
            }
            p
        })
        .collect();
    let mut st = DepositStats { generated: n as u64, convention: convention.name(), reduction: "tiled-stable-input-order-v1", ..Default::default() };
    let mut first_invalid = None;
    for p in &partials {
        st.selected += p.selected;
        st.invalid += p.invalid;
        st.outside += p.outside;
        st.deposited += p.deposited;
        st.selected_signal += p.selected_signal;
        st.deposited_signal += p.deposited_signal;
        st.outside_signal += p.outside_signal;
        first_invalid = first_invalid.or(p.first_invalid);
    }
    if let (Some(i), InvalidPolicy::Error) = (first_invalid, invalid) {
        return Err(Error::nonfinite("/catalog", format!("Row {i} has a non-finite coordinate or weight."))
            .with_details(serde_json::json!({ "row": i, "alternatives": ["invalid: drop"] })));
    }
    let mut img = Image::zeros(width, height);
    let w_of = |i: usize| weights.map_or(1.0, |w| w[i]);
    if st.deposited < 1 << 18 || width * height < 1 << 18 {
        for (i, c) in cells.iter().enumerate() {
            if *c < INVALID {
                img.data[*c as usize] += w_of(i);
            }
        }
        return Ok((img, st));
    }
    // Stable counting sort of stars into row tiles of about 256 KiB.
    let tile_rows = (32_768 / width).max(1);
    let tile_len = tile_rows * width;
    let tiles = height.div_ceil(tile_rows);
    let mut starts = vec![0usize; tiles + 1];
    for c in &cells {
        if *c < INVALID {
            starts[*c as usize / tile_len + 1] += 1;
        }
    }
    for t in 0..tiles {
        starts[t + 1] += starts[t];
    }
    let mut fill = starts.clone();
    let mut order = vec![0u32; st.deposited as usize];
    for (i, c) in cells.iter().enumerate() {
        if *c < INVALID {
            let t = *c as usize / tile_len;
            order[fill[t]] = i as u32;
            fill[t] += 1;
        }
    }
    img.data.par_chunks_mut(tile_len).enumerate().for_each(|(t, tile)| {
        let base = t * tile_len;
        for &i in &order[starts[t]..starts[t + 1]] {
            tile[cells[i as usize] as usize - base] += w_of(i as usize);
        }
    });
    Ok((img, st))
}

/// Overlap of a `model` image centered at `center` (pixel indices) within a
/// destination image: returns `(dst_x0, dst_y0, src_x0, src_y0, nx, ny)`.
pub fn overlap(dst: (usize, usize), src: (usize, usize), center: (i64, i64)) -> Option<(usize, usize, usize, usize, usize, usize)> {
    let x0 = center.0 - (src.0 as i64) / 2;
    let y0 = center.1 - (src.1 as i64) / 2;
    let dx0 = x0.max(0);
    let dy0 = y0.max(0);
    let dx1 = (x0 + src.0 as i64).min(dst.0 as i64);
    let dy1 = (y0 + src.1 as i64).min(dst.1 as i64);
    if dx1 <= dx0 || dy1 <= dy0 {
        return None;
    }
    Some((dx0 as usize, dy0 as usize, (dx0 - x0) as usize, (dy0 - y0) as usize, (dx1 - dx0) as usize, (dy1 - dy0) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_edges_match_fast_histogram_probe() {
        // docs/evidence/probes.json histogram_edges (5x5, y = 1).
        let cases = [(-0.01, None), (0.0, Some(0)), (0.79, Some(0)), (0.8, Some(1)), (1.0, Some(1)), (3.99, Some(4)), (4.0, None), (4.99, None), (5.0, None)];
        for (x, want) in cases {
            let (img, st) = deposit(Coordinates::ArtPop012, 5, 5, &[x], &[1.0], None, None, InvalidPolicy::Error).unwrap();
            match want {
                Some(ix) => assert_eq!(img.get(ix, 1), 1.0, "x={x}"),
                None => assert_eq!(st.deposited, 0, "x={x}"),
            }
        }
    }

    #[test]
    fn pixel_centers_accept_final_column_and_reject_edges() {
        let xs = [4.0, 4.49, 4.5, -0.5, -0.51, 2.0];
        let ys = [0.0, 0.0, 0.0, 0.0, 0.0, 3.0];
        let (img, st) = deposit(Coordinates::PixelCentersV1, 5, 4, &xs, &ys, None, None, InvalidPolicy::Error).unwrap();
        assert_eq!(img.get(4, 0), 2.0);
        assert_eq!(img.get(0, 0), 1.0);
        assert_eq!(img.get(2, 3), 1.0);
        assert_eq!(st.outside, 2);
        assert_eq!(st.deposited, 4);
    }

    #[test]
    fn tiled_parallel_deposit_is_bitwise_serial() {
        let (w, h) = (701, 503);
        let mut st = crate::rng::Seed(11).stream("t", &[]);
        let n = 600_000;
        let xs: Vec<f64> = (0..n).map(|_| st.uniform_range(-3.0, w as f64 + 2.0)).collect();
        let ys: Vec<f64> = (0..n).map(|_| st.uniform_range(-3.0, h as f64 + 2.0)).collect();
        let ws: Vec<f64> = (0..n).map(|_| 10f64.powf(st.uniform_range(-3.0, 3.0))).collect();
        for conv in [Coordinates::PixelCentersV1, Coordinates::ArtPop012] {
            let (img, stats) = deposit(conv, w, h, &xs, &ys, Some(&ws), None, InvalidPolicy::Error).unwrap();
            assert_eq!(stats.reduction, "tiled-stable-input-order-v1");
            let mut serial = vec![0.0; w * h];
            for i in 0..n {
                if let Some(c) = cell(conv, w, h, xs[i], ys[i]) {
                    serial[c] += ws[i];
                }
            }
            assert!(img.data.iter().zip(&serial).all(|(a, b)| a.to_bits() == b.to_bits()), "{conv:?}");
        }
    }

    #[test]
    fn invalid_rows_are_counted_or_rejected() {
        let xs = [1.0, f64::NAN];
        let ys = [1.0, 1.0];
        assert!(deposit(Coordinates::PixelCentersV1, 3, 3, &xs, &ys, None, None, InvalidPolicy::Error).is_err());
        let (img, st) = deposit(Coordinates::PixelCentersV1, 3, 3, &xs, &ys, None, None, InvalidPolicy::Drop).unwrap();
        assert_eq!((st.invalid, st.deposited, img.sum()), (1, 1, 1.0));
    }
}
