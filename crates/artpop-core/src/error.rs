//! Typed scientific errors shared by every layer.
//!
//! Codes are the stable families from the CLI/MCP contract. Core code never
//! panics on invalid user input; it returns one of these errors with the JSON
//! pointer of the offending request field when one is known.

use serde::Serialize;
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidParameter,
    UnknownFilter,
    MissingZeropoint,
    UnitMismatch,
    ShapeMismatch,
    DataMissing,
    DataCorrupt,
    ModelAlignment,
    OutOfDomain,
    ZeroWeightGrid,
    NonfiniteValue,
    ResourceLimit,
    NumericalConvergence,
    UnsupportedFormat,
    ArtifactNotFound,
    IoError,
    NetworkError,
    Cancelled,
    JobNotReady,
    IdempotencyConflict,
    InternalError,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidParameter => "INVALID_PARAMETER",
            ErrorCode::UnknownFilter => "UNKNOWN_FILTER",
            ErrorCode::MissingZeropoint => "MISSING_ZEROPOINT",
            ErrorCode::UnitMismatch => "UNIT_MISMATCH",
            ErrorCode::ShapeMismatch => "SHAPE_MISMATCH",
            ErrorCode::DataMissing => "DATA_MISSING",
            ErrorCode::DataCorrupt => "DATA_CORRUPT",
            ErrorCode::ModelAlignment => "MODEL_ALIGNMENT",
            ErrorCode::OutOfDomain => "OUT_OF_DOMAIN",
            ErrorCode::ZeroWeightGrid => "ZERO_WEIGHT_GRID",
            ErrorCode::NonfiniteValue => "NONFINITE_VALUE",
            ErrorCode::ResourceLimit => "RESOURCE_LIMIT",
            ErrorCode::NumericalConvergence => "NUMERICAL_CONVERGENCE",
            ErrorCode::UnsupportedFormat => "UNSUPPORTED_FORMAT",
            ErrorCode::ArtifactNotFound => "ARTIFACT_NOT_FOUND",
            ErrorCode::IoError => "IO_ERROR",
            ErrorCode::NetworkError => "NETWORK_ERROR",
            ErrorCode::Cancelled => "CANCELLED",
            ErrorCode::JobNotReady => "JOB_NOT_READY",
            ErrorCode::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }

    /// Process exit code used by the CLI for this error family.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::InvalidParameter
            | ErrorCode::UnknownFilter
            | ErrorCode::MissingZeropoint
            | ErrorCode::UnitMismatch
            | ErrorCode::ShapeMismatch
            | ErrorCode::OutOfDomain
            | ErrorCode::ZeroWeightGrid
            | ErrorCode::NonfiniteValue
            | ErrorCode::ModelAlignment
            | ErrorCode::UnsupportedFormat => 2,
            ErrorCode::DataMissing | ErrorCode::DataCorrupt | ErrorCode::ArtifactNotFound => 3,
            ErrorCode::ResourceLimit => 4,
            ErrorCode::IoError | ErrorCode::NetworkError => 5,
            ErrorCode::NumericalConvergence => 6,
            ErrorCode::JobNotReady | ErrorCode::IdempotencyConflict => 7,
            ErrorCode::Cancelled => 130,
            ErrorCode::InternalError => 1,
        }
    }

    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::IoError | ErrorCode::NetworkError | ErrorCode::JobNotReady | ErrorCode::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Error { code, message: message.into(), path: None, details: Value::Null }
    }

    pub fn invalid(path: &str, message: impl Into<String>) -> Self {
        Error::new(ErrorCode::InvalidParameter, message).at(path)
    }

    pub fn domain(path: &str, message: impl Into<String>) -> Self {
        Error::new(ErrorCode::OutOfDomain, message).at(path)
    }

    pub fn nonfinite(path: &str, message: impl Into<String>) -> Self {
        Error::new(ErrorCode::NonfiniteValue, message).at(path)
    }

    pub fn unknown_filter(path: &str, band: &str, available: &[String]) -> Self {
        Error::new(ErrorCode::UnknownFilter, format!("Filter '{band}' is not available."))
            .at(path)
            .with_details(serde_json::json!({ "received": band, "available": available }))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Error::new(ErrorCode::InternalError, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Error::new(ErrorCode::IoError, message)
    }

    /// Attach a JSON pointer unless a more specific one is already present.
    pub fn at(mut self, path: &str) -> Self {
        if self.path.is_none() && !path.is_empty() {
            self.path = Some(path.to_string());
        }
        self
    }

    /// Prefix the existing pointer, used when a nested request is validated.
    pub fn under(mut self, prefix: &str) -> Self {
        self.path = Some(match self.path.take() {
            Some(p) => format!("{prefix}{p}"),
            None => prefix.to_string(),
        });
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{} at {}: {}", self.code.as_str(), p, self.message),
            None => write!(f, "{}: {}", self.code.as_str(), self.message),
        }
    }
}

impl std::error::Error for Error {}

/// Return an error unless every value is finite.
pub fn require_finite(values: &[f64], path: &str, what: &str) -> Result<()> {
    if let Some(i) = values.iter().position(|v| !v.is_finite()) {
        return Err(Error::nonfinite(path, format!("{what} contains a non-finite value at index {i}."))
            .with_details(serde_json::json!({ "index": i })));
    }
    Ok(())
}

/// [`require_finite`] for large arrays: the scan runs in parallel and reports
/// the first non-finite index, exactly like the sequential check.
pub fn require_finite_par(values: &[f64], path: &str, what: &str) -> Result<()> {
    use rayon::prelude::*;
    if let Some(i) = values.par_iter().with_min_len(1 << 15).position_first(|v| !v.is_finite()) {
        return Err(Error::nonfinite(path, format!("{what} contains a non-finite value at index {i}."))
            .with_details(serde_json::json!({ "index": i })));
    }
    Ok(())
}

pub fn require_positive(value: f64, path: &str, what: &str) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        return Err(Error::invalid(path, format!("{what} must be a finite number greater than zero."))
            .with_details(serde_json::json!({ "received": finite_or_null(value), "exclusive_minimum": 0 })));
    }
    Ok(())
}

pub fn require_nonnegative(value: f64, path: &str, what: &str) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::invalid(path, format!("{what} must be a finite number greater than or equal to zero."))
            .with_details(serde_json::json!({ "received": finite_or_null(value), "minimum": 0 })));
    }
    Ok(())
}

/// JSON has no NaN/infinity; represent them as null.
pub fn finite_or_null(value: f64) -> Value {
    if value.is_finite() { Value::from(value) } else { Value::Null }
}
