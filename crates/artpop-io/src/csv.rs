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

/// A parse failure inside one chunk, with the line offset within the chunk;
/// the absolute line number is resolved by the caller (only on error).
enum BodyError {
    TooManyValues { rel_line: usize },
    NotANumber { rel_line: usize, token: String, column: usize },
    WrongCount { rel_line: usize, count: usize },
}

/// Parsed columns of one chunk and the number of lines it contained.
type ChunkResult = std::result::Result<(Vec<Vec<f64>>, usize), BodyError>;

/// Parse the data lines of one chunk into per-column vectors; returns the
/// columns and the number of lines in the chunk.
fn parse_body(body: &str, delim: Delimiter, ncol: usize) -> ChunkResult {
    // Pre-size for about one value per 20 bytes.
    let guess = body.len() / (20 * ncol.max(1)) + 16;
    let mut data: Vec<Vec<f64>> = (0..ncol).map(|_| Vec::with_capacity(guess)).collect();
    let mut lines = 0usize;
    for (rel_line, line) in body.lines().enumerate() {
        lines += 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut count = 0usize;
        let mut push = |tok: &str, data: &mut Vec<Vec<f64>>| -> std::result::Result<(), BodyError> {
            if count >= ncol {
                return Err(BodyError::TooManyValues { rel_line });
            }
            let v = match tok.parse::<f64>() {
                Ok(v) => v,
                Err(_) => tok.trim().parse::<f64>().map_err(|_| BodyError::NotANumber { rel_line, token: tok.trim().to_string(), column: count })?,
            };
            data[count].push(v);
            count += 1;
            Ok(())
        };
        match delim {
            Delimiter::Comma => {
                for tok in t.split(',') {
                    push(tok, &mut data)?;
                }
            }
            Delimiter::Whitespace => {
                for tok in t.split_whitespace() {
                    push(tok, &mut data)?;
                }
            }
            Delimiter::Char(c) => {
                for tok in t.split(c) {
                    push(tok, &mut data)?;
                }
            }
        }
        if count != ncol {
            return Err(BodyError::WrongCount { rel_line, count });
        }
    }
    Ok((data, lines))
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
    // Newline-aligned chunks; line numbers are resolved from the chunks' own
    // line counts, so no separate counting pass is needed.
    let mut chunks: Vec<&str> = Vec::new();
    let mut start = 0usize;
    while start < body.len() {
        let mut end = (start + CHUNK_BYTES).min(body.len());
        if end < body.len() {
            match body[end..].find('\n') {
                Some(k) => end += k + 1,
                None => end = body.len(),
            }
        }
        chunks.push(&body[start..end]);
        start = end;
    }
    let parsed: Vec<ChunkResult> = chunks.par_iter().map(|piece| parse_body(piece, delim, ncol)).collect();
    // Errors are reported for the earliest failing line, as a sequential parse would.
    let mut data: Vec<Vec<f64>> = vec![Vec::new(); ncol];
    let mut first_line = hline + 1;
    for part in parsed {
        match part {
            Ok((cols, lines)) => {
                for (dst, src) in data.iter_mut().zip(cols) {
                    if dst.is_empty() {
                        *dst = src;
                    } else {
                        dst.extend_from_slice(&src);
                    }
                }
                first_line += lines;
            }
            Err(e) => {
                return Err(match e {
                    BodyError::TooManyValues { rel_line } => {
                        let n = first_line + rel_line;
                        Error::new(ErrorCode::DataCorrupt, format!("Line {n} has more than {ncol} values.")).with_details(serde_json::json!({ "line": n }))
                    }
                    BodyError::NotANumber { rel_line, token, column } => {
                        let n = first_line + rel_line;
                        Error::new(ErrorCode::DataCorrupt, format!("Line {n}: '{token}' is not a number (column '{}').", columns[column]))
                            .with_details(serde_json::json!({ "line": n, "column": columns[column] }))
                    }
                    BodyError::WrongCount { rel_line, count } => {
                        let n = first_line + rel_line;
                        Error::new(ErrorCode::DataCorrupt, format!("Line {n} has {count} values; expected {ncol}.")).with_details(serde_json::json!({ "line": n }))
                    }
                });
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

/// Rows per formatted block.
const ROWS_PER_BLOCK: usize = 1 << 12;
/// Blocks formatted per wave; a wave is handed to the sink while the next is formatted.
const BLOCKS_PER_WAVE: usize = 16;

fn format_block(cols: &[Column<'_>], start: usize, end: usize) -> String {
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
}

/// Render typed columns as CSV text (header row, `,` separator, `\n` line
/// ends), formatting row blocks in parallel.
pub fn write_columns(names: &[String], cols: &[Column<'_>]) -> String {
    let mut text = String::new();
    write_columns_to(names, cols, |piece| {
        text.push_str(std::str::from_utf8(piece).expect("csv text"));
        Ok(())
    })
    .expect("infallible sink");
    text
}

/// Render typed columns as CSV and hand the text to `sink` in order, in
/// pieces: the header first, then waves of row blocks. Each wave is sunk
/// (for example written and hashed) while the next wave is being formatted
/// on the other threads. The concatenation of the pieces equals
/// [`write_columns`].
pub fn write_columns_to(names: &[String], cols: &[Column<'_>], mut sink: impl FnMut(&[u8]) -> Result<()> + Send) -> Result<()> {
    let rows = cols.first().map_or(0, |c| c.len());
    let mut header = names.join(",");
    header.push('\n');
    sink(header.as_bytes())?;
    let blocks = rows.div_ceil(ROWS_PER_BLOCK);
    let format_wave = |first_block: usize| -> Vec<String> {
        let last_block = (first_block + BLOCKS_PER_WAVE).min(blocks);
        (first_block..last_block).into_par_iter().map(|b| format_block(cols, b * ROWS_PER_BLOCK, ((b + 1) * ROWS_PER_BLOCK).min(rows))).collect()
    };
    let mut pending = format_wave(0);
    let mut next_first = BLOCKS_PER_WAVE;
    while !pending.is_empty() {
        let (sunk, formatted) = rayon::join(
            || -> Result<()> {
                for b in &pending {
                    sink(b.as_bytes())?;
                }
                Ok(())
            },
            || if next_first < blocks { format_wave(next_first) } else { Vec::new() },
        );
        sunk?;
        pending = formatted;
        next_first += BLOCKS_PER_WAVE;
    }
    Ok(())
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
        let (seq, lines) = parse_body(&text[6..], Delimiter::Comma, 3).ok().unwrap();
        assert_eq!(seq, t.data);
        assert_eq!(lines, n);
        // Two bad lines in different chunks: the earlier one is reported.
        let mut lines: Vec<&str> = text.lines().collect();
        lines[70_000] = "1,2,x";
        lines[20_000] = "1,2";
        let bad = lines.join("\n");
        let e = parse(&bad, None).unwrap_err();
        assert_eq!(e.details["line"], 20_001);
    }

    #[test]
    fn streamed_csv_equals_whole_text() {
        let n = 3 * ROWS_PER_BLOCK * BLOCKS_PER_WAVE + 1234;
        let f: Vec<f64> = (0..n).map(|i| i as f64 * 0.1234567).collect();
        let u: Vec<u64> = (0..n as u64).collect();
        let names = vec!["f".to_string(), "u".to_string()];
        let cols = [Column::f64s(&f[..]), Column::u64s(&u[..])];
        let whole = write_columns(&names, &cols);
        let mut streamed = Vec::new();
        write_columns_to(&names, &cols, |p| {
            streamed.extend_from_slice(p);
            Ok(())
        })
        .unwrap();
        assert_eq!(whole.as_bytes(), &streamed[..]);
        assert_eq!(whole.lines().count(), n + 1);
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
