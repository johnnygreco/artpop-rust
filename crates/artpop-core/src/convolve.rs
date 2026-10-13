//! 2D convolution matching `astropy.convolution.convolve_fft` geometry.
//!
//! `wrap`: circular convolution on `max(image, kernel)` per axis (the image
//! is centered in the padded domain exactly as Astropy does), cropped back.
//! `fill`: zero-padded linear convolution. The kernel's origin is index
//! `size / 2` along each axis. Direct summation is the oracle; the FFT path
//! (RustFFT) pads `fill` to 2/3/5-smooth sizes and normalizes the inverse
//! transform by the number of transformed pixels.

use crate::error::{Error, ErrorCode, Result};
use crate::image::Image;
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Wrap,
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Auto,
    Direct,
    Fft,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvolveInfo {
    pub method: Method,
    pub boundary: Boundary,
    pub domain: [usize; 2],
    pub kernel_sum_before_normalization: f64,
    pub input_sum: f64,
    pub output_sum: f64,
}

/// Choose direct summation when it needs fewer than ~4 FFT-equivalent passes.
pub fn resolve_method(method: Method, image: (usize, usize), kernel: (usize, usize)) -> Method {
    match method {
        Method::Auto => {
            let direct = (image.0 * image.1 * kernel.0 * kernel.1) as f64;
            let n = ((image.0 + kernel.0) * (image.1 + kernel.1)) as f64;
            let fft = 6.0 * n * n.log2().max(1.0);
            if direct <= fft { Method::Direct } else { Method::Fft }
        }
        m => m,
    }
}

pub fn convolve(image: &Image, kernel: &Image, boundary: Boundary, method: Method, normalize_kernel: bool) -> Result<(Image, ConvolveInfo)> {
    crate::error::require_finite_par(&image.data, "/image", "image")?;
    crate::error::require_finite(&kernel.data, "/psf", "kernel")?;
    let ksum = crate::image::pairwise_sum(&kernel.data);
    let kdata: Vec<f64> = if normalize_kernel {
        if ksum.abs() < 1.0 / 100.0 {
            return Err(Error::invalid("/psf", "The kernel cannot be normalized because its sum is close to zero.")
                .with_details(serde_json::json!({ "kernel_sum": ksum })));
        }
        kernel.data.iter().map(|v| v / ksum).collect()
    } else {
        kernel.data.clone()
    };
    let k = Image { width: kernel.width, height: kernel.height, data: kdata };
    let method = resolve_method(method, (image.width, image.height), (k.width, k.height));
    let (out, domain) = match (boundary, method) {
        (Boundary::Wrap, Method::Direct) => direct_wrap(image, &k),
        (Boundary::Fill, Method::Direct) => (direct_fill(image, &k), [image.width, image.height]),
        (b, _) if k.width <= MAX_TILED_KERNEL && k.height <= MAX_TILED_KERNEL && (b == Boundary::Fill || (k.width <= image.width && k.height <= image.height)) => {
            (fft_tiled(image, &k, b), [image.width, image.height])
        }
        (b, _) => fft_convolve(image, &k, b)?,
    };
    let info = ConvolveInfo {
        method,
        boundary,
        domain,
        kernel_sum_before_normalization: ksum,
        input_sum: image.sum(),
        output_sum: out.sum(),
    };
    Ok((out, info))
}

/// Astropy's padded wrap domain and the image's offset within it.
fn wrap_domain(image: &Image, k: &Image) -> ([usize; 2], [usize; 2]) {
    let nw = image.width.max(k.width);
    let nh = image.height.max(k.height);
    let off = |n: usize, a: usize| -> usize {
        let center = n - n.div_ceil(2);
        center - a / 2
    };
    ([nw, nh], [off(nw, image.width), off(nh, image.height)])
}

fn direct_wrap(image: &Image, k: &Image) -> (Image, [usize; 2]) {
    let ([nw, nh], [ox, oy]) = wrap_domain(image, k);
    let (kcx, kcy) = (k.width / 2, k.height / 2);
    let mut big = vec![0.0; nw * nh];
    for y in 0..image.height {
        big[(y + oy) * nw + ox..(y + oy) * nw + ox + image.width].copy_from_slice(&image.data[y * image.width..(y + 1) * image.width]);
    }
    let mut out = Image::zeros(image.width, image.height);
    use rayon::prelude::*;
    out.data.par_chunks_mut(image.width).enumerate().for_each(|(y, orow)| {
        let by = y + oy;
        for (x, o) in orow.iter_mut().enumerate() {
            let bx = x + ox;
            let mut acc = 0.0;
            for j in 0..k.height {
                let sy = (by as i64 - (j as i64 - kcy as i64)).rem_euclid(nh as i64) as usize;
                let row = &big[sy * nw..(sy + 1) * nw];
                let krow = &k.data[j * k.width..(j + 1) * k.width];
                for (i, kv) in krow.iter().enumerate() {
                    let sx = (bx as i64 - (i as i64 - kcx as i64)).rem_euclid(nw as i64) as usize;
                    acc += kv * row[sx];
                }
            }
            *o = acc;
        }
    });
    (out, [nw, nh])
}

fn direct_fill(image: &Image, k: &Image) -> Image {
    let (kcx, kcy) = (k.width as i64 / 2, k.height as i64 / 2);
    let (w, h) = (image.width as i64, image.height as i64);
    let mut out = Image::zeros(image.width, image.height);
    use rayon::prelude::*;
    out.data.par_chunks_mut(image.width).enumerate().for_each(|(y, orow)| {
        let y = y as i64;
        for (x, o) in orow.iter_mut().enumerate() {
            let x = x as i64;
            let mut acc = 0.0;
            for j in 0..k.height as i64 {
                let sy = y - (j - kcy);
                if sy < 0 || sy >= h {
                    continue;
                }
                for i in 0..k.width as i64 {
                    let sx = x - (i - kcx);
                    if sx < 0 || sx >= w {
                        continue;
                    }
                    acc += k.data[(j * k.width as i64 + i) as usize] * image.data[(sy * w + sx) as usize];
                }
            }
            *o = acc;
        }
    });
    out
}

/// Largest kernel side handled by the tiled overlap-save path.
pub const MAX_TILED_KERNEL: usize = 192;

/// Overlap-save FFT convolution on cache-sized square blocks.
///
/// For a block of side `N` and kernel `kh x kw`, each output tile of
/// `(N-kh+1) x (N-kw+1)` pixels needs an `N x N` input block starting
/// `kh-1-cy` rows and `kw-1-cx` columns before it (boundary rule applied when
/// gathering: periodic for `wrap`, zero for `fill`). The circular convolution
/// of the block with the kernel placed at the origin equals the requested
/// convolution on the non-wrapped part. The kernel spectrum is computed once;
/// two real tiles share one complex transform (real and imaginary parts).
/// Each output pixel's arithmetic is independent of scheduling; work buffers
/// are reused per worker thread and tiles write straight into the output.
fn fft_tiled(image: &Image, k: &Image, boundary: Boundary) -> Image {
    use rayon::prelude::*;
    let kmax = k.width.max(k.height);
    let n = if kmax <= 64 { 256 } else { 512 };
    let (ty, tx) = (n - k.height + 1, n - k.width + 1);
    let (cy, cx) = (k.height / 2, k.width / 2);
    let (sy, sx) = ((k.height - 1 - cy) as i64, (k.width - 1 - cx) as i64);
    let (w, h) = (image.width as i64, image.height as i64);
    let mut planner = FftPlanner::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let scratch_len = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
    // Kernel spectrum with the kernel at the origin.
    let mut kspec = vec![Complex::new(0.0, 0.0); n * n];
    for j in 0..k.height {
        for i in 0..k.width {
            kspec[j * n + i] = Complex::new(k.get(i, j), 0.0);
        }
    }
    {
        let mut t = vec![Complex::new(0.0, 0.0); n * n];
        let mut scratch = vec![Complex::new(0.0, 0.0); scratch_len];
        block_fft(&mut kspec, &mut t, &mut scratch, &fwd, n);
    }
    let tiles_y = (image.height).div_ceil(ty);
    let tiles_x = (image.width).div_ceil(tx);
    let tiles: Vec<(usize, usize)> = (0..tiles_y).flat_map(|a| (0..tiles_x).map(move |b| (a, b))).collect();
    let scale = 1.0 / (n * n) as f64;
    // Copy block row `j` of the tile at (oy, ox) into the real or imaginary
    // parts of `dst` (length n). `dst` holds zeros on entry.
    let gather_row = |oy: i64, ox: i64, j: usize, dst: &mut [Complex<f64>], imag: bool| {
        let yy = oy - sy + j as i64;
        let x0 = ox - sx;
        let put = |dst: &mut [Complex<f64>], src: &[f64]| {
            if imag {
                for (d, s) in dst.iter_mut().zip(src) {
                    d.im = *s;
                }
            } else {
                for (d, s) in dst.iter_mut().zip(src) {
                    d.re = *s;
                }
            }
        };
        match boundary {
            Boundary::Wrap => {
                let row = &image.data[(yy.rem_euclid(h) * w) as usize..][..w as usize];
                let (mut i, mut xx) = (0usize, x0.rem_euclid(w) as usize);
                while i < n {
                    let run = (n - i).min(w as usize - xx);
                    put(&mut dst[i..i + run], &row[xx..xx + run]);
                    i += run;
                    xx = 0;
                }
            }
            Boundary::Fill => {
                if yy < 0 || yy >= h {
                    return;
                }
                let row = &image.data[(yy * w) as usize..][..w as usize];
                let (lo, hi) = (x0.max(0), (x0 + n as i64).min(w));
                if hi > lo {
                    put(&mut dst[(lo - x0) as usize..(hi - x0) as usize], &row[lo as usize..hi as usize]);
                }
            }
        }
    };
    let mut out = Image::zeros(image.width, image.height);
    let out_ptr = OutPtr(out.data.as_mut_ptr());
    let kspec = &kspec;
    tiles
        .par_chunks(2)
        .for_each_init(
            || (vec![Complex::new(0.0, 0.0); n * n], vec![Complex::new(0.0, 0.0); n * n], vec![Complex::new(0.0, 0.0); scratch_len]),
            |(buf, t, scratch), pair| {
                buf.fill(Complex::new(0.0, 0.0));
                for (which, &(a, b)) in pair.iter().enumerate() {
                    let (oy, ox) = ((a * ty) as i64, (b * tx) as i64);
                    for j in 0..n {
                        gather_row(oy, ox, j, &mut buf[j * n..(j + 1) * n], which == 1);
                    }
                }
                block_fft(buf, t, scratch, &fwd, n);
                for (v, kv) in buf.iter_mut().zip(kspec.iter()) {
                    *v *= kv;
                }
                // Inverse of the transposed spectrum returns to row-major order.
                block_fft(buf, t, scratch, &inv, n);
                for (which, &(a, b)) in pair.iter().enumerate() {
                    let (oy, ox) = (a * ty, b * tx);
                    let (th, tw) = (ty.min(image.height - oy), tx.min(image.width - ox));
                    for r in 0..th {
                        let src = &buf[(r + k.height - 1) * n + (k.width - 1)..][..tw];
                        let base = (oy + r) * image.width + ox;
                        for (u, c) in src.iter().enumerate() {
                            // SAFETY: tiles cover disjoint output rectangles, so no
                            // two tasks write the same pixel.
                            unsafe { out_ptr.write(base + u, if which == 0 { c.re } else { c.im } * scale) };
                        }
                    }
                }
            },
        );
    out
}

/// Raw output pointer shared by tile tasks that write disjoint rectangles.
struct OutPtr(*mut f64);

unsafe impl Sync for OutPtr {}
unsafe impl Send for OutPtr {}

impl OutPtr {
    /// # Safety
    /// `i` must be in bounds and written by exactly one task.
    #[inline]
    unsafe fn write(&self, i: usize, v: f64) {
        unsafe { self.0.add(i).write(v) }
    }
}

/// In-place 2D FFT of an `n x n` block: rows, then (transposed) columns. The
/// spectrum is left transposed; applying the same routine with the inverse
/// plan transposes back, so forward/multiply/inverse stays consistent.
fn block_fft(buf: &mut [Complex<f64>], t: &mut [Complex<f64>], scratch: &mut [Complex<f64>], f: &std::sync::Arc<dyn rustfft::Fft<f64>>, n: usize) {
    for r in buf.chunks_exact_mut(n) {
        f.process_with_scratch(r, scratch);
    }
    transpose_square(buf, t, n);
    for c in t.chunks_exact_mut(n) {
        f.process_with_scratch(c, scratch);
    }
    buf.copy_from_slice(t);
}

/// `dst[x * n + y] = src[y * n + x]` in cache-friendly 16x16 blocks.
fn transpose_square(src: &[Complex<f64>], dst: &mut [Complex<f64>], n: usize) {
    const B: usize = 16;
    for y0 in (0..n).step_by(B) {
        for x0 in (0..n).step_by(B) {
            for y in y0..(y0 + B).min(n) {
                let row = &src[y * n..(y + 1) * n];
                for x in x0..(x0 + B).min(n) {
                    dst[x * n + y] = row[x];
                }
            }
        }
    }
}

/// Smallest 2/3/5-smooth integer >= n.
pub fn next_fast_len(n: usize) -> usize {
    let mut m = n.max(1);
    loop {
        let mut r = m;
        for p in [2, 3, 5] {
            while r.is_multiple_of(p) {
                r /= p;
            }
        }
        if r == 1 {
            return m;
        }
        m += 1;
    }
}

/// Parallel 2D FFT: rows, then columns via a blocked transpose. Each 1D
/// transform is independent, so results do not depend on thread count.
fn fft2(data: &mut [Complex<f64>], w: usize, h: usize, inverse: bool, planner: &mut FftPlanner<f64>) {
    use rayon::prelude::*;
    let row = if inverse { planner.plan_fft_inverse(w) } else { planner.plan_fft_forward(w) };
    data.par_chunks_exact_mut(w).for_each(|r| row.process(r));
    let col = if inverse { planner.plan_fft_inverse(h) } else { planner.plan_fft_forward(h) };
    let mut t = vec![Complex::new(0.0, 0.0); w * h];
    transpose(data, &mut t, w, h);
    t.par_chunks_exact_mut(h).for_each(|c| col.process(c));
    transpose(&t, data, h, w);
}

/// `dst[x * h + y] = src[y * w + x]` in cache-friendly 32x32 blocks.
fn transpose(src: &[Complex<f64>], dst: &mut [Complex<f64>], w: usize, h: usize) {
    use rayon::prelude::*;
    const B: usize = 32;
    dst.par_chunks_mut(B * h).enumerate().for_each(|(bx, out)| {
        let x0 = bx * B;
        let x1 = (x0 + B).min(w);
        for y0 in (0..h).step_by(B) {
            for y in y0..(y0 + B).min(h) {
                for x in x0..x1 {
                    out[(x - x0) * h + y] = src[y * w + x];
                }
            }
        }
    });
}

fn fft_convolve(image: &Image, k: &Image, boundary: Boundary) -> Result<(Image, [usize; 2])> {
    let (nw, nh, ox, oy) = match boundary {
        Boundary::Wrap => {
            let ([nw, nh], [ox, oy]) = wrap_domain(image, k);
            (nw, nh, ox, oy)
        }
        Boundary::Fill => {
            let nw = next_fast_len(image.width + k.width);
            let nh = next_fast_len(image.height + k.height);
            let off = |n: usize, a: usize| (n - n.div_ceil(2)) - a / 2;
            (nw, nh, off(nw, image.width), off(nh, image.height))
        }
    };
    let cells = nw.checked_mul(nh).ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "FFT domain overflows."))?;
    let mut a = vec![Complex::new(0.0, 0.0); cells];
    for y in 0..image.height {
        for x in 0..image.width {
            a[(y + oy) * nw + x + ox] = Complex::new(image.get(x, y), 0.0);
        }
    }
    // Kernel origin (k.size / 2) moved to domain index 0.
    let mut b = vec![Complex::new(0.0, 0.0); cells];
    let (kcx, kcy) = (k.width as i64 / 2, k.height as i64 / 2);
    for j in 0..k.height {
        for i in 0..k.width {
            let dx = (i as i64 - kcx).rem_euclid(nw as i64) as usize;
            let dy = (j as i64 - kcy).rem_euclid(nh as i64) as usize;
            b[dy * nw + dx] += Complex::new(k.get(i, j), 0.0);
        }
    }
    let mut planner = FftPlanner::new();
    fft2(&mut a, nw, nh, false, &mut planner);
    fft2(&mut b, nw, nh, false, &mut planner);
    for (p, q) in a.iter_mut().zip(&b) {
        *p *= q;
    }
    fft2(&mut a, nw, nh, true, &mut planner);
    let scale = 1.0 / cells as f64;
    let mut out = Image::zeros(image.width, image.height);
    for y in 0..image.height {
        for x in 0..image.width {
            out.data[y * image.width + x] = a[(y + oy) * nw + x + ox].re * scale;
        }
    }
    Ok((out, [nw, nh]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel3() -> Image {
        Image::new(3, 3, vec![0.0, 1.0, 0.0, 2.0, 4.0, 1.0, 0.0, 3.0, 1.0]).unwrap()
    }

    #[test]
    fn direct_and_fft_agree_for_both_boundaries() {
        let mut img = Image::zeros(7, 5);
        img.data[0] = 5.0; // corner impulse
        img.data[2 * 7 + 3] = 2.0;
        img.data[4 * 7 + 6] = 1.0;
        for boundary in [Boundary::Wrap, Boundary::Fill] {
            let (d, _) = convolve(&img, &kernel3(), boundary, Method::Direct, true).unwrap();
            let (f, _) = convolve(&img, &kernel3(), boundary, Method::Fft, true).unwrap();
            for (a, b) in d.data.iter().zip(&f.data) {
                assert!((a - b).abs() < 1e-13, "{boundary:?}");
            }
        }
        let (w, _) = convolve(&img, &kernel3(), Boundary::Wrap, Method::Direct, true).unwrap();
        assert!((w.sum() - img.sum()).abs() < 1e-12, "wrap conserves flux");
        let (f, _) = convolve(&img, &kernel3(), Boundary::Fill, Method::Direct, true).unwrap();
        assert!(f.sum() < img.sum(), "fill loses edge flux");
    }

    #[test]
    fn tiled_overlap_save_matches_direct() {
        let mut st = crate::rng::Seed(5).stream("conv", &[]);
        for (w, h, kw, kh) in [(301usize, 517usize, 41usize, 41usize), (700, 260, 15, 7), (90, 70, 65, 9), (50, 40, 3, 5)] {
            let img = Image::new(w, h, (0..w * h).map(|_| st.uniform() * 100.0).collect()).unwrap();
            let k = Image::new(kw, kh, (0..kw * kh).map(|_| st.uniform()).collect()).unwrap();
            for b in [Boundary::Wrap, Boundary::Fill] {
                if b == Boundary::Wrap && (kw > w || kh > h) {
                    continue;
                }
                let (d, _) = convolve(&img, &k, b, Method::Direct, true).unwrap();
                let (f, _) = convolve(&img, &k, b, Method::Fft, true).unwrap();
                let scale = d.data.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                for (a, c) in d.data.iter().zip(&f.data) {
                    assert!((a - c).abs() <= 1e-12 * scale, "{w}x{h} kernel {kw}x{kh} {b:?}: {a} vs {c}");
                }
            }
        }
    }

    #[test]
    fn impulse_recovers_kernel_orientation() {
        let mut img = Image::zeros(5, 5);
        img.data[2 * 5 + 2] = 1.0;
        let k = kernel3();
        let (out, _) = convolve(&img, &k, Boundary::Fill, Method::Direct, false).unwrap();
        for j in 0..3 {
            for i in 0..3 {
                assert_eq!(out.get(1 + i, 1 + j), k.get(i, j));
            }
        }
    }

    #[test]
    fn matches_astropy_convolve_fft() {
        // astropy.convolution.convolve_fft(img, k, boundary=..., normalize_kernel=True)
        // with img = arange(35).reshape(5, 7) ** 1.5 and the 3x3 kernel above.
        let img = Image::new(7, 5, (0..35).map(|v| (v as f64).powf(1.5)).collect()).unwrap();
        let (w, _) = convolve(&img, &kernel3(), Boundary::Wrap, Method::Direct, true).unwrap();
        let (f, _) = convolve(&img, &kernel3(), Boundary::Fill, Method::Direct, true).unwrap();
        let wrap_expected = [(0usize, 0usize, 56.49631519314523), (6, 4, 153.92199699360822), (3, 2, 61.37917372807826)];
        let fill_expected = [(0usize, 0usize, 1.7100215981210103), (6, 4, 128.00357321894714), (3, 2, 61.379173728078264)];
        for (x, y, v) in wrap_expected {
            assert!((w.get(x, y) - v).abs() < 1e-9, "wrap {x},{y}: {}", w.get(x, y));
        }
        for (x, y, v) in fill_expected {
            assert!((f.get(x, y) - v).abs() < 1e-9, "fill {x},{y}: {}", f.get(x, y));
        }
    }
}
