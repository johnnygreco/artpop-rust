//! MIST v1.2 `.iso.cmd` parsing, grid selection, and [Fe/H] interpolation.
//!
//! The parser streams over text and fully parses only the selected age block.
//! Age selection uses exact integer indices of ArtPop's nominal grid
//! (5.0 to 10.3 in steps of 0.05, 107 ages), so the literal endpoint 10.3 is
//! accepted. Off-grid [Fe/H] uses ArtPop's row-wise linear interpolation with
//! tail extension; both endpoints use the same rotation and version, and EEP
//! correspondence is checked and reported.

use crate::error::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};

pub const MIST_VERSION: &str = "1.2";
pub const AGE_GRID_START: f64 = 5.0;
pub const AGE_GRID_STEP: f64 = 0.05;
pub const AGE_GRID_LEN: usize = 107;
pub const ROTATIONS: [f64; 2] = [0.0, 0.4];

/// [Fe/H] grid used by ArtPop (MIST has lower values; ArtPop stops at -3).
pub fn feh_grid() -> Vec<f64> {
    let mut g = vec![-3.0, -2.5];
    for i in 0..11 {
        g.push(-2.0 + 0.25 * i as f64);
    }
    g
}

pub fn nominal_age(index: usize) -> f64 {
    AGE_GRID_START + AGE_GRID_STEP * index as f64
}

/// Nearest nominal grid index, ties toward the lower age.
pub fn age_index(log_age: f64) -> Result<usize> {
    let lo = AGE_GRID_START;
    let hi = nominal_age(AGE_GRID_LEN - 1);
    if !log_age.is_finite() || log_age < lo - 1e-9 || log_age > hi + 1e-9 {
        return Err(Error::domain("/log_age_yr", format!("log10(age/yr) = {log_age} is outside the MIST grid [{lo}, {hi}]."))
            .with_details(serde_json::json!({ "minimum": lo, "maximum": hi, "step": AGE_GRID_STEP })));
    }
    let t = (log_age - lo) / AGE_GRID_STEP;
    let i = t.floor().clamp(0.0, (AGE_GRID_LEN - 1) as f64) as usize;
    let j = (i + 1).min(AGE_GRID_LEN - 1);
    let (di, dj) = ((log_age - nominal_age(i)).abs(), (log_age - nominal_age(j)).abs());
    Ok(if dj < di - 1e-12 { j } else { i })
}

pub fn file_name(feh: f64, rotation: f64, system: &str) -> String {
    let sign = if feh < 0.0 { 'm' } else { 'p' };
    format!("MIST_v{MIST_VERSION}_feh_{sign}{:.2}_afe_p0.0_vvcrit{rotation:.1}_{system}.iso.cmd", feh.abs())
}

pub fn grid_directory(rotation: f64, system: &str) -> String {
    format!("MIST_v{MIST_VERSION}_vvcrit{rotation:.1}_{system}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Header {
    pub mist_version: String,
    pub mesa_revision: String,
    pub photometric_system: String,
    pub y_init: f64,
    pub z_init: f64,
    pub feh: f64,
    pub afe: f64,
    pub v_vcrit: f64,
    pub num_ages: usize,
    pub av: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub log_age: f64,
    pub columns: Vec<String>,
    /// `[column][row]`
    pub data: Vec<Vec<f64>>,
}

impl Block {
    pub fn column(&self, name: &str) -> Option<&[f64]> {
        self.columns.iter().position(|c| c == name).map(|i| self.data[i].as_slice())
    }

    pub fn rows(&self) -> usize {
        self.data.first().map_or(0, |c| c.len())
    }
}

fn corrupt(line: usize, msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::DataCorrupt, msg).with_details(serde_json::json!({ "line": line + 1 }))
}

fn last_value(line: &str, n: usize) -> Result<f64> {
    line.split_whitespace().last().and_then(|v| v.parse().ok()).ok_or_else(|| corrupt(n, format!("Expected a numeric value at the end of: {line}")))
}

pub fn parse_header(text: &str) -> Result<Header> {
    let lines: Vec<&str> = text.lines().take(10).collect();
    if lines.len() < 10 || !lines[0].starts_with("# MIST version number") {
        return Err(corrupt(0, "Not a MIST .iso.cmd file (missing '# MIST version number' header)."));
    }
    let after_eq = |l: &str| l.split_once('=').map(|(_, v)| v.trim().to_string()).unwrap_or_default();
    let abun: Vec<f64> = lines[5].trim_start_matches('#').split_whitespace().map(|v| v.parse().map_err(|_| corrupt(5, "Bad abundance line"))).collect::<Result<_>>()?;
    if abun.len() != 5 {
        return Err(corrupt(5, "Abundance line must contain Yinit, Zinit, [Fe/H], [a/Fe], v/vcrit."));
    }
    Ok(Header {
        mist_version: after_eq(lines[0]),
        mesa_revision: after_eq(lines[1]),
        photometric_system: after_eq(lines[2]),
        y_init: abun[0],
        z_init: abun[1],
        feh: abun[2],
        afe: abun[3],
        v_vcrit: abun[4],
        num_ages: last_value(lines[7], 7)? as usize,
        av: last_value(lines[8], 8)?,
    })
}

/// Parse the header, list every block age, and fully parse the block nearest
/// to `log_age` (first minimum on ties, as ArtPop's `age_index`).
pub fn read_nearest_block(text: &str, log_age: f64) -> Result<(Header, Vec<f64>, Block)> {
    let header = parse_header(text)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 10;
    let mut blocks: Vec<(f64, usize, usize, Vec<String>)> = Vec::new(); // (age, first data line, rows, columns)
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with("# number of EEPs") {
            let nums: Vec<usize> = l.split('=').nth(1).unwrap_or("").split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if nums.len() != 2 {
                return Err(corrupt(i, "Malformed '# number of EEPs, cols' line."));
            }
            let (rows, cols) = (nums[0], nums[1]);
            let hdr = lines.get(i + 2).ok_or_else(|| corrupt(i, "Truncated block header."))?;
            let columns: Vec<String> = hdr.trim_start_matches('#').split_whitespace().map(String::from).collect();
            if columns.len() != cols {
                return Err(corrupt(i + 2, format!("Header lists {} columns but the block declares {cols}.", columns.len())));
            }
            let first = i + 3;
            if first + rows > lines.len() || rows == 0 {
                return Err(corrupt(i, "Truncated age block."));
            }
            let age: f64 = lines[first].split_whitespace().nth(1).and_then(|v| v.parse().ok()).ok_or_else(|| corrupt(first, "Missing age column."))?;
            blocks.push((age, first, rows, columns));
            i = first + rows;
        } else {
            i += 1;
        }
    }
    if blocks.len() != header.num_ages {
        return Err(Error::new(ErrorCode::DataCorrupt, format!("Header declares {} ages but {} blocks were found.", header.num_ages, blocks.len())));
    }
    let ages: Vec<f64> = blocks.iter().map(|b| b.0).collect();
    let mut best = 0;
    for (k, a) in ages.iter().enumerate() {
        if (a - log_age).abs() < (ages[best] - log_age).abs() {
            best = k;
        }
    }
    let (age, first, rows, columns) = blocks.swap_remove(best);
    let mut data = vec![Vec::with_capacity(rows); columns.len()];
    for r in 0..rows {
        let line = lines[first + r];
        let mut n = 0;
        for (c, tok) in line.split_whitespace().enumerate() {
            if c >= columns.len() {
                return Err(corrupt(first + r, "Row has more values than columns."));
            }
            data[c].push(tok.parse::<f64>().map_err(|_| corrupt(first + r, format!("Invalid number '{tok}'.")))?);
            n += 1;
        }
        if n != columns.len() {
            return Err(corrupt(first + r, format!("Row has {n} values; expected {}.", columns.len())));
        }
    }
    Ok((header, ages, Block { log_age: age, columns, data }))
}

#[derive(Debug, Clone, Serialize)]
pub struct FehInterpolation {
    pub feh_low: f64,
    pub feh_high: f64,
    pub weight: f64,
    pub rows_low: usize,
    pub rows_high: usize,
    pub extrapolated_tail_rows: usize,
    pub extended_endpoint: Option<&'static str>,
    pub eep_mismatched_rows: usize,
}

/// ArtPop `_interp_on_feh`: extend the shorter table with the longer table's
/// tail increments, then interpolate every column row by row. EEP is
/// truncated to an integer as in the structured-array cast upstream.
pub fn interpolate_feh(lo: &Block, hi: &Block, feh_low: f64, feh_high: f64, feh: f64) -> Result<(Block, FehInterpolation)> {
    if lo.columns != hi.columns {
        return Err(Error::new(ErrorCode::ModelAlignment, "MIST tables at neighboring [Fe/H] have different columns."));
    }
    let w = (feh - feh_low) / (feh_high - feh_low);
    let (n0, n1) = (lo.rows(), hi.rows());
    let extend = |short: &Block, long: &Block| -> Vec<Vec<f64>> {
        let ns = short.rows();
        short
            .data
            .iter()
            .zip(&long.data)
            .map(|(s, l)| {
                let mut v = s.clone();
                for r in ns..l.len() {
                    v.push(s[ns - 1] + (l[r] - l[ns - 1]));
                }
                v
            })
            .collect()
    };
    let (y0, y1, ext) = if n0 < n1 {
        (extend(lo, hi), hi.data.clone(), Some("low"))
    } else if n0 > n1 {
        (lo.data.clone(), extend(hi, lo), Some("high"))
    } else {
        (lo.data.clone(), hi.data.clone(), None)
    };
    let eep_col = lo.columns.iter().position(|c| c == "EEP");
    let overlap = n0.min(n1);
    let mismatched = eep_col.map_or(0, |c| (0..overlap).filter(|r| lo.data[c][*r] != hi.data[c][*r]).count());
    let mut data = Vec::with_capacity(y0.len());
    for (c, (a, b)) in y0.iter().zip(&y1).enumerate() {
        let mut col: Vec<f64> = a.iter().zip(b).map(|(p, q)| p * (1.0 - w) + q * w).collect();
        if Some(c) == eep_col {
            for v in &mut col {
                *v = v.trunc();
            }
        }
        data.push(col);
    }
    let info = FehInterpolation {
        feh_low,
        feh_high,
        weight: w,
        rows_low: n0,
        rows_high: n1,
        extrapolated_tail_rows: n0.abs_diff(n1),
        extended_endpoint: ext,
        eep_mismatched_rows: mismatched,
    };
    Ok((Block { log_age: lo.log_age, columns: lo.columns.clone(), data }, info))
}

/// PARSEC CMD table: columns named by the header line whose second token
/// contains `Zini`; numeric rows are all non-comment lines.
pub fn parse_parsec(text: &str) -> Result<(Vec<String>, Vec<Vec<f64>>)> {
    let mut names: Option<Vec<String>> = None;
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() > 1 && toks[0].starts_with('#') && toks[1].contains("Zini") {
            names = Some(toks[1..].iter().map(|s| s.to_string()).collect());
            break;
        }
    }
    let names = names.ok_or_else(|| Error::new(ErrorCode::DataCorrupt, "No PARSEC header line containing 'Zini' was found."))?;
    let mut cols = vec![Vec::new(); names.len()];
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let vals: Vec<&str> = t.split_whitespace().collect();
        if vals.len() != names.len() {
            return Err(corrupt(n, format!("PARSEC row has {} values; header has {} columns.", vals.len(), names.len())));
        }
        for (c, v) in vals.iter().enumerate() {
            cols[c].push(v.parse::<f64>().map_err(|_| corrupt(n, format!("Invalid number '{v}'.")))?);
        }
    }
    if cols[0].is_empty() {
        return Err(Error::new(ErrorCode::DataCorrupt, "PARSEC file contains no data rows."));
    }
    Ok((names, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_grid_indices() {
        assert_eq!(age_index(5.0).unwrap(), 0);
        assert_eq!(age_index(10.3).unwrap(), 106);
        assert_eq!(age_index(10.0).unwrap(), 100);
        assert_eq!(age_index(8.524).unwrap(), 70);
        assert_eq!(age_index(8.525).unwrap(), 70, "tie goes to the lower age");
        assert!(age_index(10.31).is_err());
        assert_eq!(file_name(-1.0, 0.4, "LSST"), "MIST_v1.2_feh_m1.00_afe_p0.0_vvcrit0.4_LSST.iso.cmd");
        assert_eq!(file_name(0.0, 0.0, "HSC"), "MIST_v1.2_feh_p0.00_afe_p0.0_vvcrit0.0_HSC.iso.cmd");
        assert_eq!(feh_grid().len(), 13);
    }

    #[test]
    fn tail_extension_matches_upstream_rule() {
        let lo = Block { log_age: 9.0, columns: vec!["EEP".into(), "m".into()], data: vec![vec![1.0, 2.0], vec![10.0, 20.0]] };
        let hi = Block { log_age: 9.0, columns: vec!["EEP".into(), "m".into()], data: vec![vec![1.0, 2.0, 3.0], vec![11.0, 21.0, 26.0]] };
        let (b, info) = interpolate_feh(&lo, &hi, -1.0, -0.75, -0.875).unwrap();
        // lo extended: 20 + (26 - 21) = 25 -> mean with 26 = 25.5
        assert_eq!(b.data[1], vec![10.5, 20.5, 25.5]);
        assert_eq!(b.data[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(info.extrapolated_tail_rows, 1);
    }
}
