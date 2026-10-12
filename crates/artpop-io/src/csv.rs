//! Numeric CSV tables (header row, comma or whitespace delimited).

use artpop_core::error::{Error, ErrorCode, Result};
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

/// Parse a numeric table. The first non-comment line is the header (a leading
/// `#` on the header itself is allowed). Empty lines are ignored.
pub fn parse(text: &str, delimiter: Option<Delimiter>) -> Result<Table> {
    let mut lines = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty());
    let (hline, header) = lines.next().ok_or_else(|| Error::new(ErrorCode::DataCorrupt, "Table is empty."))?;
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
        return Err(Error::new(ErrorCode::DataCorrupt, "Table header has empty column names.").with_details(serde_json::json!({ "line": hline + 1 })));
    }
    for (i, c) in columns.iter().enumerate() {
        if columns[..i].contains(c) {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Duplicate column name '{c}'.")).with_details(serde_json::json!({ "line": hline + 1 })));
        }
    }
    let ncol = columns.len();
    let mut data = vec![Vec::new(); ncol];
    for (n, line) in lines {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let mut count = 0;
        let mut push = |tok: &str| -> Result<()> {
            if count >= ncol {
                return Err(Error::new(ErrorCode::DataCorrupt, format!("Line {} has more than {ncol} values.", n + 1)).with_details(serde_json::json!({ "line": n + 1 })));
            }
            let v: f64 = tok.trim().parse().map_err(|_| {
                Error::new(ErrorCode::DataCorrupt, format!("Line {}: '{}' is not a number (column '{}').", n + 1, tok.trim(), columns[count]))
                    .with_details(serde_json::json!({ "line": n + 1, "column": columns[count] }))
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
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Line {} has {count} values; expected {ncol}.", n + 1)).with_details(serde_json::json!({ "line": n + 1 })));
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
    }
}
