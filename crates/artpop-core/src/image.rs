//! Row-major f64 images and weighted stellar histograms.

use crate::error::{Error, ErrorCode, Result};
use crate::mem;
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
        Image { width, height, data: mem::zeroed_f64(width * height) }
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

/// Pairwise summation (leaves of at most 128 sequential additions, split at
/// the midpoint). Halves above [`PAIRWISE_PAR_MIN`] elements are summed on
/// separate threads; the addition tree, and therefore every bit of the
/// result, is identical to the sequential evaluation.
pub fn pairwise_sum(v: &[f64]) -> f64 {
    if v.len() <= 128 {
        let mut s = 0.0;
        for x in v {
            s += x;
        }
        s
    } else {
        let m = v.len() / 2;
        if v.len() >= PAIRWISE_PAR_MIN {
            let (a, b) = rayon::join(|| pairwise_sum(&v[..m]), || pairwise_sum(&v[m..]));
            a + b
        } else {
            pairwise_sum(&v[..m]) + pairwise_sum(&v[m..])
        }
    }
}

/// Smallest slice length whose two halves are summed concurrently.
pub const PAIRWISE_PAR_MIN: usize = 1 << 17;

/// Raw pointer that may be shared across threads; callers guarantee that
/// concurrent writes never alias (disjoint index windows).
struct SyncPtr<T>(*mut T);

unsafe impl<T> Sync for SyncPtr<T> {}
unsafe impl<T> Send for SyncPtr<T> {}

impl<T> SyncPtr<T> {
    /// # Safety
    /// `i` must be in bounds and no other thread may write index `i`.
    #[inline]
    unsafe fn write(&self, i: usize, v: T) {
        unsafe { self.0.add(i).write(v) }
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
    let mut cells: Vec<u32> = mem::with_capacity(n);
    cells.resize(n, NONE);
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
    // Stable counting sort of stars into row tiles of about 256 KiB. Every
    // classification block counts its stars per tile, prefix sums give each
    // (tile, block) pair its own window, and blocks scatter their stars (tile-
    // local pixel index and weight) into those windows in parallel. Within a
    // tile the order is block order then input order, i.e. input order, so the
    // accumulation below adds exactly what a serial loop would add, in the
    // same order, while reading the sorted arrays sequentially.
    let tile_rows = (32_768 / width).max(1);
    let tile_len = tile_rows * width;
    let tiles = height.div_ceil(tile_rows);
    let blocks = n.div_ceil(BLOCK);
    let mut counts = vec![0u32; blocks * tiles];
    counts.par_chunks_mut(tiles).zip(cells.par_chunks(BLOCK)).for_each(|(cnt, chunk)| {
        for c in chunk {
            if *c < INVALID {
                cnt[*c as usize / tile_len] += 1;
            }
        }
    });
    let mut starts = vec![0usize; tiles + 1];
    let mut offsets = vec![0usize; blocks * tiles];
    for t in 0..tiles {
        let mut o = starts[t];
        for b in 0..blocks {
            offsets[b * tiles + t] = o;
            o += counts[b * tiles + t] as usize;
        }
        starts[t + 1] = o;
    }
    let total = starts[tiles];
    let mut sorted_cells: Vec<u32> = mem::zeroed_u32(total);
    let mut sorted_weights: Option<Vec<f64>> = weights.map(|_| mem::zeroed_f64(total));
    {
        let sc = SyncPtr(sorted_cells.as_mut_ptr());
        let sw = sorted_weights.as_mut().map(|v| SyncPtr(v.as_mut_ptr()));
        cells.par_chunks(BLOCK).enumerate().for_each(|(b, chunk)| {
            let mut fill: Vec<usize> = offsets[b * tiles..(b + 1) * tiles].to_vec();
            for (k, c) in chunk.iter().enumerate() {
                if *c < INVALID {
                    let t = *c as usize / tile_len;
                    let pos = fill[t];
                    fill[t] += 1;
                    // SAFETY: `pos` lies in the window reserved for (block b, tile t);
                    // windows partition `0..total`, so no two threads write one slot.
                    unsafe {
                        sc.write(pos, *c - (t * tile_len) as u32);
                        if let Some(p) = &sw {
                            p.write(pos, w_of(b * BLOCK + k));
                        }
                    }
                }
            }
        });
    }
    img.data.par_chunks_mut(tile_len).enumerate().for_each(|(t, tile)| {
        let range = starts[t]..starts[t + 1];
        match &sorted_weights {
            Some(w) => {
                for (c, wv) in sorted_cells[range.clone()].iter().zip(&w[range]) {
                    tile[*c as usize] += wv;
                }
            }
            None => {
                for c in &sorted_cells[range] {
                    tile[*c as usize] += 1.0;
                }
            }
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
    fn parallel_pairwise_sum_is_bitwise_sequential() {
        fn sequential(v: &[f64]) -> f64 {
            if v.len() <= 128 {
                v.iter().fold(0.0, |s, x| s + x)
            } else {
                let m = v.len() / 2;
                sequential(&v[..m]) + sequential(&v[m..])
            }
        }
        let mut st = crate::rng::Seed(2).stream("sum", &[]);
        let v: Vec<f64> = (0..(3 << 17) + 777).map(|_| 10f64.powf(st.uniform_range(-8.0, 8.0))).collect();
        assert_eq!(pairwise_sum(&v).to_bits(), sequential(&v).to_bits());
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
