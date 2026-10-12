//! PSF generation and import.

use crate::codec::{image_member, read_image};
use crate::types::{ArtifactInput, Quantity};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::Image;
use artpop_core::psf::{Discretization, KernelOptions, Profile, kernel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PsfKind {
    Gaussian,
    Moffat,
}

fn size41() -> usize {
    41
}
fn alpha() -> f64 {
    4.765
}
fn center() -> Discretization {
    Discretization::Center
}
fn factor() -> usize {
    10
}
fn rel_tol() -> f64 {
    1e-10
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    pub kind: PsfKind,
    /// FWHM in an angular unit (with pixel_scale_arcsec) or in `pixel`.
    pub fwhm: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_scale_arcsec: Option<f64>,
    /// Odd kernel width (ArtPop default 41).
    #[serde(default = "size41")]
    pub width: usize,
    #[serde(default = "size41")]
    pub height: usize,
    /// Moffat power index (Trujillo et al. 2001 default 4.765).
    #[serde(default = "alpha")]
    pub alpha: f64,
    #[serde(default = "center")]
    pub mode: Discretization,
    /// Oversampling factor for `oversample`.
    #[serde(default = "factor")]
    pub factor: usize,
    /// Relative tolerance of `integrate` quadrature (Moffat).
    #[serde(default = "rel_tol")]
    pub integration_rel_tol: f64,
}

pub fn build(ctx: &Ctx, req: BuildRequest) -> Result<OpOutput> {
    let fwhm_pix = crate::units::fwhm_pixels(&req.fwhm, req.pixel_scale_arcsec, "/fwhm")?;
    let profile = match req.kind {
        PsfKind::Gaussian => Profile::gaussian_from_fwhm(fwhm_pix),
        PsfKind::Moffat => Profile::moffat_from_fwhm(fwhm_pix, req.alpha),
    };
    if req.width as u64 * req.height as u64 > 25_000_000 {
        return Err(Error::new(ErrorCode::ResourceLimit, "PSF kernel is too large.").at("/width"));
    }
    let opts = KernelOptions { mode: req.mode, factor: req.factor, rel_tol: req.integration_rel_tol, ..Default::default() };
    let (k, info) = kernel(&profile, req.width, req.height, &opts)?;
    let (cx, cy) = (req.width / 2, req.height / 2);
    let meta = json!({
        "profile": profile,
        "kind": req.kind,
        "fwhm_pixels": fwhm_pix,
        "pixel_scale_arcsec": req.pixel_scale_arcsec,
        "mode": req.mode,
        "factor": req.factor,
        "width": req.width,
        "height": req.height,
        "origin": [cx, cy],
        "normalized": true,
        "prenormalization_sum": info.prenormalization_sum,
        "evaluations": info.evaluations,
    });
    let r = ctx.put("psf", meta, vec![image_member("kernel", &k, "fraction")], vec![])?;
    Ok(OpOutput::new(json!({ "psf_artifact_id": r.artifact_id, "fwhm_pixels": fwhm_pix, "profile": profile, "prenormalization_sum": info.prenormalization_sum, "peak": k.get(cx, cy), "sum": k.sum() })).with(r))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitsInput {
    pub path: String,
    #[serde(default)]
    pub hdu: Option<usize>,
    #[serde(default)]
    pub extname: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    #[serde(default)]
    pub fits: Option<FitsInput>,
    #[serde(default)]
    pub image: Option<ArtifactInput>,
    /// Small inline kernel `values_yx[y][x]`.
    #[serde(default)]
    pub values_yx: Option<Vec<Vec<f64>>>,
    /// Divide by the kernel sum (must be well away from zero).
    #[serde(default = "yes")]
    pub normalize: bool,
    /// Allow negative kernel values (generic image convolution only).
    #[serde(default)]
    pub allow_signed: bool,
}

pub fn import(ctx: &Ctx, req: ImportRequest) -> Result<OpOutput> {
    let (img, source): (Image, Value) = match (&req.fits, &req.image, &req.values_yx) {
        (Some(f), None, None) => {
            let p = ctx.read_path(&f.path, "/fits/path")?;
            let r = artpop_io::fits::read_image(&p, f.hdu, f.extname.as_deref()).map_err(|e| e.at("/fits"))?;
            (r.image, json!({ "fits": p, "hdu": r.hdu_index, "image_type": r.image_type }))
        }
        (None, Some(a), None) => {
            let l = ctx.store().load(&a.artifact_id, "/image/artifact_id", &["image", "psf"])?;
            let member = if l.manifest.kind == "psf" { "kernel" } else { "image" };
            (read_image(&l, member)?, json!({ "artifact_id": a.artifact_id }))
        }
        (None, None, Some(rows)) => {
            let h = rows.len();
            let w = rows.first().map_or(0, |r| r.len());
            if h == 0 || w == 0 || rows.iter().any(|r| r.len() != w) {
                return Err(Error::new(ErrorCode::ShapeMismatch, "values_yx must be a nonempty rectangular array.").at("/values_yx"));
            }
            (Image::new(w, h, rows.iter().flatten().copied().collect())?, json!({ "inline": true }))
        }
        _ => return Err(Error::invalid("", "Give exactly one of fits, image, or values_yx.")),
    };
    artpop_core::error::require_finite(&img.data, "/psf", "PSF")?;
    if !req.allow_signed && img.data.iter().any(|v| *v < 0.0) {
        return Err(Error::invalid("/allow_signed", "The PSF has negative values; set allow_signed for generic (non-physical) convolution."));
    }
    let sum = img.sum();
    let k = if req.normalize {
        if sum.abs() < 1.0 / 100.0 {
            return Err(Error::invalid("/normalize", "The kernel sum is too close to zero to normalize.").with_details(json!({ "sum": sum })));
        }
        Image { data: img.data.iter().map(|v| v / sum).collect(), ..img }
    } else {
        img
    };
    if k.width % 2 == 0 || k.height % 2 == 0 {
        ctx.warn("EVEN_KERNEL", "Even-sized kernels are centered at index size/2 (Astropy convention); odd sizes avoid half-pixel shifts.");
    }
    let meta = json!({ "kind": "empirical", "width": k.width, "height": k.height, "origin": [k.width / 2, k.height / 2], "normalized": req.normalize, "prenormalization_sum": sum, "signed": k.data.iter().any(|v| *v < 0.0), "source": source });
    let r = ctx.put("psf", meta, vec![image_member("kernel", &k, "fraction")], req.image.iter().map(|a| a.artifact_id.clone()).collect())?;
    Ok(OpOutput::new(json!({ "psf_artifact_id": r.artifact_id, "width": k.width, "height": k.height, "prenormalization_sum": sum })).with(r))
}
