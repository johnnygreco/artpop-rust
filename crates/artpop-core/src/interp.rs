//! One-dimensional interpolation with SciPy `interp1d` semantics.
//!
//! - `linear` follows the `numpy.interp` path SciPy uses for finite float data
//!   (exact node values are returned exactly).
//! - `nearest` / `nearest-up` use midpoint boundaries with left/right ties.
//! - `previous` / `next` shift nodes by one ulp, like SciPy.
//! - `zero`, `slinear`, `quadratic`, `cubic`, and integer orders build an
//!   interpolating B-spline with SciPy's `make_interp_spline` knot rules
//!   (not-a-knot for odd order, Greville-like knots for order 2).
//!
//! Inputs must have nondecreasing x; strictly increasing x is required for
//! spline orders >= 1 because the collocation system is otherwise singular.

use crate::error::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InterpKind {
    Linear,
    Nearest,
    NearestUp,
    Previous,
    Next,
    Zero,
    Slinear,
    Quadratic,
    Cubic,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Bounds {
    /// Reject queries outside `[x[0], x[-1]]` (SciPy default).
    Error,
    /// Return separate constants below and above the data range.
    Fill { below: f64, above: f64 },
    /// Extend the boundary polynomial piece.
    Extrapolate,
}

/// `numpy.interp` for sorted `x`; returns `y[0]`/`y[n-1]` outside the range.
#[inline]
pub fn np_interp(x: &[f64], y: &[f64], xq: f64) -> f64 {
    let n = x.len();
    if n == 1 {
        return y[0];
    }
    if xq < x[0] {
        return y[0];
    }
    if xq > x[n - 1] {
        return y[n - 1];
    }
    if xq == x[n - 1] {
        return y[n - 1];
    }
    // Largest j with x[j] <= xq.
    let j = x.partition_point(|v| *v <= xq) - 1;
    if xq == x[j] {
        return y[j];
    }
    let slope = (y[j + 1] - y[j]) / (x[j + 1] - x[j]);
    let mut r = slope * (xq - x[j]) + y[j];
    if r.is_nan() {
        r = slope * (xq - x[j + 1]) + y[j + 1];
        if r.is_nan() && y[j] == y[j + 1] {
            r = y[j];
        }
    }
    r
}

/// Linear extrapolation using the boundary segment (SciPy `_call_linear`).
fn linear_any(x: &[f64], y: &[f64], xq: f64) -> f64 {
    let n = x.len();
    let idx = x.partition_point(|v| *v < xq).clamp(1, n - 1);
    let (lo, hi) = (idx - 1, idx);
    let slope = (y[hi] - y[lo]) / (x[hi] - x[lo]);
    slope * (xq - x[lo]) + y[lo]
}

pub struct Interp1d {
    kind: InterpKind,
    bounds: Bounds,
    x: Vec<f64>,
    y: Vec<f64>,
    shifted: Vec<f64>,
    spline: Option<BSpline>,
}

impl Interp1d {
    pub fn new(x: &[f64], y: &[f64], kind: InterpKind, bounds: Bounds) -> Result<Self> {
        if x.len() != y.len() {
            return Err(Error::new(ErrorCode::ShapeMismatch, "x and y columns must have the same length."));
        }
        let min_points = match kind {
            InterpKind::Quadratic => 3,
            InterpKind::Cubic => 4,
            _ => 1,
        };
        if x.len() < min_points.max(1) {
            return Err(Error::invalid("/x", format!("Interpolation kind {kind:?} needs at least {min_points} points.")));
        }
        crate::error::require_finite(x, "/x", "x column")?;
        crate::error::require_finite(y, "/y", "y column")?;
        if x.windows(2).any(|w| w[1] < w[0]) {
            return Err(Error::invalid("/x", "x values must be nondecreasing; this interpolator does not reorder data."));
        }
        let shifted = match kind {
            InterpKind::Previous => x.iter().map(|v| next_toward(*v, f64::NEG_INFINITY)).collect(),
            InterpKind::Next => x.iter().map(|v| next_toward(*v, f64::INFINITY)).collect(),
            _ => Vec::new(),
        };
        let spline = match kind {
            InterpKind::Zero => Some(BSpline::interpolate(x, y, 0)?),
            InterpKind::Slinear => Some(BSpline::interpolate(x, y, 1)?),
            InterpKind::Quadratic => Some(BSpline::interpolate(x, y, 2)?),
            InterpKind::Cubic => Some(BSpline::interpolate(x, y, 3)?),
            _ => None,
        };
        Ok(Interp1d { kind, bounds, x: x.to_vec(), y: y.to_vec(), shifted, spline })
    }

    /// Integer spline order (SciPy accepts an integer `kind`).
    pub fn with_order(x: &[f64], y: &[f64], order: usize, bounds: Bounds) -> Result<Self> {
        let mut s = Interp1d::new(x, y, InterpKind::Linear, bounds)?;
        if x.len() < order + 1 {
            return Err(Error::invalid("/order", "Spline order must be smaller than the number of points."));
        }
        s.kind = InterpKind::Slinear;
        s.spline = Some(BSpline::interpolate(x, y, order)?);
        Ok(s)
    }

    pub fn eval(&self, xq: f64) -> Result<f64> {
        let n = self.x.len();
        let below = xq < self.x[0];
        let above = xq > self.x[n - 1];
        if !xq.is_finite() {
            return Err(Error::nonfinite("/x_new", "Interpolation queries must be finite."));
        }
        if below || above {
            match self.bounds {
                Bounds::Error => {
                    return Err(Error::domain(
                        "/x_new",
                        format!("Query {xq} lies outside the interpolation range [{}, {}].", self.x[0], self.x[n - 1]),
                    )
                    .with_details(serde_json::json!({ "query": xq, "minimum": self.x[0], "maximum": self.x[n - 1] })));
                }
                Bounds::Fill { below: b, above: a } => return Ok(if below { b } else { a }),
                Bounds::Extrapolate => {}
            }
        }
        Ok(match self.kind {
            InterpKind::Linear => {
                if n == 1 {
                    self.y[0]
                } else if below || above {
                    linear_any(&self.x, &self.y, xq)
                } else {
                    np_interp(&self.x, &self.y, xq)
                }
            }
            InterpKind::Nearest | InterpKind::NearestUp => {
                if n == 1 {
                    return Ok(self.y[0]);
                }
                // Boundaries are midpoints; left ties for nearest, right for nearest-up.
                let idx = if self.kind == InterpKind::Nearest {
                    count_midpoints(&self.x, |m| m < xq)
                } else {
                    count_midpoints(&self.x, |m| m <= xq)
                };
                self.y[idx.min(n - 1)]
            }
            InterpKind::Previous => {
                let idx = self.shifted.partition_point(|v| *v < xq).clamp(1, n);
                self.y[idx - 1]
            }
            InterpKind::Next => {
                let idx = self.shifted.partition_point(|v| *v <= xq).clamp(0, n - 1);
                self.y[idx]
            }
            _ => self.spline.as_ref().expect("spline built").eval(xq),
        })
    }
}

fn count_midpoints(x: &[f64], pred: impl Fn(f64) -> bool) -> usize {
    // Number of midpoints satisfying `pred` (midpoints are nondecreasing).
    let n = x.len();
    let (mut lo, mut hi) = (0usize, n - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if pred((x[mid + 1] + x[mid]) / 2.0) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

fn next_toward(v: f64, dir: f64) -> f64 {
    if v.is_nan() || v == dir {
        return v;
    }
    if v == 0.0 {
        let tiny = f64::from_bits(1);
        return if dir > 0.0 { tiny } else { -tiny };
    }
    let bits = v.to_bits();
    let up = (dir > v) == (v > 0.0);
    f64::from_bits(if up { bits + 1 } else { bits - 1 })
}

/// Interpolating B-spline (coefficients `c`, knots `t`, degree `k`).
#[derive(Debug, Clone)]
pub struct BSpline {
    t: Vec<f64>,
    c: Vec<f64>,
    k: usize,
}

impl BSpline {
    pub fn interpolate(x: &[f64], y: &[f64], k: usize) -> Result<BSpline> {
        let n = x.len();
        if k == 0 {
            let mut t = x.to_vec();
            t.push(x[n - 1]);
            return Ok(BSpline { t, c: y.to_vec(), k });
        }
        if x.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Error::invalid("/x", "Spline interpolation requires strictly increasing x values."));
        }
        if k == 1 {
            let mut t = vec![x[0]];
            t.extend_from_slice(x);
            t.push(x[n - 1]);
            return Ok(BSpline { t, c: y.to_vec(), k });
        }
        let t: Vec<f64> = if k == 2 {
            let mid: Vec<f64> = x.windows(2).map(|w| (w[1] + w[0]) / 2.0).collect();
            let mut t = vec![x[0]; k + 1];
            t.extend_from_slice(&mid[1..mid.len() - 1]);
            t.extend(std::iter::repeat_n(x[n - 1], k + 1));
            t
        } else {
            if k % 2 != 1 {
                return Err(Error::invalid("/order", "Not-a-knot spline interpolation requires an odd order (or 2)."));
            }
            let m = (k - 1) / 2;
            let mut t = vec![x[0]; k + 1];
            t.extend_from_slice(&x[m + 1..n - m - 1]);
            t.extend(std::iter::repeat_n(x[n - 1], k + 1));
            t
        };
        if t.len() != n + k + 1 {
            return Err(Error::internal("Inconsistent spline knot vector"));
        }
        // Collocation matrix in band storage.
        let mut rows: Vec<(usize, Vec<f64>)> = Vec::with_capacity(n);
        let (mut kl, mut ku) = (0usize, 0usize);
        for (i, &xi) in x.iter().enumerate() {
            let l = find_interval(&t, k, xi);
            let b = basis(&t, k, l, xi);
            let first = l - k;
            kl = kl.max(i.saturating_sub(first));
            ku = ku.max((first + k).saturating_sub(i));
            rows.push((first, b));
        }
        let c = solve_banded(n, kl, ku, &rows, y)?;
        Ok(BSpline { t, c, k })
    }

    pub fn eval(&self, xq: f64) -> f64 {
        let l = find_interval(&self.t, self.k, xq);
        let b = basis(&self.t, self.k, l, xq);
        let first = l - self.k;
        b.iter().enumerate().map(|(j, bj)| bj * self.c[first + j]).sum()
    }
}

/// Interval `l` with `t[l] <= x < t[l+1]`, restricted to `[k, n-1]` where
/// `n = t.len() - k - 1`, extrapolating from the boundary pieces.
fn find_interval(t: &[f64], k: usize, x: f64) -> usize {
    let n = t.len() - k - 1;
    if x >= t[n] {
        // SciPy's `find_interval` returns n - 1 here, even when that interval
        // is degenerate (order 0 then yields the final coefficient).
        return n - 1;
    }
    if x < t[k] {
        return k;
    }
    let l = t.partition_point(|v| *v <= x) - 1;
    l.clamp(k, n - 1)
}

/// The `k + 1` nonzero basis values `B_{l-k..=l}(x)` (Cox–de Boor).
fn basis(t: &[f64], k: usize, l: usize, x: f64) -> Vec<f64> {
    let mut h = vec![0.0; k + 1];
    h[0] = 1.0;
    let mut hh = vec![0.0; k + 1];
    for j in 1..=k {
        hh[..j].copy_from_slice(&h[..j]);
        h[0] = 0.0;
        for n in 1..=j {
            let ind = l + n;
            let xb = t[ind];
            let xa = t[ind - j];
            if xb == xa {
                h[n] = 0.0;
                continue;
            }
            let w = hh[n - 1] / (xb - xa);
            h[n - 1] += w * (xb - x);
            h[n] = w * (x - xa);
        }
    }
    h
}

/// Gaussian elimination with partial pivoting on a banded system.
fn solve_banded(n: usize, kl: usize, ku: usize, rows: &[(usize, Vec<f64>)], rhs: &[f64]) -> Result<Vec<f64>> {
    let width = 2 * kl + ku + 1;
    // a[i][c - i + kl] for c in [i - kl, i + kl + ku].
    let mut a = vec![0.0; n * width];
    let idx = |i: usize, c: usize| -> usize { i * width + (c + kl - i) };
    for (i, (first, vals)) in rows.iter().enumerate() {
        for (j, v) in vals.iter().enumerate() {
            a[idx(i, first + j)] = *v;
        }
    }
    let mut b = rhs.to_vec();
    for col in 0..n {
        let last_row = (col + kl).min(n - 1);
        let mut piv = col;
        let mut best = a[idx(col, col)].abs();
        for r in col + 1..=last_row {
            let v = a[idx(r, col)].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best == 0.0 || !best.is_finite() {
            return Err(Error::new(ErrorCode::NumericalConvergence, "Spline collocation matrix is singular."));
        }
        let last_col = (col + kl + ku).min(n - 1);
        if piv != col {
            for c in col..=last_col {
                let (p, q) = (idx(piv, c), idx(col, c));
                a.swap(p, q);
            }
            b.swap(piv, col);
        }
        let d = a[idx(col, col)];
        for r in col + 1..=last_row {
            let f = a[idx(r, col)] / d;
            if f == 0.0 {
                continue;
            }
            a[idx(r, col)] = 0.0;
            for c in col + 1..=last_col {
                a[idx(r, c)] -= f * a[idx(col, c)];
            }
            b[r] -= f * b[col];
        }
    }
    let mut xsol = vec![0.0; n];
    for i in (0..n).rev() {
        let last_col = (i + kl + ku).min(n - 1);
        let mut s = b[i];
        for c in i + 1..=last_col {
            s -= a[idx(i, c)] * xsol[c];
        }
        xsol[i] = s / a[idx(i, i)];
    }
    Ok(xsol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_matches_numpy_interp() {
        let x = [0.0, 1.0, 2.0, 4.0];
        let y = [0.0, 10.0, 20.0, 0.0];
        assert_eq!(np_interp(&x, &y, 1.0), 10.0);
        assert_eq!(np_interp(&x, &y, 3.0), 10.0);
        assert_eq!(np_interp(&x, &y, 4.0), 0.0);
        let f = Interp1d::new(&x, &y, InterpKind::Linear, Bounds::Error).unwrap();
        assert!(f.eval(4.5).is_err());
        let e = Interp1d::new(&x, &y, InterpKind::Linear, Bounds::Extrapolate).unwrap();
        assert_eq!(e.eval(5.0).unwrap(), -10.0);
    }

    #[test]
    fn step_kinds_match_scipy() {
        // scipy.interpolate.interp1d([0,1,2,3],[10,11,12,13], kind=...)
        let x = [0.0, 1.0, 2.0, 3.0];
        let y = [10.0, 11.0, 12.0, 13.0];
        let ev = |k| {
            let f = Interp1d::new(&x, &y, k, Bounds::Error).unwrap();
            [0.0, 0.5, 1.0, 1.2, 2.5, 3.0].map(|q| f.eval(q).unwrap())
        };
        assert_eq!(ev(InterpKind::Nearest), [10.0, 10.0, 11.0, 11.0, 12.0, 13.0]);
        assert_eq!(ev(InterpKind::NearestUp), [10.0, 11.0, 11.0, 11.0, 13.0, 13.0]);
        assert_eq!(ev(InterpKind::Previous), [10.0, 10.0, 11.0, 11.0, 12.0, 13.0]);
        assert_eq!(ev(InterpKind::Next), [10.0, 11.0, 11.0, 12.0, 13.0, 13.0]);
        assert_eq!(ev(InterpKind::Zero), [10.0, 10.0, 11.0, 11.0, 12.0, 13.0]);
    }

    #[test]
    fn splines_match_scipy_make_interp_spline() {
        let x: Vec<f64> = (0..9).map(|i| i as f64 * 0.7 + (i * i) as f64 * 0.05).collect();
        let y: Vec<f64> = x.iter().map(|v| v.sin() * 3.0 + v).collect();
        let q = [0.1, 1.3, 2.9, 4.4, 6.0];
        // scipy.interpolate.interp1d(x, y, kind=...)(q), SciPy 1.15.3.
        let expected: [(usize, [f64; 5]); 3] = [
            (2, [0.43217805894572064, 4.1795336329912605, 3.6377390309219546, 1.5551121893050328, 5.161753505403222]),
            (3, [0.41287698026917213, 4.182449841206901, 3.616562968794753, 1.5499613027212775, 5.1617535054032215]),
            (5, [0.39130459259656275, 4.1940581244936945, 3.619214406167763, 1.5435949530923005, 5.161753505403222]),
        ];
        for (order, values) in expected {
            let s = Interp1d::with_order(&x, &y, order, Bounds::Error).unwrap();
            for (xq, want) in q.iter().zip(values) {
                let got = s.eval(*xq).unwrap();
                assert!((got - want).abs() < 1e-12, "order {order} at {xq}: {got} vs {want}");
            }
        }
    }
}
