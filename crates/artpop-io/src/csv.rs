//! Numeric CSV tables (header row, comma or whitespace delimited).
//!
//! Parsing splits the body into newline-aligned chunks that are parsed on
//! separate threads and concatenated in order; the result and every error
//! (first failing line, with its column) are identical to a sequential parse.

use artpop_core::error::{Error, ErrorCode, Result};
use rayon::prelude::*;
use std::fmt::Write as _;

/// Shortest round-trip float formatter buffer.
pub type RyuBuffer = ryu::Buffer;

#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<String>,
    /// `[column][row]`
    pub data: Vec<Vec<f64>>,
}

impl Table {
    pub fn rows(&self) -> usize {
        self.data.first().map_or(0, |c| c.len())
    }

    pub fn column(&self, name: &str) -> Option<&[f64]> {
        self.columns.iter().position(|c| c == name).map(|i| self.data[i].as_slice())
    }

    pub fn require(&self, name: &str, path: &str) -> Result<&[f64]> {
        self.column(name).ok_or_else(|| {
            Error::invalid(path, format!("Column '{name}' is not present in the table."))
                .with_details(serde_json::json!({ "available": self.columns }))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Whitespace,
    Char(char),
}

/// Bytes per parallel parsing chunk (chunks end at a newline).
const CHUNK_BYTES: usize = 1 << 18;

/// Parse one token as `f64`; whitespace around a token is tolerated.
#[inline]
fn parse_token(tok: &str) -> Option<f64> {
    tok.parse::<f64>().ok().or_else(|| tok.trim().parse::<f64>().ok())
}

/// Parse the data lines of `body` (whose first line is line number `first_line`,
/// 1-based) into per-column vectors.
fn parse_body(body: &str, first_line: usize, delim: Delimiter, columns: &[String]) -> Result<Vec<Vec<f64>>> {
    let ncol = columns.len();
    let mut data = vec![Vec::new(); ncol];
    for (k, line) in body.lines().enumerate() {
        let n = first_line + k;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut count = 0;
        let mut push = |tok: &str| -> Result<()> {
            if count >= ncol {
                return Err(Error::new(ErrorCode::DataCorrupt, format!("Line {n} has more than {ncol} values.")).with_details(serde_json::json!({ "line": n })));
            }
            let v = parse_token(tok).ok_or_else(|| {
                Error::new(ErrorCode::DataCorrupt, format!("Line {n}: '{}' is not a number (column '{}').", tok.trim(), columns[count]))
                    .with_details(serde_json::json!({ "line": n, "column": columns[count] }))
            })?;
            data[count].push(v);
            count += 1;
            Ok(())
        };
        match delim {
            Delimiter::Comma => t.split(',').try_for_each(&mut push)?,
            Delimiter::Whitespace => t.split_whitespace().try_for_each(&mut push)?,
            Delimiter::Char(c) => t.split(c).try_for_each(&mut push)?,
        }
        if count != ncol {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Line {n} has {count} values; expected {ncol}.")).with_details(serde_json::json!({ "line": n })));
        }
    }
    Ok(data)
}

/// Parse a numeric table. The first non-comment line is the header (a leading
/// `#` on the header itself is allowed). Empty lines are ignored.
pub fn parse(text: &str, delimiter: Option<Delimiter>) -> Result<Table> {
    // Locate the header: the first non-empty line.
    let mut offset = 0usize;
    let mut line_no = 0usize;
    let mut header: Option<(usize, &str)> = None;
    for line in text.split_inclusive('\n') {
        line_no += 1;
        offset += line.len();
        if !line.trim().is_empty() {
            header = Some((line_no, line));
            break;
        }
    }
    let (hline, header) = header.ok_or_else(|| Error::new(ErrorCode::DataCorrupt, "Table is empty."))?;
    let header_text = header.trim().trim_start_matches('#').trim();
    let delim = delimiter.unwrap_or(if header_text.contains(',') { Delimiter::Comma } else { Delimiter::Whitespace });
    let split = |s: &str| -> Vec<String> {
        match delim {
            Delimiter::Comma => s.split(',').map(|t| t.trim().to_string()).collect(),
            Delimiter::Whitespace => s.split_whitespace().map(String::from).collect(),
            Delimiter::Char(c) => s.split(c).map(|t| t.trim().to_string()).collect(),
        }
    };
    let columns = split(header_text);
    if columns.is_empty() || columns.iter().any(|c| c.is_empty()) {
        return Err(Error::new(ErrorCode::DataCorrupt, "Table header has empty column names.").with_details(serde_json::json!({ "line": hline })));
    }
    for (i, c) in columns.iter().enumerate() {
        if columns[..i].contains(c) {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Duplicate column name '{c}'.")).with_details(serde_json::json!({ "line": hline })));
        }
    }
    let body = &text[offset..];
    let ncol = columns.len();
    // Chunk boundaries at newlines, with the 1-based line number of each chunk's first line.
    let mut chunks: Vec<(usize, &str)> = Vec::new();
    let mut start = 0usize;
    let mut first_line = hline + 1;
    while start < body.len() {
        let mut end = (start + CHUNK_BYTES).min(body.len());
        if end < body.len() {
            match body[end..].find('\n') {
                Some(k) => end += k + 1,
                None => end = body.len(),
            }
        }
        let piece = &body[start..end];
        chunks.push((first_line, piece));
        first_line += piece.bytes().filter(|b| *b == b'\n').count();
        start = end;
    }
    let parsed: Vec<Result<Vec<Vec<f64>>>> = chunks.par_iter().map(|(first, piece)| parse_body(piece, *first, delim, &columns)).collect();
    // Errors are reported for the earliest failing line, as a sequential parse would.
    let mut data: Vec<Vec<f64>> = vec![Vec::new(); ncol];
    for part in parsed {
        let part = part?;
        for (dst, src) in data.iter_mut().zip(part) {
            if dst.is_empty() {
                *dst = src;
            } else {
                dst.extend_from_slice(&src);
            }
        }
    }
    Ok(Table { columns, data })
}

/// Shortest round-trip decimal representation (finite values only).
pub fn fmt_f64(v: f64, buf: &mut ryu::Buffer) -> &str {
    if v.is_finite() { buf.format_finite(v) } else if v.is_nan() { "nan" } else if v > 0.0 { "inf" } else { "-inf" }
}

pub fn write(columns: &[&str], data: &[&[f64]]) -> String {
    let rows = data.first().map_or(0, |c| c.len());
    let mut out = String::with_capacity(rows * columns.len() * 20 + 64);
    out.push_str(&columns.join(","));
    out.push('\n');
    let mut buf = ryu::Buffer::new();
    for r in 0..rows {
        for (c, col) in data.iter().enumerate() {
            if c > 0 {
                out.push(',');
            }
            let _ = write!(out, "{}", fmt_f64(col[r], &mut buf));
        }
        out.push('\n');
    }
    out
}

/// A typed export column; rows are rendered without intermediate JSON values.
pub enum Column<'a> {
    /// Finite values print in shortest round-trip form; non-finite values print `nan`.
    F64(std::borrow::Cow<'a, [f64]>),
    U64(std::borrow::Cow<'a, [u64]>),
    U32(std::borrow::Cow<'a, [u32]>),
    /// Printed as `1`/`0`.
    Bool(std::borrow::Cow<'a, [bool]>),
    Str(Vec<String>),
}

impl<'a> Column<'a> {
    pub fn f64s(v: impl Into<std::borrow::Cow<'a, [f64]>>) -> Column<'a> {
        Column::F64(v.into())
    }
    pub fn u64s(v: impl Into<std::borrow::Cow<'a, [u64]>>) -> Column<'a> {
        Column::U64(v.into())
    }
    pub fn u32s(v: impl Into<std::borrow::Cow<'a, [u32]>>) -> Column<'a> {
        Column::U32(v.into())
    }
    pub fn bools(v: impl Into<std::borrow::Cow<'a, [bool]>>) -> Column<'a> {
        Column::Bool(v.into())
    }
}

impl Column<'_> {
    pub fn len(&self) -> usize {
        match self {
            Column::F64(v) => v.len(),
            Column::U64(v) => v.len(),
            Column::U32(v) => v.len(),
            Column::Bool(v) => v.len(),
            Column::Str(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn write_cell<'b>(&'b self, r: usize, out: &mut String, buf: &'b mut ryu::Buffer, ibuf: &mut itoa::Buffer) {
        match self {
            Column::F64(v) => out.push_str(if v[r].is_finite() { buf.format_finite(v[r]) } else { "nan" }),
            Column::U64(v) => out.push_str(ibuf.format(v[r])),
            Column::U32(v) => out.push_str(ibuf.format(v[r])),
            Column::Bool(v) => out.push_str(if v[r] { "1" } else { "0" }),
            Column::Str(v) => out.push_str(&v[r]),
        }
    }
}

/// Render typed columns as CSV text (header row, `,` separator, `\n` line
/// ends), formatting row blocks in parallel.
pub fn write_columns(names: &[String], cols: &[Column<'_>]) -> String {
    let rows = cols.first().map_or(0, |c| c.len());
    const ROWS_PER_BLOCK: usize = 1 << 14;
    let blocks: Vec<String> = (0..rows.div_ceil(ROWS_PER_BLOCK))
        .into_par_iter()
        .map(|b| {
            let (start, end) = (b * ROWS_PER_BLOCK, ((b + 1) * ROWS_PER_BLOCK).min(rows));
            let mut out = String::with_capacity((end - start) * cols.len() * 20);
            let mut buf = ryu::Buffer::new();
            let mut ibuf = itoa::Buffer::new();
            for r in start..end {
                for (c, col) in cols.iter().enumerate() {
                    if c > 0 {
                        out.push(',');
                    }
                    col.write_cell(r, &mut out, &mut buf, &mut ibuf);
                }
                out.push('\n');
            }
            out
        })
        .collect();
    let mut text = String::with_capacity(blocks.iter().map(String::len).sum::<usize>() + names.iter().map(|n| n.len() + 1).sum::<usize>() + 1);
    text.push_str(&names.join(","));
    text.push('\n');
    for b in blocks {
        text.push_str(&b);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_exact() {
        let xs = [0.1, 1.0 / 3.0, 1e-300, 123456789.125, -2.5e21];
        let text = write(&["a"], &[&xs]);
        let t = parse(&text, None).unwrap();
        assert_eq!(t.column("a").unwrap(), &xs);
    }

    #[test]
    fn errors_report_lines() {
        let e = parse("x,y\n1,2\n3\n", None).unwrap_err();
        assert_eq!(e.details["line"], 3);
        let e = parse("x,y\n1,abc\n", None).unwrap_err();
        assert_eq!(e.details["column"], "y");
        let t = parse("# wave trans\n1 2\n3 4\n", None).unwrap();
        assert_eq!(t.columns, vec!["wave", "trans"]);
        // Blank lines before the header, comments and padded tokens inside the body.
        let t = parse("\n\n x , y \n 1 , 2\n# note\n\n3,4\n", None).unwrap();
        assert_eq!(t.columns, vec!["x", "y"]);
        assert_eq!(t.data, vec![vec![1.0, 3.0], vec![2.0, 4.0]]);
        let e = parse("x,y\n1,2\n\n\n5,6,7\n", None).unwrap_err();
        assert_eq!(e.details["line"], 5);
    }

    #[test]
    fn chunked_parse_matches_sequential_and_reports_first_error() {
        let n = 90_000;
        let mut text = String::from("a,b,c\n");
        let mut st = artpop_core::rng::Seed(1).stream("csv", &[]);
        let mut cols = vec![Vec::new(); 3];
        let mut buf = ryu::Buffer::new();
        for _ in 0..n {
            for (c, col) in cols.iter_mut().enumerate() {
                let v = st.uniform_range(-1e5, 1e5) * 10f64.powi(c as i32 * 7 - 7);
                col.push(v);
                text.push_str(buf.format_finite(v));
                text.push(if c < 2 { ',' } else { '\n' });
            }
        }
        assert!(text.len() > 4 * CHUNK_BYTES, "test must span several chunks");
        let t = parse(&text, None).unwrap();
        assert_eq!(t.rows(), n);
        assert_eq!(t.data, cols);
        let seq = parse_body(&text[6..], 2, Delimiter::Comma, &t.columns).unwrap();
        assert_eq!(seq, t.data);
        // Two bad lines in different chunks: the earlier one is reported.
        let mut lines: Vec<&str> = text.lines().collect();
        lines[70_000] = "1,2,x";
        lines[20_000] = "1,2";
        let bad = lines.join("\n");
        let e = parse(&bad, None).unwrap_err();
        assert_eq!(e.details["line"], 20_001);
    }

    #[test]
    fn typed_columns_render_like_json_values() {
        let f = [1.5, f64::NAN, -2.0e-7, f64::INFINITY];
        let u = [0u64, 10, 200, 3_000_000_000_000];
        let b = [true, false, true, false];
        let s = Column::Str(vec!["MS".into(), "RGB".into(), "".into(), "x,y".into()]);
        let text = write_columns(&["f".into(), "u".into(), "b".into(), "s".into()], &[Column::f64s(&f[..]), Column::u64s(&u[..]), Column::bools(&b[..]), s]);
        assert_eq!(text, "f,u,b,s\n1.5,0,1,MS\nnan,10,0,RGB\n-2e-7,200,1,\nnan,3000000000000,0,x,y\n");
    }
}
