//! FITS image input/output through CFITSIO (`fitsio` crate, vendored build).
//!
//! Science images are written as 64-bit floats in the primary HDU with
//! `NAXIS1 = width` and `NAXIS2 = height`. Reading accepts any integer or
//! floating image HDU; CFITSIO applies BSCALE/BZERO while converting to f64.

use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::image::Image;
use fitsio::FitsFile;
use fitsio::hdu::HduInfo;
use fitsio::images::{ImageDescription, ImageType};
use std::ffi::{CStr, CString};
use std::path::Path;

pub enum Key {
    Str(String),
    F64(f64),
    I64(i64),
    Bool(bool),
}

fn fits_err(e: fitsio::errors::Error, path: &Path) -> Error {
    Error::new(ErrorCode::IoError, format!("FITS error for {}: {e}", path.display()))
}

pub fn cfitsio_version() -> String {
    let v = fitsio::cfitsio_version();
    format!("{v}")
}

const STRUCTURAL: [&str; 14] = ["SIMPLE", "BITPIX", "NAXIS", "EXTEND", "XTENSION", "PCOUNT", "GCOUNT", "BSCALE", "BZERO", "BLANK", "END", "CHECKSUM", "DATASUM", "EXTNAME"];

fn is_structural(card: &str) -> bool {
    let key = card.get(..8).unwrap_or(card).trim();
    STRUCTURAL.contains(&key) || (key.starts_with("NAXIS") && key[5..].chars().all(|c| c.is_ascii_digit()))
}

/// Write `image` to a new file (the caller handles overwrite/atomic rename).
pub fn write_image(path: &Path, image: &Image, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<()> {
    let desc = ImageDescription { data_type: ImageType::Double, dimensions: &[image.height, image.width] };
    let mut f = FitsFile::create(path).with_custom_primary(&desc).open().map_err(|e| fits_err(e, path))?;
    let hdu = f.primary_hdu().map_err(|e| fits_err(e, path))?;
    hdu.write_image(&mut f, &image.data).map_err(|e| fits_err(e, path))?;
    for card in extra_cards {
        if is_structural(card) {
            continue;
        }
        let c = CString::new(card.as_str()).map_err(|_| Error::invalid("/header", "Header card contains a NUL byte."))?;
        let mut status = 0;
        unsafe {
            fitsio::sys::ffprec(f.as_raw(), c.as_ptr(), &mut status);
        }
        if status != 0 {
            return Err(Error::new(ErrorCode::IoError, format!("CFITSIO could not copy header card (status {status}): {card}")));
        }
    }
    for (name, value, comment) in keys {
        let r = match value {
            Key::Str(s) => hdu.write_key(&mut f, name, (s.as_str(), *comment)),
            Key::F64(v) => hdu.write_key(&mut f, name, (*v, *comment)),
            Key::I64(v) => hdu.write_key(&mut f, name, (*v, *comment)),
            Key::Bool(v) => hdu.write_key(&mut f, name, (*v as i64, *comment)),
        };
        r.map_err(|e| fits_err(e, path))?;
    }
    Ok(())
}

pub struct FitsImage {
    pub image: Image,
    pub hdu_index: usize,
    pub image_type: String,
    /// Non-structural header cards (80 characters each).
    pub cards: Vec<String>,
}

/// Read an image HDU: by index, by EXTNAME, or the first HDU with data.
pub fn read_image(path: &Path, hdu_index: Option<usize>, extname: Option<&str>) -> Result<FitsImage> {
    if !path.is_file() {
        return Err(Error::new(ErrorCode::ArtifactNotFound, format!("FITS file {} does not exist.", path.display())));
    }
    let mut f = FitsFile::open(path).map_err(|e| fits_err(e, path))?;
    let n = f.num_hdus().map_err(|e| fits_err(e, path))?;
    let mut chosen = None;
    for i in 0..n {
        if hdu_index.is_some_and(|h| h != i) {
            continue;
        }
        let hdu = f.hdu(i).map_err(|e| fits_err(e, path))?;
        if let Some(name) = extname {
            let en: Option<String> = hdu.read_key(&mut f, "EXTNAME").ok();
            if en.as_deref().map(str::trim) != Some(name) {
                continue;
            }
        }
        if let HduInfo::ImageInfo { shape, image_type } = &hdu.info {
            if shape.len() == 2 && shape[0] > 0 && shape[1] > 0 {
                chosen = Some((i, shape.clone(), format!("{image_type:?}")));
                break;
            } else if hdu_index.is_some() || extname.is_some() {
                return Err(Error::new(ErrorCode::UnsupportedFormat, format!("HDU {i} is not a two-dimensional image (shape {shape:?}).")));
            }
        } else if hdu_index.is_some() || extname.is_some() {
            return Err(Error::new(ErrorCode::UnsupportedFormat, format!("HDU {i} is not an image HDU.")));
        }
    }
    let (idx, shape, image_type) = chosen.ok_or_else(|| Error::new(ErrorCode::UnsupportedFormat, "No two-dimensional image HDU matched the request."))?;
    let hdu = f.hdu(idx).map_err(|e| fits_err(e, path))?;
    let data: Vec<f64> = hdu.read_image(&mut f).map_err(|e| fits_err(e, path))?;
    let image = Image::new(shape[1], shape[0], data)?;
    // Collect header cards through the raw CFITSIO interface.
    let mut cards = Vec::new();
    unsafe {
        let raw = f.as_raw();
        let mut status = 0;
        let (mut nexist, mut nmore) = (0, 0);
        fitsio::sys::ffghsp(raw, &mut nexist, &mut nmore, &mut status);
        for k in 1..=nexist {
            let mut buf = [0 as std::os::raw::c_char; 81];
            fitsio::sys::ffgrec(raw, k, buf.as_mut_ptr(), &mut status);
            if status != 0 {
                break;
            }
            let card = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
            if !is_structural(&card) {
                cards.push(card);
            }
        }
    }
    Ok(FitsImage { image, hdu_index: idx, image_type, cards })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_non_square_with_headers() {
        let dir = std::env::temp_dir().join(format!("artpop-fits-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.fits");
        let _ = std::fs::remove_file(&p);
        let img = Image::new(3, 2, vec![0.0, 1.0, 2.0, 3.0, 4.5, -5.0]).unwrap();
        write_image(&p, &img, &[("BUNIT", Key::Str("counts".into()), "unit"), ("ZPT", Key::F64(27.0), "zero point")], &["OBSERVER= 'test'".to_string()]).unwrap();
        let r = read_image(&p, None, None).unwrap();
        assert_eq!(r.image, img);
        assert!(r.cards.iter().any(|c| c.starts_with("BUNIT")));
        assert!(r.cards.iter().any(|c| c.starts_with("OBSERVER")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
