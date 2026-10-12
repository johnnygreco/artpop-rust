//! The operation registry: one definition per operation, used verbatim by
//! the CLI (`invoke`, domain subcommands), MCP tool listing, schemas, and
//! capability discovery.

use crate::ops::*;
use crate::{Ctx, OpOutput};
use artpop_core::error::Result;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Effects {
    pub read_only: bool,
    pub writes_artifacts: bool,
    pub writes_files: bool,
    pub network: bool,
    pub destructive: bool,
    pub idempotent: bool,
}

/// Reads artifacts/metadata only.
pub const QUERY: Effects = Effects { read_only: true, writes_artifacts: false, writes_files: false, network: false, destructive: false, idempotent: true };
/// Writes content-addressed artifacts (same request => same artifact).
pub const PRODUCE: Effects = Effects { read_only: false, writes_artifacts: true, writes_files: false, network: false, destructive: false, idempotent: true };
/// Writes files outside the artifact store.
pub const EXPORT: Effects = Effects { read_only: false, writes_artifacts: false, writes_files: true, network: false, destructive: false, idempotent: false };
/// Writes into the configured data cache.
pub const DATA: Effects = Effects { read_only: false, writes_artifacts: false, writes_files: true, network: false, destructive: false, idempotent: true };
pub const NETWORK: Effects = Effects { read_only: false, writes_artifacts: false, writes_files: true, network: true, destructive: false, idempotent: true };
/// Job control (state changes in the job store).
pub const JOB: Effects = Effects { read_only: false, writes_artifacts: true, writes_files: false, network: false, destructive: false, idempotent: false };

pub struct OperationDef {
    pub id: &'static str,
    pub summary: &'static str,
    pub effects: Effects,
    pub stochastic: bool,
    pub cost: &'static str,
    pub schema: fn() -> Value,
    pub handler: fn(&Ctx, Value) -> Result<OpOutput>,
    /// Strict parse only: returns the request with defaults applied.
    pub validate: fn(Value) -> Result<Value>,
}

impl OperationDef {
    pub fn domain(&self) -> &'static str {
        self.id.split('.').next().unwrap_or(self.id)
    }

    pub fn mcp_name(&self) -> String {
        mcp_name(self.id)
    }

    /// Full request schema including the common envelope fields.
    pub fn input_schema(&self) -> Value {
        let mut s = (self.schema)();
        let exec = serde_json::to_value(schemars::schema_for!(crate::types::ExecutionSpec)).unwrap();
        if let Value::Object(m) = &mut s {
            let defs = m.entry("$defs").or_insert_with(|| json!({}));
            if let (Value::Object(d), Value::Object(e)) = (defs, &exec) {
                d.insert("ExecutionSpec".into(), Value::Object(e.iter().filter(|(k, _)| k.as_str() != "$schema" && k.as_str() != "title").map(|(k, v)| (k.clone(), v.clone())).collect()));
            }
            let props = m.entry("properties").or_insert_with(|| json!({}));
            if let Value::Object(p) = props {
                p.insert("schema_version".into(), json!({ "const": "1", "description": "Request schema version." }));
                p.insert(
                    "execution".into(),
                    json!({ "$ref": "#/$defs/ExecutionSpec", "description": if self.stochastic { "Required: provide execution.seed for this stochastic operation." } else { "Optional execution controls and limits." } }),
                );
            }
            let req = m.entry("required").or_insert_with(|| json!([]));
            if let Value::Array(r) = req {
                r.insert(0, json!("schema_version"));
            }
            m.insert("additionalProperties".into(), json!(false));
            m.insert("description".into(), json!(self.summary));
        }
        s
    }
}

pub fn mcp_name(id: &str) -> String {
    format!("artpop_{}", id.replace(['.', '-'], "_"))
}

macro_rules! op {
    ($id:literal, $req:ty, $handler:path, $effects:expr, $stochastic:expr, $cost:literal, $summary:literal) => {
        OperationDef {
            id: $id,
            summary: $summary,
            effects: $effects,
            stochastic: $stochastic,
            cost: $cost,
            schema: || serde_json::to_value(schemars::schema_for!($req)).unwrap(),
            handler: |ctx, v| {
                let req: $req = crate::parse_request(v, "")?;
                let resolved = serde_json::to_value(&req).unwrap_or(Value::Null);
                let mut out = $handler(ctx, req)?;
                out.resolved = resolved;
                Ok(out)
            },
            validate: |v| {
                let req: $req = crate::parse_request(v, "")?;
                Ok(serde_json::to_value(&req).unwrap_or(Value::Null))
            },
        }
    };
}

fn build() -> Vec<OperationDef> {
    vec![
        // Discovery, validation, and data.
        op!("system.capabilities", system::CapabilitiesRequest, system::capabilities, QUERY, false, "light", "Engine, schema, and protocol versions; implemented operations; algorithms; limits; formats; bundled data."),
        op!("system.schema", system::SchemaRequest, system::schema, QUERY, false, "light", "JSON Schema of an operation's request (input) or result envelope (output), with an example."),
        op!("system.validate", system::ValidateRequest, system::validate, QUERY, false, "light", "Validate a request for an operation and return resolved defaults without executing it."),
        op!("system.estimate", system::EstimateRequest, system::estimate, QUERY, false, "light", "Estimate stars, grid/image/FFT sizes, and working memory for a request before running it."),
        op!("system.citations", system::CitationsRequest, system::citations, QUERY, false, "light", "Scientific and software references, plus the provenance of bundled data."),
        op!("data.list", data::ListRequest, data::list, QUERY, false, "light", "List registered model datasets (MIST) in the data directory and their verification state."),
        op!("data.import", data::ImportRequest, data::import, DATA, false, "moderate", "Register an existing local MIST grid directory: validate members, hash files, write a manifest."),
        op!("data.verify", data::VerifyRequest, data::verify, QUERY, false, "moderate", "Re-hash a registered dataset and report missing or corrupt members."),
        op!("data.fetch", data::FetchRequest, data::fetch, NETWORK, false, "heavy", "Explicitly download a MIST v1.2 grid archive, verify it, extract it safely, and register it."),
        // Filters and IMF.
        op!("filters.list", filters::ListRequest, filters::list, QUERY, false, "light", "List photometric systems and filters with native magnitude systems and available conversions."),
        op!("filters.inspect", filters::InspectRequest, filters::inspect, QUERY, false, "light", "Filter properties (width, effective/pivot wavelength, throughput) from bundled metadata or curves."),
        op!("filters.import", filters::ImportRequest, filters::import, PRODUCE, false, "light", "Import custom throughput curves; compute Fukugita et al. (1996) properties as a filter catalog artifact."),
        op!("filters.convert", filters::ConvertRequest, filters::convert, QUERY, false, "light", "Convert magnitudes or colors between AB, Vega, and ST using the MIST zero-point table."),
        op!("imf.evaluate", imf::EvaluateRequest, imf::evaluate, QUERY, false, "light", "Evaluate dN/dm or m dN/dm of an IMF at masses with raw, sum, number, or mass normalization."),
        op!("imf.integrate", imf::IntegrateRequest, imf::integrate, QUERY, false, "light", "Integrate the IMF number or mass moment between mass limits (optionally normalized)."),
        op!("imf.sample", imf::SampleRequest, imf::sample, PRODUCE, true, "moderate", "Draw a fixed number of initial masses (tabulated or analytic inverse CDF)."),
        op!("imf.build", imf::BuildRequest, imf::build, PRODUCE, true, "moderate", "Draw initial masses until a target total initial mass is reached (whole-batch or nearest-target)."),
        // Isochrones.
        op!("isochrone.import", isochrone::ImportRequest, isochrone::import, PRODUCE, false, "moderate", "Import an isochrone from CSV, PARSEC CMD output, a MIST .iso.cmd file, or inline columns."),
        op!("isochrone.mist", isochrone::MistRequest, isochrone::mist, PRODUCE, false, "moderate", "Select a MIST v1.2 isochrone (nearest grid age, interpolated [Fe/H]) from local data in AB/Vega/ST."),
        op!("isochrone.inspect", isochrone::InspectRequest, isochrone::inspect, QUERY, false, "light", "Summarize an isochrone or page through its columns."),
        op!("isochrone.interpolate", isochrone::InterpolateRequest, isochrone::interpolate, QUERY, false, "light", "Interpolate one isochrone column against another (SciPy interp1d kinds and bounds)."),
        op!("isochrone.invert", isochrone::InvertRequest, isochrone::invert, QUERY, false, "light", "All initial masses at which a band reaches a magnitude (no monotonicity assumption)."),
        op!("isochrone.magnitude-limit", isochrone::MagLimitRequest, isochrone::magnitude_limit, QUERY, false, "light", "Magnitude and mass limits that sample a given number or mass fraction of a population."),
        op!("isochrone.nearest", isochrone::NearestRequest, isochrone::nearest, QUERY, false, "light", "Nearest tabulated initial masses and their rows."),
        op!("isochrone.weights", isochrone::WeightsRequest, isochrone::weights, QUERY, false, "light", "Per-row IMF weights (mass or number normalized) of an isochrone."),
        op!("isochrone.integrated", isochrone::IntegratedRequest, isochrone::integrated, QUERY, false, "light", "IMF-weighted SSP magnitudes, colors, SBF magnitudes, mean stellar mass, and surviving mass."),
        // Populations and sources.
        op!("population.build", population::BuildRequest, population::build, PRODUCE, true, "heavy", "Sample a simple stellar population by count or current mass with optional magnitude cutoff, extinction, and remnants."),
        op!("population.compose", population::ComposeRequest, population::compose, PRODUCE, false, "moderate", "Combine populations as labeled components, preserving each one's moments and attenuation."),
        op!("population.inspect", population::InspectRequest, population::inspect, QUERY, false, "light", "Mass definitions, counts, sampled fractions, distances, and per-component metadata."),
        op!("population.query", population::QueryRequest, population::query, QUERY, false, "light", "Page through discrete stars (masses, absolute/apparent magnitudes, EEP, phases)."),
        op!("population.photometry", population::PhotometryRequest, population::photometry, QUERY, false, "light", "Total, mean, and SBF magnitudes and colors, with discrete and unresolved moments."),
        op!("population.update", population::UpdateRequest, population::update, PRODUCE, false, "light", "Change distance and/or extinction without resampling."),
        op!("population.sample-fraction", population::SampleFractionRequest, population::sample_fraction, QUERY, false, "light", "Mass cutoff and sampled number/mass fractions for a magnitude limit, without building."),
        op!("population.phases", population::PhasesRequest, population::phases, QUERY, false, "light", "MIST phase counts, labels, or a phase selection mask artifact."),
        op!("population.surface-density", population::SurfaceDensityRequest, population::surface_density, QUERY, false, "light", "Expected stars per square arcsec and per pixel for a uniform surface brightness."),
        op!("source.build", source::BuildRequest, source::build, PRODUCE, true, "heavy", "Make a source from a catalog, a population plus spatial model, or a uniform surface brightness."),
        op!("source.compose", source::ComposeRequest, source::compose, PRODUCE, false, "moderate", "Combine sources on one geometry into a scene, keeping smooth components by default."),
        op!("source.inspect", source::InspectRequest, source::inspect, QUERY, false, "light", "Geometry, counts, labels, smooth components, and provenance of a source."),
        op!("source.query", source::QueryRequest, source::query, QUERY, false, "light", "Page through source stars or produce a label/selection mask artifact."),
        op!("source.amplitude", source::AmplitudeRequest, source::amplitude, QUERY, false, "light", "Smooth-component surface brightness and image amplitude for a total magnitude."),
        // Spatial sampling and imaging.
        op!("spatial.evaluate", spatial::EvaluateRequest, spatial::evaluate, QUERY, false, "light", "Evaluate Sérsic, Plummer, constant, ring, Gaussian, or summed models at points or on a grid."),
        op!("spatial.grid-build", spatial::GridBuildRequest, spatial::grid_build, PRODUCE, false, "moderate", "Build a validated probability grid from supplied weights or a model, entirely in Rust."),
        op!("spatial.sample", spatial::SampleRequest, spatial::sample, PRODUCE, true, "moderate", "Sample positions from a grid, Sérsic, Plummer, or uniform distribution with offsets and outside policy."),
        op!("psf.build", psf::BuildRequest, psf::build, PRODUCE, false, "light", "Gaussian or Moffat PSF with center, linear_interp, oversample, or integrate discretization."),
        op!("psf.import", psf::ImportRequest, psf::import, PRODUCE, false, "light", "Import an empirical PSF from FITS, an image artifact, or inline values."),
        op!("imager.inspect", imager::InspectRequest, imager::inspect, QUERY, false, "light", "Validate an instrument description and report bands, aperture area, and calibration mode."),
        op!("imager.convert", imager::ConvertRequest, imager::convert, QUERY, false, "light", "Convert magnitudes to counts, sky brightness to counts per pixel, or report calibration factors."),
        op!("image.histogram", image::HistogramRequest, image::histogram, PRODUCE, false, "moderate", "Accumulate stars into a weighted image (pixel-centers-v1 or artpop-0.1.2 bins) with diagnostics."),
        op!("image.smooth", image::SmoothRequest, image::smooth, PRODUCE, false, "moderate", "Render a source's smooth (unresolved) components into an image in flux or count units."),
        op!("image.convolve", image::ConvolveRequest, image::convolve, PRODUCE, false, "moderate", "Convolve an image with a PSF (wrap or fill boundary; direct or FFT)."),
        op!("image.embed", image::EmbedRequest, image::embed, PRODUCE, false, "light", "Add or place an image into another at a pixel center with clipping (FITS headers preserved)."),
        op!("image.combine", image::CombineRequest, image::combine, PRODUCE, false, "light", "Linear combination of same-geometry images with unit and band checks."),
        op!("image.preview", image::PreviewRequest, image::preview, PRODUCE, false, "light", "Percentile-clipped PNG preview of an image or observation plane."),
        op!("image.rgb", image::RgbRequest, image::rgb, PRODUCE, false, "light", "Asinh RGB composite PNG from three registered planes."),
        op!("plot.render", image::PlotRequest, image::plot, PRODUCE, false, "light", "SVG color-magnitude, HR, spatial, or IMF plot."),
        op!("observe.ideal", observe::IdealRequest, observe::ideal, PRODUCE, false, "moderate", "Noise-free observation: stars and smooth light in flux units, optionally PSF-convolved."),
        op!("observe.artificial", observe::ArtificialRequest, observe::artificial, PRODUCE, true, "heavy", "Detector observation: expected counts, sky, Poisson and read noise, calibrated image, variances."),
        op!("observation.inspect", observe::InspectRequest, observe::inspect, QUERY, false, "light", "Observation metadata, plane statistics, and bounded cutouts."),
        // Persistence and orchestration.
        op!("artifact.list", artifact::ListRequest, artifact::list, QUERY, false, "light", "List stored artifacts (optionally by kind)."),
        op!("artifact.inspect", artifact::InspectRequest, artifact::inspect, QUERY, false, "light", "Manifest, member digests, shapes, units, and inputs of an artifact."),
        op!("artifact.import", artifact::ImportRequest, artifact::import, PRODUCE, false, "moderate", "Import a FITS image, CSV catalog/positions, or portable bundle directory as a validated artifact."),
        op!("artifact.export", artifact::ExportRequest, artifact::export, EXPORT, false, "moderate", "Export an artifact member to FITS, CSV, JSON, PNG/SVG, or a portable bundle directory."),
        op!("artifact.clone", artifact::CloneRequest, artifact::clone, PRODUCE, false, "light", "Return an alias for an immutable artifact, optionally exporting a physical copy."),
        op!("simulate.run", simulate::RunRequest, simulate::run, PRODUCE, true, "heavy", "End-to-end: population, source, PSF, and one or more observations with exports."),
        op!("job.submit", jobs::SubmitRequest, jobs::submit, JOB, false, "light", "Submit a producing operation as a durable background job (idempotency keys supported)."),
        op!("job.status", jobs::StatusRequest, jobs::status, QUERY, false, "light", "State, stage, and timing of a job."),
        op!("job.cancel", jobs::CancelRequest, jobs::cancel, JOB, false, "light", "Request cooperative cancellation of a job."),
        op!("job.result", jobs::ResultRequest, jobs::result, QUERY, false, "light", "Completed result envelope of a job, or a typed not-ready/cancelled/failed response."),
    ]
}

pub fn all() -> &'static [OperationDef] {
    static REGISTRY: OnceLock<Vec<OperationDef>> = OnceLock::new();
    REGISTRY.get_or_init(build)
}

pub fn find(id: &str) -> Option<&'static OperationDef> {
    all().iter().find(|d| d.id == id || d.mcp_name() == id)
}

/// JSON Schema of the result envelope shared by every operation.
pub fn envelope_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "ResultEnvelope",
        "type": "object",
        "required": ["schema_version", "status", "operation", "request_id", "artifacts", "warnings", "provenance", "metrics"],
        "properties": {
            "schema_version": { "const": "1" },
            "status": { "enum": ["completed", "accepted", "error"] },
            "operation": { "type": "string" },
            "request_id": { "type": "string" },
            "result": { "type": "object" },
            "artifacts": { "type": "array", "items": {
                "type": "object",
                "required": ["artifact_id", "kind", "uri", "media_type"],
                "properties": {
                    "artifact_id": { "type": "string", "pattern": "^sha256:[0-9a-f]{64}$" },
                    "kind": { "type": "string" },
                    "uri": { "type": "string" },
                    "media_type": { "type": "string" },
                    "path": { "type": "string" },
                    "bytes": { "type": "integer" }
                }
            }},
            "warnings": { "type": "array", "items": { "type": "object", "properties": { "code": { "type": "string" }, "message": { "type": "string" } } } },
            "error": { "type": "object", "required": ["code", "message", "retryable"], "properties": {
                "code": { "type": "string" }, "message": { "type": "string" }, "path": { "type": "string" }, "details": {}, "retryable": { "type": "boolean" }
            }},
            "provenance": { "type": "object" },
            "resolved_request": { "type": "object" },
            "metrics": { "type": "object" }
        }
    })
}
