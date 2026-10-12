//! Request vocabulary shared by operations (strict: unknown fields rejected).

use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::imf::Imf;
use artpop_core::population::CutoffFrame;
use artpop_core::spatial::Coordinates;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A finite value with an explicit unit; the dimension is checked before use.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
}

impl Quantity {
    pub fn new(value: f64, unit: &str) -> Quantity {
        Quantity { value, unit: unit.into() }
    }
}

/// Reference to an immutable artifact in the store.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInput {
    /// `sha256:<64 hex digits>` returned by a producing operation.
    pub artifact_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_scale_arcsec: Option<f64>,
    /// `pixel-centers-v1` (default) or `artpop-0.1.2`.
    #[serde(default)]
    pub coordinates: Coordinates,
}

impl Geometry {
    pub fn validate(&self, pointer: &str, max_pixels: u64) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::invalid(&format!("{pointer}/width"), "Image width and height must be positive."));
        }
        let p = self.width as u64 * self.height as u64;
        if p > max_pixels {
            return Err(Error::new(ErrorCode::ResourceLimit, format!("A {}x{} image exceeds the configured pixel limit.", self.width, self.height))
                .at(pointer)
                .with_details(serde_json::json!({ "pixels": p, "max_image_pixels": max_pixels })));
        }
        if let Some(ps) = self.pixel_scale_arcsec {
            artpop_core::error::require_positive(ps, &format!("{pointer}/pixel_scale_arcsec"), "Pixel scale")?;
        }
        if self.coordinates == Coordinates::ArtPop012 && (self.width.is_multiple_of(2) || self.height.is_multiple_of(2)) {
            return Err(Error::invalid(&format!("{pointer}/coordinates"), "The artpop-0.1.2 convention requires odd image dimensions (ArtPop check_xy_dim).")
                .with_details(serde_json::json!({ "width": self.width, "height": self.height, "alternative": "pixel-centers-v1" })));
        }
        Ok(())
    }

    pub fn pixel_scale(&self, pointer: &str) -> Result<f64> {
        self.pixel_scale_arcsec.ok_or_else(|| Error::invalid(&format!("{pointer}/pixel_scale_arcsec"), "This operation needs geometry.pixel_scale_arcsec."))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(Default)]
pub enum ImfSpec {
    /// Kroupa (2001): slopes -0.3, -1.3, -2.3; breaks 0.08, 0.5 Msun.
    #[default]
    Kroupa,
    /// Salpeter (1955): slope -2.35.
    Salpeter,
    /// Scalo (1998): slopes -1.2, -2.7, -2.3; breaks 1, 10 Msun.
    Scalo,
    /// Single power law dN/dm ∝ m^slope.
    PowerLaw { slope: f64 },
    /// Continuous broken power law (`slopes.len() == breaks.len() + 1`).
    BrokenPowerLaw { slopes: Vec<f64>, breaks: Vec<f64> },
}


impl ImfSpec {
    pub fn build(&self) -> Result<Imf> {
        match self {
            ImfSpec::Kroupa => Ok(Imf::kroupa()),
            ImfSpec::Salpeter => Ok(Imf::salpeter()),
            ImfSpec::Scalo => Ok(Imf::scalo()),
            ImfSpec::PowerLaw { slope } => Imf::piecewise("power_law", vec![*slope], vec![]),
            ImfSpec::BrokenPowerLaw { slopes, breaks } => Imf::piecewise("broken_power_law", slopes.clone(), breaks.clone()),
        }
    }
}

/// Uniform extinction in magnitudes, or a per-band map (absent bands are 0).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Extinction {
    Uniform(f64),
    PerBand(IndexMap<String, f64>),
}

impl Default for Extinction {
    fn default() -> Self {
        Extinction::Uniform(0.0)
    }
}

impl Extinction {
    pub fn resolve(&self, filters: &[String], pointer: &str) -> Result<Vec<f64>> {
        let v = match self {
            Extinction::Uniform(a) => vec![*a; filters.len()],
            Extinction::PerBand(m) => {
                for k in m.keys() {
                    if !filters.contains(k) {
                        return Err(Error::unknown_filter(pointer, k, filters));
                    }
                }
                filters.iter().map(|f| m.get(f).copied().unwrap_or(0.0)).collect()
            }
        };
        if v.iter().any(|a| !a.is_finite()) {
            return Err(Error::nonfinite(pointer, "Extinction values must be finite."));
        }
        if v.iter().any(|a| *a < 0.0) {
            return Err(Error::invalid(pointer, "Negative extinction is not physical."));
        }
        Ok(v)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CountOrMass {
    /// Number of stars formed in the population.
    Count { value: u64 },
    /// Current stellar mass including remnants when `add_remnants` is true.
    CurrentMass {
        value: f64,
        #[serde(default = "msun")]
        unit: String,
    },
}

fn msun() -> String {
    "Msun".into()
}

fn observed() -> CutoffFrame {
    CutoffFrame::Observed
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagLimitSpec {
    /// Stars fainter than this apparent magnitude become unresolved light.
    pub magnitude: f64,
    pub band: String,
    /// `observed` includes extinction (default); `intrinsic_apparent` is ArtPop 0.1.2.
    #[serde(default = "observed")]
    pub frame: CutoffFrame,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    #[default]
    All,
    Components { ids: Vec<u32> },
    /// Match source labels (numbers or strings).
    Labels { values: Vec<Value> },
    /// MIST phases selected from interpolated EEPs.
    Phases { phases: Vec<String> },
    Rows {
        start: usize,
        #[serde(default)]
        stop: Option<usize>,
        #[serde(default = "one")]
        step: usize,
    },
    /// A stored boolean mask artifact.
    Mask { artifact_id: String },
}

fn one() -> usize {
    1
}

impl Selection {
    pub fn is_all(&self) -> bool {
        matches!(self, Selection::All)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Opaque cursor returned by a previous page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Rows per page (default 100, maximum 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl Page {
    pub fn resolve(&self, max: usize) -> Result<(usize, usize)> {
        let start = match &self.cursor {
            None => 0,
            Some(c) => c.strip_prefix("row:").and_then(|v| v.parse().ok()).ok_or_else(|| Error::invalid("/page/cursor", "Unrecognized page cursor."))?,
        };
        let limit = self.limit.unwrap_or(100);
        if limit == 0 || limit > max {
            return Err(Error::invalid("/page/limit", format!("Page limit must be between 1 and {max}.")));
        }
        Ok((start, limit))
    }
}

/// Execution controls common to every request (stripped before the
/// operation-specific fields are validated).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    #[serde(default = "profile")]
    pub algorithm_profile: String,
    /// 16 hexadecimal digits; required by stochastic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_grid_pixels: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

fn profile() -> String {
    artpop_core::ALGORITHM_PROFILE.into()
}

impl Default for ExecutionSpec {
    fn default() -> Self {
        ExecutionSpec { algorithm_profile: profile(), seed: None, max_stars: None, max_grid_pixels: None, max_memory_bytes: None, idempotency_key: None }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TableFormat {
    Csv,
}

/// A numeric CSV catalog with a header row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogInput {
    pub format: TableFormat,
    pub path: String,
    #[serde(default = "x_col")]
    pub x_column: String,
    #[serde(default = "y_col")]
    pub y_column: String,
    /// Band name -> column name.
    #[serde(default)]
    pub magnitude_columns: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnitude_system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_column: Option<String>,
    /// `,` (default when the header has commas), `whitespace`, or one character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
}

fn x_col() -> String {
    "x".into()
}

fn y_col() -> String {
    "y".into()
}

pub fn delimiter(d: &Option<String>, pointer: &str) -> Result<Option<artpop_io::csv::Delimiter>> {
    use artpop_io::csv::Delimiter;
    Ok(match d.as_deref() {
        None => None,
        Some(",") => Some(Delimiter::Comma),
        Some("whitespace") | Some(" ") => Some(Delimiter::Whitespace),
        Some(s) if s.chars().count() == 1 => Some(Delimiter::Char(s.chars().next().unwrap())),
        Some(_) => return Err(Error::invalid(pointer, "Delimiter must be ',', 'whitespace', or a single character.")),
    })
}
