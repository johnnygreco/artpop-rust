//! FITS image input/output.
//!
//! Science images are written as 64-bit floats in the primary HDU with
//! `NAXIS1 = width` and `NAXIS2 = height`. Writing encodes the file in memory
//! ([`encode_image`]) with CFITSIO's header layout and value formatting, so
//! the bytes are identical to what CFITSIO would write (tested against it)
//! while the digest of the output can be taken from the same buffer. Reading
//! goes through CFITSIO (`fitsio` crate, vendored build) and accepts any
//! integer or floating image HDU; BSCALE/BZERO are applied while converting
//! to f64.

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

const RECORD: usize = 2880;

/// Format a `double` the way CFITSIO's `ffd2e(value, -15)` does: C `%.15G`,
/// then a decimal point is guaranteed (`27.`, `1.0E+20`).
pub fn format_double(v: f64) -> String {
    let mut s = if v == 0.0 {
        if v.is_sign_negative() { "-0".to_string() } else { "0".to_string() }
    } else if !v.is_finite() {
        // CFITSIO prints the C library spelling for these; they never occur in
        // our headers because every key is validated upstream.
        if v.is_nan() { "NAN".to_string() } else if v > 0.0 { "INF".to_string() } else { "-INF".to_string() }
    } else {
        let e = format!("{:.14e}", v);
        let (mant, exp) = e.split_once('e').expect("exponent form");
        let exp: i32 = exp.parse().expect("exponent");
        if !(-4..15).contains(&exp) {
            let mant = strip_trailing_zeros(mant);
            format!("{mant}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
        } else {
            let decimals = (15 - 1 - exp).max(0) as usize;
            strip_trailing_zeros(&format!("{:.*}", decimals, v)).to_string()
        }
    };
    if !s.contains('.') {
        if let Some(i) = s.find('E') {
            s.insert_str(i, ".0");
        } else if !s.contains("NAN") && !s.contains("INF") {
            s.push('.');
        }
    }
    s
}

fn strip_trailing_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

/// One 80-character header card in CFITSIO's layout: keyword padded to 8,
/// `= `, the value (strings left-justified and padded to 20 characters,
/// numbers and logicals right-justified in 20), then ` / comment`.
fn card(key: &str, value: &str, string_value: bool, comment: &str) -> [u8; 80] {
    let mut s = String::with_capacity(96);
    s.push_str(key);
    while s.len() < 8 {
        s.push(' ');
    }
    s.push_str("= ");
    if string_value {
        s.push_str(value);
        while s.len() < 30 {
            s.push(' ');
        }
    } else {
        for _ in value.len()..20 {
            s.push(' ');
        }
        s.push_str(value);
    }
    if !comment.is_empty() && s.len() < 77 {
        s.push_str(" / ");
        s.push_str(comment);
    }
    pad_card(&s)
}

fn pad_card(s: &str) -> [u8; 80] {
    let mut out = [b' '; 80];
    let bytes = s.as_bytes();
    let n = bytes.len().min(80);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Quote a string value as CFITSIO does: single quotes doubled, padded to at
/// least 8 characters inside the quotes.
fn quoted(s: &str) -> String {
    let mut inner = s.replace('\'', "''");
    while inner.len() < 8 {
        inner.push(' ');
    }
    format!("'{inner}'")
}

/// The padded primary header (a multiple of 2880 bytes) of a 64-bit float
/// image of `width` x `height` pixels with the given keys and verbatim extra
/// cards (structural cards must already be filtered by the caller).
pub fn encode_header(width: usize, height: usize, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<Vec<u8>> {
    let mut cards: Vec<[u8; 80]> = Vec::with_capacity(16 + keys.len() + extra_cards.len());
    cards.push(card("SIMPLE", "T", false, "file does conform to FITS standard"));
    cards.push(card("BITPIX", "-64", false, "number of bits per data pixel"));
    cards.push(card("NAXIS", "2", false, "number of data axes"));
    cards.push(card("NAXIS1", &width.to_string(), false, "length of data axis 1"));
    cards.push(card("NAXIS2", &height.to_string(), false, "length of data axis 2"));
    cards.push(card("EXTEND", "T", false, "FITS dataset may contain extensions"));
    cards.push(pad_card("COMMENT   FITS (Flexible Image Transport System) format is defined in 'Astronomy"));
    cards.push(pad_card("COMMENT   and Astrophysics', volume 376, page 359; bibcode: 2001A&A...376..359H"));
    cards.push(card("EXTNAME", "'_PRIMARY'", true, ""));
    for c in extra_cards {
        if c.bytes().any(|b| b == 0) {
            return Err(Error::invalid("/header", "Header card contains a NUL byte."));
        }
        cards.push(pad_card(c));
    }
    for (name, value, comment) in keys {
        let (v, is_str) = match value {
            Key::Str(s) => (quoted(s), true),
            Key::F64(v) => (format_double(*v), false),
            Key::I64(v) => (v.to_string(), false),
            Key::Bool(v) => ((*v as i64).to_string(), false),
        };
        cards.push(card(name, &v, is_str, comment));
    }
    cards.push(pad_card("END"));
    let header_len = cards.len() * 80;
    let mut out = vec![b' '; header_len.div_ceil(RECORD) * RECORD];
    for (i, c) in cards.iter().enumerate() {
        out[i * 80..(i + 1) * 80].copy_from_slice(c);
    }
    Ok(out)
}

/// Incremental writer of a 64-bit float primary image HDU: header first, then
/// the pixel data pushed as little-endian f64 bytes in any chunking, byte-
/// swapped to FITS big-endian order, hashed, and written as it arrives. The
/// bytes are identical to [`encode_image`] of the same image.
pub struct ImageWriter {
    file: std::fs::File,
    hasher: sha2::Sha256,
    swapped: Vec<u8>,
    data_bytes_written: usize,
    data_bytes_expected: usize,
    path: std::path::PathBuf,
}

impl ImageWriter {
    /// Create `path` and write the header.
    pub fn create(path: &Path, width: usize, height: usize, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<ImageWriter> {
        use sha2::Digest;
        use std::io::Write;
        let header = encode_header(width, height, keys, extra_cards)?;
        let mut file = std::fs::File::create(path).map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot create {}: {e}", path.display())))?;
        file.write_all(&header).map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot write {}: {e}", path.display())))?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&header);
        Ok(ImageWriter { file, hasher, swapped: Vec::new(), data_bytes_written: 0, data_bytes_expected: width * height * 8, path: path.to_path_buf() })
    }

    /// Append pixels given as little-endian f64 bytes (length a multiple of 8).
    pub fn push_le_f64_bytes(&mut self, le: &[u8]) -> Result<()> {
        use rayon::prelude::*;
        use sha2::Digest;
        use std::io::Write;
        if !le.len().is_multiple_of(8) || self.data_bytes_written + le.len() > self.data_bytes_expected {
            return Err(Error::internal("FITS pixel stream does not match the image shape."));
        }
        let ImageWriter { file, hasher, swapped, path, .. } = self;
        swapped.clear();
        swapped.resize(le.len(), 0);
        swapped.par_chunks_mut(8 << 15).zip(le.par_chunks(8 << 15)).for_each(|(dst, src)| {
            for (d, s) in dst.as_chunks_mut::<8>().0.iter_mut().zip(src.as_chunks::<8>().0) {
                *d = [s[7], s[6], s[5], s[4], s[3], s[2], s[1], s[0]];
            }
        });
        let (written, ()) = rayon::join(|| file.write_all(swapped), || hasher.update(&*swapped));
        written.map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot write {}: {e}", path.display())))?;
        self.data_bytes_written += le.len();
        Ok(())
    }

    /// Pad the data unit, flush, and return the SHA-256 hex digest of the file.
    pub fn finish(mut self) -> Result<String> {
        use sha2::Digest;
        use std::io::Write;
        if self.data_bytes_written != self.data_bytes_expected {
            return Err(Error::internal("FITS pixel stream ended before the image was complete."));
        }
        let pad = self.data_bytes_expected.div_ceil(RECORD) * RECORD - self.data_bytes_expected;
        let zeros = vec![0u8; pad];
        self.file.write_all(&zeros).map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot write {}: {e}", self.path.display())))?;
        self.hasher.update(&zeros);
        self.file.flush().map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot write {}: {e}", self.path.display())))?;
        let mut hex = String::with_capacity(64);
        for b in self.hasher.finalize() {
            hex.push_str(&format!("{b:02x}"));
        }
        Ok(hex)
    }
}

/// Encode a 64-bit float primary image HDU with the given keys and verbatim
/// extra cards (structural cards must already be filtered by the caller).
pub fn encode_image(image: &Image, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<Vec<u8>> {
    let header = encode_header(image.width, image.height, keys, extra_cards)?;
    let header_padded = header.len();
    let data_len = image.data.len() * 8;
    let data_padded = data_len.div_ceil(RECORD) * RECORD;
    let mut out = vec![0u8; header_padded + data_padded];
    out[..header_padded].copy_from_slice(&header);
    {
        use rayon::prelude::*;
        out[header_padded..header_padded + data_len].par_chunks_mut(8 << 15).zip(image.data.par_chunks(1 << 15)).for_each(|(dst, src)| {
            for (d, v) in dst.as_chunks_mut::<8>().0.iter_mut().zip(src) {
                *d = v.to_bits().to_be_bytes();
            }
        });
    }
    Ok(out)
}

/// Write `image` to a new file (the caller handles overwrite/atomic rename)
/// and return the SHA-256 hex digest of the bytes written.
pub fn write_image(path: &Path, image: &Image, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<String> {
    use sha2::Digest;
    let bytes = encode_image(image, keys, extra_cards)?;
    let (written, digest) = rayon::join(
        || std::fs::write(path, &bytes).map_err(|e| Error::new(ErrorCode::IoError, format!("Cannot write {}: {e}", path.display()))),
        || {
            let d = sha2::Sha256::digest(&bytes);
            let mut s = String::with_capacity(64);
            for b in d {
                s.push_str(&format!("{b:02x}"));
            }
            s
        },
    );
    written?;
    Ok(digest)
}

/// Reference writer through CFITSIO; kept to prove [`encode_image`] produces
/// identical bytes.
#[doc(hidden)]
pub fn write_image_cfitsio(path: &Path, image: &Image, keys: &[(&str, Key, &str)], extra_cards: &[String]) -> Result<()> {
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
        let digest = write_image(&p, &img, &[("BUNIT", Key::Str("counts".into()), "unit"), ("ZPT", Key::F64(27.0), "zero point")], &["OBSERVER= 'test'".to_string()]).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 2 * 2880);
        assert_eq!(digest, {
            use sha2::Digest;
            sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
        });
        let r = read_image(&p, None, None).unwrap();
        assert_eq!(r.image, img);
        assert!(r.cards.iter().any(|c| c.starts_with("BUNIT")));
        assert!(r.cards.iter().any(|c| c.starts_with("OBSERVER")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn streaming_writer_matches_encode_image() {
        let dir = std::env::temp_dir().join(format!("artpop-fits-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = artpop_core::rng::Seed(21).stream("fits", &[]);
        let (w, h) = (613usize, 377usize);
        let img = Image::new(w, h, (0..w * h).map(|_| st.uniform_range(-1e3, 1e3)).collect()).unwrap();
        let keys: Vec<(&str, Key, &str)> = vec![("BUNIT", Key::Str("counts".into()), "pixel unit"), ("ZPT", Key::F64(27.0), "zero point")];
        let extra = vec!["OBSERVER= 'test'".to_string()];
        let want = encode_image(&img, &keys, &extra).unwrap();
        let p = dir.join("stream.fits");
        let mut wr = ImageWriter::create(&p, w, h, &keys, &extra).unwrap();
        // SAFETY (test): little-endian host byte view of the pixel values.
        let le = unsafe { std::slice::from_raw_parts(img.data.as_ptr() as *const u8, img.data.len() * 8) };
        let mut off = 0;
        for size in [8 * 1000, 8 * 70_000, 8 * 3] {
            let end = (off + size).min(le.len());
            wr.push_le_f64_bytes(&le[off..end]).unwrap();
            off = end;
        }
        wr.push_le_f64_bytes(&le[off..]).unwrap();
        let digest = wr.finish().unwrap();
        let got = std::fs::read(&p).unwrap();
        assert!(got == want, "streamed bytes differ");
        use sha2::Digest;
        assert_eq!(digest, sha2::Sha256::digest(&want).iter().map(|b| format!("{b:02x}")).collect::<String>());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn double_formatting_matches_c_percent_15g() {
        let cases = [
            (27.0, "27."),
            (900.0, "900."),
            (4.0, "4."),
            (40627.3948706395, "40627.3948706395"),
            (0.000247309623421106, "0.000247309623421106"),
            (1e20, "1.0E+20"),
            (1.5e-7, "1.5E-07"),
            (0.1, "0.1"),
            (123456789012345.0, "123456789012345."),
            (1234567890123456.0, "1.23456789012346E+15"),
            (-0.0, "-0."),
            (0.0, "0."),
            (1.0 / 3.0, "0.333333333333333"),
            (-2.5, "-2.5"),
            (0.0001, "0.0001"),
            (0.00001, "1.0E-05"),
        ];
        for (v, want) in cases {
            assert_eq!(format_double(v), want, "{v}");
        }
    }

    #[test]
    fn native_encoding_is_byte_identical_to_cfitsio() {
        let dir = std::env::temp_dir().join(format!("artpop-fits-cmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = artpop_core::rng::Seed(8).stream("fits", &[]);
        for (w, h) in [(3usize, 2usize), (361, 7), (101, 203)] {
            let img = Image::new(w, h, (0..w * h).map(|_| st.uniform_range(-1e6, 1e6) * if st.uniform() < 0.1 { 1e-9 } else { 1.0 }).collect()).unwrap();
            let keys: Vec<(&str, Key, &str)> = vec![
                ("BUNIT", Key::Str("flux (zpt=27)".into()), "pixel unit"),
                ("PLANE", Key::Str("image".into()), "artpop-rust plane"),
                ("ARTPOPID", Key::Str("61a855053f3a8cf59ca62dc8fd2de61df28c7f13cf8efebb2658053a2f83a149".into()), "artifact SHA-256"),
                ("ARTPOPV", Key::Str("artpop-rust 0.1.0".into()), "engine"),
                ("ALGPROF", Key::Str("rust-v1".into()), "algorithm profile"),
                ("BANDPASS", Key::Str("LSST_i".into()), "filter"),
                ("ZPT", Key::F64(27.0), "magnitude zero point"),
                ("EXPTIME", Key::F64(900.0), "exposure [s]"),
                ("SKYCNTS", Key::F64(40627.3948706395), "sky counts per pixel"),
                ("CALIB", Key::F64(0.000247309623421106), "flux per count"),
                ("RDNOISE", Key::F64(4.0), "read noise [e-]"),
                ("BIGNUM", Key::F64(1.0e20), "exponent form"),
                ("TINY", Key::F64(1.5e-7), "negative exponent"),
                ("THIRD", Key::F64(1.0 / 3.0), "repeating"),
                ("NEG", Key::F64(-1234.5678), ""),
                ("QUOTE", Key::Str("it's".into()), "doubled quote"),
                ("COUNT", Key::I64(-42), "integer"),
                ("FLAG", Key::Bool(true), "logical as integer"),
                ("PIXCONV", Key::Str("pixel-centers-v1".into()), "pixel coordinate convention"),
                ("LONGCOMM", Key::Str("v".into()), "a comment that is long enough to be truncated at the eightieth column of the card"),
            ];
            let extra = vec!["OBSERVER= 'test'".to_string(), "HISTORY embedded card".to_string(), format!("{:<80}", "CRVAL1  =                  1.5 / wcs")];
            let ours = encode_image(&img, &keys, &extra).unwrap();
            let p = dir.join(format!("ref-{w}x{h}.fits"));
            let _ = std::fs::remove_file(&p);
            write_image_cfitsio(&p, &img, &keys, &extra).unwrap();
            let theirs = std::fs::read(&p).unwrap();
            assert_eq!(ours.len(), theirs.len());
            for (i, (a, b)) in ours.chunks(80).zip(theirs.chunks(80)).enumerate().take(ours.len().div_ceil(2880).min(2) * 36) {
                assert_eq!(String::from_utf8_lossy(a), String::from_utf8_lossy(b), "card {i} of {w}x{h}");
            }
            assert!(ours == theirs, "{w}x{h} data differs");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
