//! Display products: percentile-clipped PNG previews, RGB composites, and
//! SVG diagnostic plots. These never replace the f64 science data.

use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::Image;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stretch {
    Linear,
    Sqrt,
    Log,
    Asinh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Colormap {
    Gray,
    GrayR,
    Viridis,
    Magma,
    Inferno,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Row y = 0 at the bottom (Matplotlib `origin='lower'`, ArtPop default).
    Lower,
    Upper,
}

/// numpy `nanpercentile` with linear interpolation.
pub fn nanpercentile(values: &[f64], q: f64) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    Some(v[lo] + (v[hi] - v[lo]) * (pos - lo as f64))
}

fn stretch(t: f64, s: Stretch) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match s {
        Stretch::Linear => t,
        Stretch::Sqrt => t.sqrt(),
        Stretch::Log => (1.0 + 1000.0 * t).ln() / 1001f64.ln(),
        Stretch::Asinh => (10.0 * t).asinh() / 10f64.asinh(),
    }
}

/// Polynomial fits of the perceptual colormaps (Matt Zucker, CC0).
#[allow(clippy::excessive_precision)]
fn colormap(t: f64, c: Colormap) -> [u8; 3] {
    let poly = |k: [[f64; 3]; 7]| -> [u8; 3] {
        let mut out = [0u8; 3];
        for ch in 0..3 {
            let mut v = k[6][ch];
            for i in (0..6).rev() {
                v = v * t + k[i][ch];
            }
            out[ch] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        out
    };
    match c {
        Colormap::Gray => {
            let g = (t * 255.0).round() as u8;
            [g, g, g]
        }
        Colormap::GrayR => {
            let g = ((1.0 - t) * 255.0).round() as u8;
            [g, g, g]
        }
        Colormap::Viridis => poly([
            [0.2777273272234177, 0.005407344544966578, 0.3340998053353061],
            [0.1050930431085774, 1.404613529898575, 1.384590162594685],
            [-0.3308618287255563, 0.214847559468213, 0.09509516302823659],
            [-4.634230498983486, -5.799100973351585, -19.33244095627987],
            [6.228269936347081, 14.17993336680509, 56.69055260068105],
            [4.776384997670288, -13.74514537774601, -65.35303263337234],
            [-5.435455855934631, 4.645852612178535, 26.3124352495832],
        ]),
        Colormap::Magma => poly([
            [-0.002136485053939582, -0.000749655052795221, -0.005386127855323933],
            [0.2516605407371642, 0.6775232436837668, 2.494026599312351],
            [8.353717279216625, -3.577719514958484, 0.3144679030132573],
            [-27.66873308576866, 14.26473078096533, -13.64921318813922],
            [52.17613981234068, -27.94360607168351, 12.94416944238394],
            [-50.76852536473588, 29.04658282127291, 4.23415299384598],
            [18.65570506591883, -11.48977351997711, -5.601961508734096],
        ]),
        Colormap::Inferno => poly([
            [0.0002189403691192265, 0.001651004631001012, -0.01948089843709184],
            [0.1065134194856116, 0.5639564367884091, 3.932712388889277],
            [11.60249308247187, -3.972853965665698, -15.9423941062914],
            [-41.70399613139459, 17.43639888205313, 44.35414519872813],
            [77.162935699427, -33.40235894210092, -81.80730925738993],
            [-71.31942824499214, 32.62606426397723, 73.20951985803202],
            [25.13112622477341, -12.24266895238567, -23.07032500287172],
        ]),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayOptions {
    pub percentile_low: f64,
    pub percentile_high: f64,
    pub stretch: Stretch,
    pub colormap: Colormap,
    pub origin: Origin,
    /// Maximum output side; larger images are block-averaged.
    pub max_size: usize,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        DisplayOptions { percentile_low: 0.1, percentile_high: 99.9, stretch: Stretch::Linear, colormap: Colormap::GrayR, origin: Origin::Lower, max_size: 1024 }
    }
}

/// Block-average to fit within `max_size` (non-finite values ignored).
fn downsample(img: &Image, max_size: usize) -> (Image, usize) {
    let f = img.width.max(img.height).div_ceil(max_size.max(1)).max(1);
    if f == 1 {
        return (img.clone(), 1);
    }
    let (w, h) = (img.width.div_ceil(f), img.height.div_ceil(f));
    let mut out = Image::zeros(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut n) = (0.0, 0);
            for yy in y * f..((y + 1) * f).min(img.height) {
                for xx in x * f..((x + 1) * f).min(img.width) {
                    let v = img.get(xx, yy);
                    if v.is_finite() {
                        s += v;
                        n += 1;
                    }
                }
            }
            out.data[y * w + x] = if n > 0 { s / n as f64 } else { f64::NAN };
        }
    }
    (out, f)
}

fn encode_png(width: usize, height: usize, rgb: &[u8]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| Error::new(ErrorCode::IoError, format!("PNG encoding failed: {e}")))?;
        w.write_image_data(rgb).map_err(|e| Error::new(ErrorCode::IoError, format!("PNG encoding failed: {e}")))?;
    }
    Ok(buf)
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewInfo {
    pub width: usize,
    pub height: usize,
    pub block_factor: usize,
    pub vmin: f64,
    pub vmax: f64,
}

pub fn render_png(img: &Image, opts: &DisplayOptions) -> Result<(Vec<u8>, PreviewInfo)> {
    if !(0.0..=100.0).contains(&opts.percentile_low) || !(0.0..=100.0).contains(&opts.percentile_high) || opts.percentile_low >= opts.percentile_high {
        return Err(Error::invalid("/percentiles", "Percentiles must satisfy 0 <= low < high <= 100."));
    }
    let (d, f) = downsample(img, opts.max_size);
    let vmin = nanpercentile(&d.data, opts.percentile_low).unwrap_or(0.0);
    let vmax = nanpercentile(&d.data, opts.percentile_high).unwrap_or(1.0);
    let span = if vmax > vmin { vmax - vmin } else { 1.0 };
    let mut rgb = vec![0u8; d.width * d.height * 3];
    for row in 0..d.height {
        let y = match opts.origin {
            Origin::Lower => d.height - 1 - row,
            Origin::Upper => row,
        };
        for x in 0..d.width {
            let v = d.get(x, y);
            let c = if v.is_finite() { colormap(stretch((v - vmin) / span, opts.stretch), opts.colormap) } else { [255, 0, 255] };
            let o = (row * d.width + x) * 3;
            rgb[o..o + 3].copy_from_slice(&c);
        }
    }
    Ok((encode_png(d.width, d.height, &rgb)?, PreviewInfo { width: d.width, height: d.height, block_factor: f, vmin, vmax }))
}

/// Lupton-style RGB composite: per-channel scaling, then a shared asinh
/// stretch of the summed intensity.
pub fn render_rgb(planes: [&Image; 3], scales: [f64; 3], q: f64, stretch_value: f64, minimum: f64, origin: Origin, max_size: usize) -> Result<(Vec<u8>, PreviewInfo)> {
    let (w, h) = (planes[0].width, planes[0].height);
    if planes.iter().any(|p| p.width != w || p.height != h) {
        return Err(Error::new(ErrorCode::ShapeMismatch, "RGB planes must have identical shapes."));
    }
    let ds: Vec<(Image, usize)> = planes.iter().map(|p| downsample(p, max_size)).collect();
    let (dw, dh, f) = (ds[0].0.width, ds[0].0.height, ds[0].1);
    let mut rgb = vec![0u8; dw * dh * 3];
    for row in 0..dh {
        let y = if origin == Origin::Lower { dh - 1 - row } else { row };
        for x in 0..dw {
            let v: Vec<f64> = (0..3).map(|c| ((ds[c].0.get(x, y) * scales[c]) - minimum).max(0.0)).collect();
            let i = (v[0] + v[1] + v[2]) / 3.0;
            let fac = if i > 0.0 { (q * i / stretch_value).asinh() / q / i } else { 0.0 };
            let mut px: Vec<f64> = v.iter().map(|c| c * fac).collect();
            let m = px.iter().cloned().fold(1.0, f64::max);
            for p in &mut px {
                *p /= m;
            }
            let o = (row * dw + x) * 3;
            for c in 0..3 {
                rgb[o + c] = (px[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    Ok((encode_png(dw, dh, &rgb)?, PreviewInfo { width: dw, height: dh, block_factor: f, vmin: minimum, vmax: f64::NAN }))
}

pub struct Series<'a> {
    pub x: &'a [f64],
    pub y: &'a [f64],
    pub label: String,
    pub color: &'a str,
    pub line: bool,
}

/// Minimal SVG scatter/line plot with axes, ticks, labels, and legend.
pub fn render_svg(series: &[Series], x_label: &str, y_label: &str, title: &str, invert_y: bool, log_x: bool, log_y: bool) -> Result<String> {
    let tx = |v: f64| if log_x { v.log10() } else { v };
    let ty = |v: f64| if log_y { v.log10() } else { v };
    let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for s in series {
        for (x, y) in s.x.iter().zip(s.y) {
            let (a, b) = (tx(*x), ty(*y));
            if a.is_finite() && b.is_finite() {
                x0 = x0.min(a);
                x1 = x1.max(a);
                y0 = y0.min(b);
                y1 = y1.max(b);
            }
        }
    }
    if !x0.is_finite() {
        return Err(Error::invalid("/data", "Nothing finite to plot."));
    }
    if x1 == x0 {
        x1 = x0 + 1.0;
    }
    if y1 == y0 {
        y1 = y0 + 1.0;
    }
    let (pw, ph, ml, mt) = (640.0, 480.0, 80.0, 40.0);
    let px = |v: f64| ml + (tx(v) - x0) / (x1 - x0) * pw;
    let py = |v: f64| {
        let t = (ty(v) - y0) / (y1 - y0);
        mt + if invert_y { t * ph } else { (1.0 - t) * ph }
    };
    let mut s = String::new();
    let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" font-family="sans-serif" font-size="13">"#, pw + ml + 30.0, ph + mt + 70.0);
    let _ = write!(s, r#"<rect x="{ml}" y="{mt}" width="{pw}" height="{ph}" fill="white" stroke="black" stroke-width="2"/>"#);
    for k in 0..=5 {
        let fx = x0 + (x1 - x0) * k as f64 / 5.0;
        let fy = y0 + (y1 - y0) * k as f64 / 5.0;
        let gx = ml + pw * k as f64 / 5.0;
        let gy = if invert_y { mt + ph * k as f64 / 5.0 } else { mt + ph * (1.0 - k as f64 / 5.0) };
        let lx = if log_x { format!("1e{fx:.2}") } else { format!("{fx:.4}") };
        let ly = if log_y { format!("1e{fy:.2}") } else { format!("{fy:.4}") };
        let _ = write!(s, r#"<line x1="{gx}" y1="{}" x2="{gx}" y2="{}" stroke="black"/><text x="{gx}" y="{}" text-anchor="middle">{lx}</text>"#, mt + ph, mt + ph - 8.0, mt + ph + 18.0);
        let _ = write!(s, r#"<line x1="{ml}" y1="{gy}" x2="{}" y2="{gy}" stroke="black"/><text x="{}" y="{}" text-anchor="end">{ly}</text>"#, ml + 8.0, ml - 6.0, gy + 4.0);
    }
    for (k, ser) in series.iter().enumerate() {
        if ser.line {
            let mut d = String::new();
            for (x, y) in ser.x.iter().zip(ser.y) {
                if tx(*x).is_finite() && ty(*y).is_finite() {
                    let _ = write!(d, "{}{:.2},{:.2} ", if d.is_empty() { "M" } else { "L" }, px(*x), py(*y));
                }
            }
            let _ = write!(s, r#"<path d="{d}" fill="none" stroke="{}" stroke-width="2"/>"#, ser.color);
        } else {
            let _ = write!(s, r#"<g fill="{}" fill-opacity="0.5">"#, ser.color);
            for (x, y) in ser.x.iter().zip(ser.y) {
                if tx(*x).is_finite() && ty(*y).is_finite() {
                    let _ = write!(s, r#"<circle cx="{:.2}" cy="{:.2}" r="1.5"/>"#, px(*x), py(*y));
                }
            }
            s.push_str("</g>");
        }
        let _ = write!(s, r#"<text x="{}" y="{}" fill="{}">{}</text>"#, ml + pw - 150.0, mt + 20.0 + 16.0 * k as f64, ser.color, xml_escape(&ser.label));
    }
    let _ = write!(s, r#"<text x="{}" y="{}" text-anchor="middle" font-size="15">{}</text>"#, ml + pw / 2.0, mt + ph + 45.0, xml_escape(x_label));
    let _ = write!(s, r#"<text x="20" y="{}" text-anchor="middle" font-size="15" transform="rotate(-90 20 {})">{}</text>"#, mt + ph / 2.0, mt + ph / 2.0, xml_escape(y_label));
    let _ = write!(s, r#"<text x="{}" y="25" text-anchor="middle" font-size="16">{}</text></svg>"#, ml + pw / 2.0, xml_escape(title));
    Ok(s)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_matches_numpy_linear() {
        let v = [1.0, 2.0, 3.0, 4.0, f64::NAN];
        assert_eq!(nanpercentile(&v, 50.0), Some(2.5));
        assert_eq!(nanpercentile(&v, 0.0), Some(1.0));
        assert_eq!(nanpercentile(&v, 100.0), Some(4.0));
    }

    #[test]
    fn png_is_bounded_and_oriented() {
        let mut img = Image::zeros(3000, 10);
        img.data[0] = 100.0;
        let (bytes, info) = render_png(&img, &DisplayOptions::default()).unwrap();
        assert!(info.width <= 1024 && info.block_factor == 3);
        assert_eq!(&bytes[1..4], b"PNG");
        let eq = Image::new(2, 2, vec![5.0; 4]).unwrap();
        assert!(render_png(&eq, &DisplayOptions::default()).is_ok());
    }
}
