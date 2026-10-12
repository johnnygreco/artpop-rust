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
use serde_json::Value;

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
