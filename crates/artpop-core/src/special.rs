//! Special functions needed by Sérsic normalization and PSF integration.

use crate::error::{Error, ErrorCode, Result};

pub fn ln_gamma(x: f64) -> f64 {
    libm::lgamma(x)
}

pub fn gamma(x: f64) -> f64 {
    libm::tgamma(x)
}

pub fn erf(x: f64) -> f64 {
    libm::erf(x)
}

/// Regularized lower incomplete gamma function `P(a, x)` for `a > 0, x >= 0`.
///
/// Series expansion below `x < a + 1`, Lentz continued fraction otherwise;
/// both iterate to double precision.
pub fn gamma_p(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x.is_infinite() {
        return 1.0;
    }
    let gln = ln_gamma(a);
    if x < a + 1.0 {
        let mut ap = a;
        let mut sum = 1.0 / a;
        let mut del = sum;
        for _ in 0..100_000 {
            ap += 1.0;
            del *= x / ap;
            sum += del;
            if del.abs() < sum.abs() * 1e-17 {
                break;
            }
        }
        (sum.ln() - x + a * x.ln() - gln).exp().min(1.0)
    } else {
        1.0 - gamma_q_continued_fraction(a, x, gln)
    }
}

/// Regularized upper incomplete gamma `Q(a, x) = 1 - P(a, x)`.
pub fn gamma_q(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        1.0 - gamma_p(a, x)
    } else {
        gamma_q_continued_fraction(a, x, ln_gamma(a))
    }
}

fn gamma_q_continued_fraction(a: f64, x: f64, gln: f64) -> f64 {
    const FPMIN: f64 = f64::MIN_POSITIVE / f64::EPSILON;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / FPMIN;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..100_000 {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = b + an / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-17 {
            break;
        }
    }
    ((-x + a * x.ln() - gln).exp() * h).clamp(0.0, 1.0)
}

/// Inverse of `P(a, x) = p` in `x` (SciPy's `gammaincinv`).
///
/// A bracket is grown until it contains the root, then safeguarded Newton
/// steps (falling back to bisection) converge to relative tolerance 1e-15.
pub fn gamma_p_inv(a: f64, p: f64) -> Result<f64> {
    if !(a > 0.0) || !a.is_finite() {
        return Err(Error::new(ErrorCode::OutOfDomain, "gamma_p_inv requires a > 0"));
    }
    if !(0.0..=1.0).contains(&p) {
        return Err(Error::new(ErrorCode::OutOfDomain, "gamma_p_inv requires 0 <= p <= 1"));
    }
    if p == 0.0 {
        return Ok(0.0);
    }
    if p == 1.0 {
        return Ok(f64::INFINITY);
    }
    let gln = ln_gamma(a);
    let mut lo = 0.0;
    let mut hi = a.max(1.0);
    let mut guard = 0;
    while gamma_p(a, hi) < p {
        lo = hi;
        hi *= 2.0;
        guard += 1;
        if guard > 2000 {
            return Err(Error::new(ErrorCode::NumericalConvergence, "Could not bracket gamma_p_inv root"));
        }
    }
    let mut x = 0.5 * (lo + hi);
    for _ in 0..500 {
        let f = gamma_p(a, x) - p;
        if f == 0.0 {
            return Ok(x);
        }
        if f < 0.0 {
            lo = x;
        } else {
            hi = x;
        }
        let deriv = ((a - 1.0) * x.ln() - x - gln).exp();
        let mut next = if deriv > 0.0 && deriv.is_finite() { x - f / deriv } else { f64::NAN };
        if !(next > lo && next < hi) {
            next = 0.5 * (lo + hi);
        }
        if (next - x).abs() <= 1e-15 * x.abs() || hi - lo <= 4.0 * f64::EPSILON * x.abs() {
            return Ok(next);
        }
        x = next;
    }
    Err(Error::new(ErrorCode::NumericalConvergence, "gamma_p_inv did not converge"))
}

/// Sérsic `b_n` such that `P(2n, b_n) = 1/2`.
pub fn sersic_bn(n: f64) -> Result<f64> {
    gamma_p_inv(2.0 * n, 0.5)
}

/// Adaptive Gauss–Kronrod (7, 15) integration of `f` over `[a, b]`.
#[allow(clippy::excessive_precision)]
pub fn integrate_gk15<F: Fn(f64) -> f64>(f: &F, a: f64, b: f64, abs_tol: f64, rel_tol: f64, max_depth: u32) -> Result<f64> {
    const XGK: [f64; 8] = [
        0.991455371120812639206854697526329,
        0.949107912342758524526189684047851,
        0.864864423359769072789712788640926,
        0.741531185599394439863864773280788,
        0.586087235467691130294144845693013,
        0.405845151377397166906606412076961,
        0.207784955007898467600689403773245,
        0.000000000000000000000000000000000,
    ];
    const WGK: [f64; 8] = [
        0.022935322010529224963732008058970,
        0.063092092629978553290700663189204,
        0.104790010322250183839876322541518,
        0.140653259715525918745189590510238,
        0.169004726639267902826583426598550,
        0.190350578064785409913256402421014,
        0.204432940075298892414161999234649,
        0.209482141084727828012999174891714,
    ];
    const WG: [f64; 4] = [
        0.129484966168869693270611432679082,
        0.279705391489276667901467771423780,
        0.381830050505118944950369775488975,
        0.417959183673469387755102040816327,
    ];
    fn rule<F: Fn(f64) -> f64>(f: &F, a: f64, b: f64) -> (f64, f64) {
        let c = 0.5 * (a + b);
        let h = 0.5 * (b - a);
        let fc = f(c);
        let mut kronrod = WGK[7] * fc;
        let mut gauss = WG[3] * fc;
        for j in 0..7 {
            let dx = h * XGK[j];
            let s = f(c - dx) + f(c + dx);
            kronrod += WGK[j] * s;
            if j % 2 == 1 {
                gauss += WG[j / 2] * s;
            }
        }
        (kronrod * h, ((kronrod - gauss) * h).abs())
    }
    fn recurse<F: Fn(f64) -> f64>(f: &F, a: f64, b: f64, abs_tol: f64, rel_tol: f64, depth: u32) -> Result<f64> {
        let (value, err) = rule(f, a, b);
        if err <= abs_tol.max(rel_tol * value.abs()) {
            return Ok(value);
        }
        if depth == 0 {
            return Err(Error::new(ErrorCode::NumericalConvergence, "Adaptive quadrature exceeded its subdivision limit")
                .with_details(serde_json::json!({ "interval": [a, b], "estimated_error": err })));
        }
        let m = 0.5 * (a + b);
        Ok(recurse(f, a, m, 0.5 * abs_tol, rel_tol, depth - 1)? + recurse(f, m, b, 0.5 * abs_tol, rel_tol, depth - 1)?)
    }
    recurse(f, a, b, abs_tol, rel_tol, max_depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_gamma_reference_values() {
        // Reference values from scipy.special.gammainc / gammaincinv 1.15.3.
        assert!((gamma_p(2.0, 1.0) - 0.2642411176571153).abs() < 1e-15);
        assert!((gamma_p(0.5, 2.0) - 0.9544997361036415).abs() < 1e-14);
        assert!((gamma_p(10.0, 20.0) - 0.9950045876916924).abs() < 1e-14);
        assert!((sersic_bn(1.0).unwrap() - 1.6783469900166612).abs() < 1e-13);
        assert!((sersic_bn(4.0).unwrap() - 7.669249442500804).abs() < 1e-12);
        assert!((sersic_bn(0.5).unwrap() - 0.6931471805599455).abs() < 1e-13);
        assert!((sersic_bn(0.2).unwrap() - 0.14507814164343544).abs() < 1e-13);
        assert!((sersic_bn(8.0).unwrap() - 15.667929544317243).abs() < 1e-11);
    }

    #[test]
    fn gauss_kronrod_integrates_gaussian() {
        let v = integrate_gk15(&|x: f64| (-x * x).exp(), -8.0, 8.0, 1e-14, 1e-13, 30).unwrap();
        assert!((v - std::f64::consts::PI.sqrt()).abs() < 1e-12);
    }
}
