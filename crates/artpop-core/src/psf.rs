//! Gaussian and Moffat PSF kernels with Astropy's four discretization modes.
//!
//! Kernels are evaluated on `x in [-(nx-1)/2, (nx-1)/2]` (odd sizes), then
//! normalized to unit sum. `integrate` uses an exact erf product for Gaussians
//! and nested adaptive Gauss–Kronrod quadrature for Moffat profiles.

use crate::error::{Error, Result};
use crate::image::Image;
use crate::special::{erf, integrate_gk15};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Discretization {
    Center,
    LinearInterp,
    Oversample,
    Integrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Profile {
    /// `sigma = FWHM / sqrt(8 ln 2)` in pixels.
    Gaussian { sigma: f64 },
    /// `gamma = FWHM / (2 sqrt(2^(1/alpha) - 1))` in pixels.
    Moffat { gamma: f64, alpha: f64 },
}

impl Profile {
    pub fn gaussian_from_fwhm(fwhm_pix: f64) -> Profile {
        Profile::Gaussian { sigma: fwhm_pix / (8.0 * std::f64::consts::LN_2).sqrt() }
    }

    pub fn moffat_from_fwhm(fwhm_pix: f64, alpha: f64) -> Profile {
        Profile::Moffat { gamma: fwhm_pix / (2.0 * (2f64.powf(1.0 / alpha) - 1.0).sqrt()), alpha }
    }

    /// Astropy kernel amplitudes (unit integral over the infinite plane).
    #[inline]
    pub fn eval(&self, x: f64, y: f64) -> f64 {
        match *self {
            Profile::Gaussian { sigma } => {
                let amp = 1.0 / (2.0 * std::f64::consts::PI * sigma * sigma);
                let a = 0.5 / (sigma * sigma);
                amp * (-(a * x * x + a * y * y)).exp()
            }
            Profile::Moffat { gamma, alpha } => {
                let amp = (alpha - 1.0) / (std::f64::consts::PI * gamma * gamma);
                let rr_gg = (x * x + y * y) / (gamma * gamma);
                amp * (1.0 + rr_gg).powf(-alpha)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KernelInfo {
    pub prenormalization_sum: f64,
    pub evaluations: u64,
}

pub struct KernelOptions {
    pub mode: Discretization,
    pub factor: usize,
    pub abs_tol: f64,
    pub rel_tol: f64,
    pub max_depth: u32,
}

impl Default for KernelOptions {
    fn default() -> Self {
        KernelOptions { mode: Discretization::Center, factor: 10, abs_tol: 1e-13, rel_tol: 1e-10, max_depth: 40 }
    }
}

/// numpy.linspace values.
fn linspace(start: f64, stop: f64, num: usize) -> Vec<f64> {
    if num == 1 {
        return vec![start];
    }
    let step = (stop - start) / (num - 1) as f64;
    let mut v: Vec<f64> = (0..num).map(|i| start + i as f64 * step).collect();
    v[num - 1] = stop;
    v
}

pub fn kernel(profile: &Profile, width: usize, height: usize, opts: &KernelOptions) -> Result<(Image, KernelInfo)> {
    if width == 0 || height == 0 || width.is_multiple_of(2) || height.is_multiple_of(2) {
        return Err(Error::invalid("/width", "PSF dimensions must be positive odd integers.")
            .with_details(serde_json::json!({ "width": width, "height": height })));
    }
    match *profile {
        Profile::Gaussian { sigma } => crate::error::require_positive(sigma, "/fwhm", "Gaussian width")?,
        Profile::Moffat { gamma, alpha } => {
            crate::error::require_positive(gamma, "/fwhm", "Moffat width")?;
            if !(alpha > 1.0) || !alpha.is_finite() {
                return Err(Error::invalid("/alpha", "Moffat alpha must exceed 1 for a normalizable profile."));
            }
        }
    }
    let hx = ((width - 1) / 2) as f64;
    let hy = ((height - 1) / 2) as f64;
    let mut data = vec![0.0; width * height];
    let mut evaluations = 0u64;
    match opts.mode {
        Discretization::Center => {
            for j in 0..height {
                for i in 0..width {
                    data[j * width + i] = profile.eval(i as f64 - hx, j as f64 - hy);
                }
            }
            evaluations = (width * height) as u64;
        }
        Discretization::LinearInterp => {
            let (cw, ch) = (width + 1, height + 1);
            let mut corners = vec![0.0; cw * ch];
            for j in 0..ch {
                for i in 0..cw {
                    corners[j * cw + i] = profile.eval(i as f64 - hx - 0.5, j as f64 - hy - 0.5);
                }
            }
            evaluations = (cw * ch) as u64;
            for j in 0..height {
                for i in 0..width {
                    let a = 0.5 * (corners[(j + 1) * cw + i] + corners[j * cw + i]);
                    let b = 0.5 * (corners[(j + 1) * cw + i + 1] + corners[j * cw + i + 1]);
                    data[j * width + i] = 0.5 * (b + a);
                }
            }
        }
        Discretization::Oversample => {
            let f = opts.factor;
            if f == 0 {
                return Err(Error::invalid("/factor", "Oversampling factor must be positive."));
            }
            let ff = f as f64;
            let xs = linspace(-hx - 0.5 * (1.0 - 1.0 / ff), (hx + 1.0) - 0.5 * (1.0 + 1.0 / ff), width * f);
            let ys = linspace(-hy - 0.5 * (1.0 - 1.0 / ff), (hy + 1.0) - 0.5 * (1.0 + 1.0 / ff), height * f);
            for j in 0..height {
                for i in 0..width {
                    // mean over x (axis 3) then over y (axis 1), as numpy.
                    let mut outer = 0.0;
                    for sj in 0..f {
                        let y = ys[j * f + sj];
                        let mut inner = 0.0;
                        for si in 0..f {
                            inner += profile.eval(xs[i * f + si], y);
                        }
                        outer += inner / ff;
                    }
                    data[j * width + i] = outer / ff;
                }
            }
            evaluations = (width * height * f * f) as u64;
        }
        Discretization::Integrate => match *profile {
            Profile::Gaussian { sigma } => {
                let s = sigma * std::f64::consts::SQRT_2;
                let cell = |c: f64| 0.5 * (erf((c + 0.5) / s) - erf((c - 0.5) / s));
                for j in 0..height {
                    let gy = cell(j as f64 - hy);
                    for i in 0..width {
                        data[j * width + i] = cell(i as f64 - hx) * gy;
                    }
                }
            }
            Profile::Moffat { .. } => {
                let counter = std::cell::Cell::new(0u64);
                for j in 0..height {
                    for i in 0..width {
                        let (x0, y0) = (i as f64 - hx - 0.5, j as f64 - hy - 0.5);
                        let inner = |x: f64| -> f64 {
                            integrate_gk15(&|y: f64| {
                                counter.set(counter.get() + 1);
                                profile.eval(x, y)
                            }, y0, y0 + 1.0, opts.abs_tol, opts.rel_tol, opts.max_depth)
                            .unwrap_or(f64::NAN)
                        };
                        let v = integrate_gk15(&inner, x0, x0 + 1.0, opts.abs_tol, opts.rel_tol, opts.max_depth)?;
                        if !v.is_finite() {
                            return Err(Error::new(crate::error::ErrorCode::NumericalConvergence, "Moffat pixel integration did not converge.")
                                .with_details(serde_json::json!({ "pixel": [i, j] })));
                        }
                        data[j * width + i] = v;
                    }
                }
                evaluations = counter.get();
            }
        },
    }
    let total = crate::image::pairwise_sum(&data);
    if !(total > 0.0) || !total.is_finite() {
        return Err(Error::new(crate::error::ErrorCode::NumericalConvergence, "PSF kernel sum is not positive."));
    }
    for v in &mut data {
        *v /= total;
    }
    Ok((Image { width, height, data }, KernelInfo { prenormalization_sum: total, evaluations }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(img: &Image, want: &[f64]) {
        for (a, b) in img.data.iter().zip(want) {
            assert!((a - b).abs() < 5e-8, "{a} vs {b}");
        }
    }

    #[test]
    fn upstream_three_by_three_kernels() {
        // ArtPop tests/test_image.py::test_psf (FWHM 0.3", 0.2"/pix).
        let (m, _) = kernel(&Profile::moffat_from_fwhm(1.5, 4.765), 3, 3, &KernelOptions::default()).unwrap();
        close(&m, &[0.04451139, 0.11380315, 0.04451139, 0.11380315, 0.36674184, 0.11380315, 0.04451139, 0.11380315, 0.04451139]);
        let (g, _) = kernel(&Profile::gaussian_from_fwhm(1.5), 3, 3, &KernelOptions::default()).unwrap();
        close(&g, &[0.03392846, 0.11633988, 0.03392846, 0.11633988, 0.39892664, 0.11633988, 0.03392846, 0.11633988, 0.03392846]);
    }

    #[test]
    fn modes_match_astropy_kernels() {
        // astropy 6.1.7 Gaussian2DKernel / Moffat2DKernel(...).normalize()
        let g = Profile::gaussian_from_fwhm(2.0);
        let (lin, _) = kernel(&g, 7, 7, &KernelOptions { mode: Discretization::LinearInterp, ..Default::default() }).unwrap();
        assert!((lin.get(3, 3) - 0.15604366677489537).abs() < 1e-14);
        assert!((lin.get(1, 0) - 0.00016444039515138567).abs() < 1e-16);
        let (over, _) = kernel(&g, 7, 7, &KernelOptions { mode: Discretization::Oversample, factor: 10, ..Default::default() }).unwrap();
        assert!((over.get(3, 3) - 0.1973010340078962).abs() < 1e-14);
        assert!((over.get(1, 0) - 5.8999214563597726e-05).abs() < 1e-16);
        // Astropy integrates with dblquad (absolute tolerance ~1.5e-8).
        let m = Profile::moffat_from_fwhm(2.5, 3.0);
        let (int, _) = kernel(&m, 11, 9, &KernelOptions { mode: Discretization::Integrate, ..Default::default() }).unwrap();
        assert!((int.get(5, 4) - 0.10070608651009995).abs() < 1e-7);
        assert!((int.get(0, 0) - 0.0002343367989443215).abs() < 1e-8);
    }

    #[test]
    fn modes_are_normalized_and_converge_to_integral() {
        let p = Profile::moffat_from_fwhm(2.5, 3.0);
        let mut opts = KernelOptions { mode: Discretization::Integrate, ..Default::default() };
        let (exact, _) = kernel(&p, 11, 9, &opts).unwrap();
        opts.mode = Discretization::Oversample;
        opts.factor = 40;
        let (over, _) = kernel(&p, 11, 9, &opts).unwrap();
        for (a, b) in exact.data.iter().zip(&over.data) {
            assert!((a - b).abs() < 2e-5);
        }
        assert!((exact.sum() - 1.0).abs() < 1e-12);
        let g = Profile::gaussian_from_fwhm(2.0);
        let (gi, _) = kernel(&g, 7, 7, &KernelOptions { mode: Discretization::Integrate, ..Default::default() }).unwrap();
        let (go, _) = kernel(&g, 7, 7, &KernelOptions { mode: Discretization::Oversample, factor: 60, ..Default::default() }).unwrap();
        for (a, b) in gi.data.iter().zip(&go.data) {
            assert!((a - b).abs() < 5e-5);
        }
        assert!(kernel(&g, 4, 5, &KernelOptions::default()).is_err());
    }
}
