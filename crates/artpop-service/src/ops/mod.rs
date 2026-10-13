//! Operation handlers, one module per contract domain.

pub mod artifact;
pub mod data;
pub mod filters;
pub mod image;
pub mod imager;
pub mod imf;
pub mod isochrone;
pub mod jobs;
pub mod observe;
pub mod population;
pub mod psf;
pub mod simulate;
pub mod source;
pub mod spatial;
pub mod system;

use artpop_core::error::{Error, Result};
use serde_json::{Value, json};

/// Wall-clock stage timings reported in results as `timings_ms` (never stored
/// in artifacts, so they do not affect artifact IDs).
#[derive(Default)]
pub struct Timings(pub Vec<(&'static str, f64)>, Option<std::time::Instant>);

impl Timings {
    pub fn start() -> Timings {
        Timings(Vec::new(), Some(std::time::Instant::now()))
    }

    /// Record the time since the previous lap (or start) under `stage`.
    pub fn lap(&mut self, stage: &'static str) {
        let now = std::time::Instant::now();
        if let Some(t) = self.1 {
            self.0.push((stage, (now - t).as_secs_f64() * 1000.0));
        }
        self.1 = Some(now);
    }

    pub fn json(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.to_string(), json!((v * 10.0).round() / 10.0))).collect())
    }
}

/// Return only the requested fields of an object (all when `fields` is empty).
pub fn pick(all: Value, fields: &[String], pointer: &str) -> Result<Value> {
    if fields.is_empty() {
        return Ok(all);
    }
    let Value::Object(m) = all else { return Ok(all) };
    let mut out = serde_json::Map::new();
    for f in fields {
        match m.get(f) {
            Some(v) => {
                out.insert(f.clone(), v.clone());
            }
            None => {
                return Err(Error::invalid(pointer, format!("Unknown field '{f}'.")).with_details(serde_json::json!({ "available": m.keys().collect::<Vec<_>>() })));
            }
        }
    }
    Ok(Value::Object(out))
}

/// Magnitude from a flux; zero/negative flux becomes null with a status.
pub fn mag_or_null(flux: f64) -> Value {
    if flux > 0.0 && flux.is_finite() { Value::from(-2.5 * flux.log10()) } else { Value::Null }
}
