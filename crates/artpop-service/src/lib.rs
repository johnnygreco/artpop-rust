//! artpop-service: the single typed operation layer behind the CLI and MCP.
//!
//! Every scientific capability is an [`registry::OperationDef`] with a stable
//! dotted ID, a strict JSON Schema generated from its Rust request type, side
//! effect annotations, and one executor. Adapters only translate transport;
//! they never implement science.

// Errors carry JSON details for agents; they are cold-path values, so the
// large-Err lint is not useful. `!(x > 0.0)` comparisons deliberately reject NaN.
#![allow(clippy::result_large_err, clippy::neg_cmp_op_on_partial_ord)]

pub mod codec;
pub mod config;
pub mod ops;
pub mod registry;
pub mod store;
pub mod types;
pub mod units;

use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::rng::Seed;
use config::ServiceConfig;
use serde::Serialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::time::Instant;
use store::{ArtifactRef, Bundle, Store};
use types::ExecutionSpec;

pub const SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

pub struct Service {
    pub config: ServiceConfig,
    pub store: Store,
}

/// Effective execution settings after merging request and service limits.
#[derive(Debug, Clone, Serialize)]
pub struct Execution {
    pub algorithm_profile: String,
    pub seed: Option<String>,
    pub max_stars: u64,
    pub max_grid_pixels: u64,
    pub max_memory_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

pub struct Ctx<'a> {
    pub svc: &'a Service,
    pub exec: Execution,
    pub operation: &'static str,
    warnings: RefCell<Vec<Warning>>,
    seed_used: RefCell<bool>,
    data_ids: RefCell<Vec<String>>,
}

#[derive(Default)]
pub struct OpOutput {
    pub result: Value,
    pub artifacts: Vec<ArtifactRef>,
    pub resolved: Value,
}

impl OpOutput {
    pub fn new(result: Value) -> OpOutput {
        OpOutput { result, ..Default::default() }
    }

    pub fn with(mut self, a: ArtifactRef) -> OpOutput {
        self.artifacts.push(a);
        self
    }
}

impl<'a> Ctx<'a> {
    pub fn store(&self) -> &Store {
        &self.svc.store
    }

    pub fn config(&self) -> &ServiceConfig {
        &self.svc.config
    }

    pub fn warn(&self, code: &str, message: impl Into<String>) {
        self.warnings.borrow_mut().push(Warning { code: code.into(), message: message.into() });
    }

    pub fn note_data(&self, id: impl Into<String>) {
        self.data_ids.borrow_mut().push(id.into());
    }

    /// The request seed; stochastic operations fail without one.
    pub fn seed(&self) -> Result<Seed> {
        let s = self.exec.seed.as_deref().ok_or_else(|| {
            Error::invalid("/execution/seed", "This operation is stochastic and requires execution.seed (16 hexadecimal digits).")
                .with_details(json!({ "example": { "execution": { "seed": "000000000000002a" } } }))
        })?;
        *self.seed_used.borrow_mut() = true;
        Seed::parse(s, "/execution/seed")
    }

    pub fn put(&self, kind: &'static str, metadata: Value, members: Vec<store::Member>, inputs: Vec<String>) -> Result<ArtifactRef> {
        let (r, _) = self.svc.store.put(Bundle { kind, created_by: self.operation.into(), inputs, metadata, members })?;
        Ok(r)
    }

    pub fn read_path(&self, raw: &str, pointer: &str) -> Result<std::path::PathBuf> {
        self.svc.config.check_path(raw, pointer)
    }

    pub fn read_text(&self, raw: &str, pointer: &str) -> Result<String> {
        let p = self.read_path(raw, pointer)?;
        std::fs::read_to_string(&p).map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound { ErrorCode::DataMissing } else { ErrorCode::IoError };
            Error::new(code, format!("Cannot read {}: {e}", p.display())).at(pointer)
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
    pub retryable: bool,
}

impl From<&Error> for ErrorBody {
    fn from(e: &Error) -> Self {
        ErrorBody { code: e.code.as_str(), message: e.message.clone(), path: e.path.clone(), details: e.details.clone(), retryable: e.code.retryable() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub schema_version: &'static str,
    pub status: &'static str,
    pub operation: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    pub artifacts: Vec<ArtifactRef>,
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    pub provenance: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_request: Option<Value>,
    pub metrics: Value,
}

impl Envelope {
    pub fn exit_code(&self) -> i32 {
        match &self.error {
            None => 0,
            Some(e) => code_from_str(e.code).exit_code(),
        }
    }
}

fn code_from_str(s: &str) -> ErrorCode {
    use ErrorCode::*;
    [
        InvalidParameter, UnknownFilter, MissingZeropoint, UnitMismatch, ShapeMismatch, DataMissing, DataCorrupt, ModelAlignment, OutOfDomain, ZeroWeightGrid,
        NonfiniteValue, ResourceLimit, NumericalConvergence, UnsupportedFormat, ArtifactNotFound, IoError, NetworkError, Cancelled, JobNotReady, IdempotencyConflict,
        InternalError,
    ]
    .into_iter()
    .find(|c| c.as_str() == s)
    .unwrap_or(InternalError)
}

/// Convert a `serde_path_to_error` path into a JSON pointer.
pub fn json_pointer(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut s = String::new();
    for seg in path.iter() {
        match seg {
            Segment::Seq { index } => s.push_str(&format!("/{index}")),
            Segment::Map { key } => s.push_str(&format!("/{}", key.replace('~', "~0").replace('/', "~1"))),
            Segment::Enum { variant } => s.push_str(&format!("/{variant}")),
            Segment::Unknown => {}
        }
    }
    s
}

/// Strictly deserialize a request, reporting the failing JSON pointer.
pub fn parse_request<T: for<'de> serde::Deserialize<'de>>(v: Value, prefix: &str) -> Result<T> {
    serde_path_to_error::deserialize(v).map_err(|e| {
        let pointer = format!("{prefix}{}", json_pointer(e.path()));
        let inner = e.inner().to_string();
        Error::invalid(&pointer, format!("Invalid request: {inner}")).with_details(json!({ "pointer": pointer }))
    })
}

fn process_peak_rss() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).map(|kb| kb * 1024)
}

impl Service {
    pub fn new(config: ServiceConfig) -> Service {
        let store = Store::new(config.artifact_root.clone());
        Service { config, store }
    }

    fn resolve_execution(&self, spec: &ExecutionSpec) -> Result<Execution> {
        if spec.algorithm_profile != artpop_core::ALGORITHM_PROFILE {
            return Err(Error::invalid("/execution/algorithm_profile", format!("Unsupported algorithm profile '{}'.", spec.algorithm_profile))
                .with_details(json!({ "supported": [artpop_core::ALGORITHM_PROFILE] })));
        }
        if let Some(s) = &spec.seed {
            Seed::parse(s, "/execution/seed")?;
        }
        let l = &self.config.limits;
        let tighten = |req: Option<u64>, cfg: u64, name: &str| -> Result<u64> {
            match req {
                None => Ok(cfg),
                Some(v) if v <= cfg => Ok(v),
                Some(v) => Err(Error::new(ErrorCode::ResourceLimit, format!("execution.{name} = {v} exceeds the service limit {cfg}."))
                    .at(&format!("/execution/{name}"))
                    .with_details(json!({ "requested": v, "service_limit": cfg }))),
            }
        };
        Ok(Execution {
            algorithm_profile: spec.algorithm_profile.clone(),
            seed: spec.seed.clone(),
            max_stars: tighten(spec.max_stars, l.max_stars, "max_stars")?,
            max_grid_pixels: tighten(spec.max_grid_pixels, l.max_grid_pixels, "max_grid_pixels")?,
            max_memory_bytes: tighten(spec.max_memory_bytes, l.max_memory_bytes, "max_memory_bytes")?,
            idempotency_key: spec.idempotency_key.clone(),
        })
    }

    /// Split the common envelope fields from an operation request.
    pub fn prepare(&self, request: Value) -> Result<(serde_json::Map<String, Value>, Execution)> {
        let Value::Object(mut obj) = request else {
            return Err(Error::invalid("", "A request must be a JSON object."));
        };
        match obj.remove("schema_version") {
            Some(Value::String(s)) if s == SCHEMA_VERSION => {}
            Some(other) => {
                return Err(Error::invalid("/schema_version", "Unsupported schema_version.").with_details(json!({ "received": other, "supported": [SCHEMA_VERSION] })));
            }
            None => return Err(Error::invalid("/schema_version", "Every request must include \"schema_version\": \"1\".")),
        }
        let spec: ExecutionSpec = match obj.remove("execution") {
            Some(v) => parse_request(v, "/execution")?,
            None => ExecutionSpec::default(),
        };
        Ok((obj, self.resolve_execution(&spec)?))
    }

    pub fn execute(&self, operation: &str, request: Value) -> Envelope {
        let started = Instant::now();
        let request_id = format!("req-{}", uuid::Uuid::new_v4().simple());
        let mut meta = OutputMeta::default();
        // Report the canonical dotted ID even when called by MCP tool name.
        let canonical = registry::find(operation).map_or(operation, |d| d.id);
        let outcome = self.run(canonical, request, &mut meta);
        self.finish(canonical, request_id, started, outcome, meta)
    }

    fn run(&self, operation: &str, request: Value, meta: &mut OutputMeta) -> Result<(OpOutput, Execution)> {
        let def = registry::find(operation).ok_or_else(|| {
            Error::invalid("/operation", format!("Unknown operation '{operation}'."))
                .with_details(json!({ "available": registry::all().iter().map(|d| d.id).collect::<Vec<_>>() }))
        })?;
        let (obj, exec) = self.prepare(request)?;
        let ctx = Ctx {
            svc: self,
            exec: exec.clone(),
            operation: def.id,
            warnings: RefCell::new(Vec::new()),
            seed_used: RefCell::new(false),
            data_ids: RefCell::new(Vec::new()),
        };
        let out = (def.handler)(&ctx, Value::Object(obj));
        meta.warnings = ctx.warnings.take();
        meta.seed_used = *ctx.seed_used.borrow();
        meta.data_ids = ctx.data_ids.take();
        Ok((out?, exec))
    }

    fn finish(&self, operation: &str, request_id: String, started: Instant, outcome: Result<(OpOutput, Execution)>, meta: OutputMeta) -> Envelope {
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let metrics = json!({ "elapsed_ms": (elapsed_ms * 1000.0).round() / 1000.0, "process_peak_rss_bytes": process_peak_rss() });
        match outcome {
            Ok((out, exec)) => {
                let OpOutput { result, artifacts, resolved } = out;
                let mut resolved = resolved;
                if let Value::Object(m) = &mut resolved {
                    m.insert("schema_version".into(), json!(SCHEMA_VERSION));
                    m.insert("execution".into(), serde_json::to_value(&exec).unwrap());
                }
                Envelope {
                    schema_version: SCHEMA_VERSION,
                    status: "completed",
                    operation: operation.into(),
                    request_id,
                    result: Some(result),
                    artifacts,
                    warnings: meta.warnings,
                    error: None,
                    provenance: json!({
                        "engine_version": artpop_core::ENGINE_VERSION,
                        "algorithm_profile": exec.algorithm_profile,
                        "seed": if meta.seed_used { exec.seed.clone() } else { None },
                        "rng": if meta.seed_used { Some(json!({ "generator": artpop_core::rng::RNG_ALGORITHM, "blocks": artpop_core::rng::BLOCK_ALGORITHM, "normal": artpop_core::rng::NORMAL_ALGORITHM, "poisson": artpop_core::rng::POISSON_ALGORITHM })) } else { None },
                        "data_manifest_ids": meta.data_ids,
                    }),
                    resolved_request: Some(resolved),
                    metrics,
                }
            }
            Err(e) => {
                Envelope {
                    schema_version: SCHEMA_VERSION,
                    status: "error",
                    operation: operation.into(),
                    request_id,
                    result: None,
                    artifacts: Vec::new(),
                    warnings: meta.warnings,
                    error: Some(ErrorBody::from(&e)),
                    provenance: json!({ "engine_version": artpop_core::ENGINE_VERSION, "algorithm_profile": artpop_core::ALGORITHM_PROFILE }),
                    resolved_request: None,
                    metrics,
                }
            }
        }
    }
}

#[derive(Default)]
struct OutputMeta {
    warnings: Vec<Warning>,
    seed_used: bool,
    data_ids: Vec<String>,
}
