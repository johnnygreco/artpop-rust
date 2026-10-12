//! `simulate.run`: one request composing population, source, PSF, and
//! observations from the same primitives exposed individually.

use crate::ops::artifact::{ExportFormat, ExportRequest, export};
use crate::ops::image::{PreviewRequest, preview, ImageRef};
use crate::ops::imager::InstrumentSpec;
use crate::ops::observe::{ArtificialRequest, IdealRequest, artificial, ideal};
use crate::ops::population::{BuildRequest as PopulationRequest, build as build_population};
use crate::ops::psf::{BuildRequest as PsfRequest, build as build_psf};
use crate::ops::source::{BuildRequest as SourceRequest, SpatialModelSpec, build as build_source};
use crate::types::{ArtifactInput, Geometry, Selection};
use crate::{Ctx, OpOutput};
use artpop_core::convolve::{Boundary, Method};
use artpop_core::error::{Error, Result};
use artpop_core::image::InvalidPolicy;
use artpop_core::spatial::OutsidePolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn retain() -> OutsidePolicy {
    OutsidePolicy::RetainMarked
}
fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub spatial_model: SpatialModelSpec,
    pub geometry: Geometry,
    #[serde(default)]
    pub offset_pixels: [f64; 2],
    #[serde(default = "retain")]
    pub outside: OutsidePolicy,
    #[serde(default)]
    pub label_type: Option<String>,
    #[serde(default = "yes")]
    pub smooth_light: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationSpec {
    pub bandpass: String,
    /// Required for artificial observations.
    #[serde(default)]
    pub exposure_s: Option<f64>,
    #[serde(default)]
    pub sky_sb_mag_arcsec2: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderKind {
    Ideal,
    Artificial,
}

fn zpt27() -> f64 {
    27.0
}
fn wrap() -> Boundary {
    Boundary::Wrap
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderSpec {
    pub kind: RenderKind,
    #[serde(default = "wrap")]
    pub boundary: Boundary,
    #[serde(default = "zpt27")]
    pub output_zpt: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    Fits,
    Png,
    Csv,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct OutputsSpec {
    /// Directory for exported files (omit to keep artifacts only).
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub export_formats: Vec<ExportKind>,
    /// Also export variance planes for artificial observations.
    #[serde(default)]
    pub include_variance: bool,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub population: PopulationRequest,
    pub source: SourceSpec,
    /// PSF; `pixel_scale_arcsec` defaults to the source geometry's.
    #[serde(default)]
    pub psf: Option<PsfRequest>,
    #[serde(default)]
    pub instrument: Option<InstrumentSpec>,
    pub observations: Vec<ObservationSpec>,
    pub render: RenderSpec,
    #[serde(default)]
    pub outputs: OutputsSpec,
}

fn id_of(out: &OpOutput, key: &str) -> Result<String> {
    out.result.get(key).and_then(Value::as_str).map(String::from).ok_or_else(|| Error::internal(format!("missing {key} in sub-result")))
}

pub fn run(ctx: &Ctx, mut req: RunRequest) -> Result<OpOutput> {
    if req.observations.is_empty() {
        return Err(Error::invalid("/observations", "Give at least one observation."));
    }
    if req.render.kind == RenderKind::Artificial && req.instrument.is_none() {
        return Err(Error::invalid("/instrument", "Artificial observations need an instrument."));
    }
    if req.outputs.directory.is_none() && !req.outputs.export_formats.is_empty() {
        ctx.warn("EXPORTS_SKIPPED", "outputs.export_formats was given without outputs.directory; only artifacts were produced.");
    }
    let mut artifacts = Vec::new();
    let mut stages = serde_json::Map::new();
    let pop = build_population(ctx, req.population).map_err(|e| e.under("/population"))?;
    let pop_id = id_of(&pop, "population_artifact_id")?;
    stages.insert("population".into(), pop.result.clone());
    artifacts.extend(pop.artifacts);
    let src = build_source(
        ctx,
        SourceRequest {
            catalog: None,
            population: Some(ArtifactInput { artifact_id: pop_id.clone() }),
            uniform_surface_brightness: None,
            spatial_model: Some(req.source.spatial_model.clone()),
            geometry: req.source.geometry.clone(),
            offset_pixels: req.source.offset_pixels,
            outside: req.source.outside,
            invalid: InvalidPolicy::Error,
            label_type: req.source.label_type.clone(),
            smooth_light: req.source.smooth_light,
        },
    )
    .map_err(|e| e.under("/source"))?;
    let src_id = id_of(&src, "source_artifact_id")?;
    stages.insert("source".into(), src.result.clone());
    artifacts.extend(src.artifacts);
    let psf_id = match req.psf.take() {
        Some(mut p) => {
            if p.pixel_scale_arcsec.is_none() {
                p.pixel_scale_arcsec = req.source.geometry.pixel_scale_arcsec;
            }
            let o = build_psf(ctx, p).map_err(|e| e.under("/psf"))?;
            let id = id_of(&o, "psf_artifact_id")?;
            stages.insert("psf".into(), o.result.clone());
            artifacts.extend(o.artifacts);
            Some(ArtifactInput { artifact_id: id })
        }
        None => None,
    };
    let mut observations = Vec::new();
    for (i, obs) in req.observations.iter().enumerate() {
        let p = format!("/observations/{i}");
        let o = match req.render.kind {
            RenderKind::Ideal => ideal(
                ctx,
                IdealRequest {
                    source: ArtifactInput { artifact_id: src_id.clone() },
                    bandpass: obs.bandpass.clone(),
                    psf: psf_id.clone(),
                    output_zpt: req.render.output_zpt,
                    boundary: req.render.boundary,
                    convolution_method: Method::Auto,
                    histogram_convention: None,
                    selection: Selection::All,
                    include_smooth: true,
                },
            ),
            RenderKind::Artificial => artificial(
                ctx,
                ArtificialRequest {
                    source: ArtifactInput { artifact_id: src_id.clone() },
                    instrument: req.instrument.clone().unwrap(),
                    bandpass: obs.bandpass.clone(),
                    exposure_s: obs.exposure_s.ok_or_else(|| Error::invalid(&format!("{p}/exposure_s"), "Artificial observations need exposure_s."))?,
                    sky_sb_mag_arcsec2: obs.sky_sb_mag_arcsec2,
                    psf: psf_id.clone(),
                    output_zpt: req.render.output_zpt,
                    boundary: req.render.boundary,
                    convolution_method: Method::Auto,
                    histogram_convention: None,
                    selection: Selection::All,
                    include_smooth: true,
                    include_observed_variance: false,
                    include_magnitude_errors: false,
                },
            ),
        }
        .map_err(|e| e.under(&p))?;
        let oid = id_of(&o, "observation_artifact_id")?;
        let mut entry = json!({ "bandpass": obs.bandpass, "observation_artifact_id": oid, "summary": o.result });
        artifacts.extend(o.artifacts);
        if let Some(dir) = &req.outputs.directory {
            let mut files = Vec::new();
            for f in &req.outputs.export_formats {
                match f {
                    ExportKind::Fits => {
                        let mut planes = vec!["image".to_string()];
                        if req.render.kind == RenderKind::Artificial {
                            planes.extend(["source_counts".to_string(), "raw_counts".to_string()]);
                            if req.outputs.include_variance {
                                planes.extend(["expected_variance_counts".to_string(), "expected_variance_calibrated".to_string()]);
                            }
                        }
                        for plane in planes {
                            let dest = format!("{dir}/{}_{plane}.fits", obs.bandpass);
                            export(ctx, ExportRequest { artifact_id: oid.clone(), destination: dest.clone(), format: ExportFormat::Fits, plane: Some(plane), columns: vec![], magnitude_frame: Default::default(), overwrite: req.outputs.overwrite })?;
                            files.push(dest);
                        }
                    }
                    ExportKind::Png => {
                        let pv = preview(ctx, PreviewRequest { image: ImageRef { artifact_id: oid.clone(), plane: None }, percentile_low: 0.1, percentile_high: 99.9, stretch: artpop_io::preview::Stretch::Linear, colormap: artpop_io::preview::Colormap::GrayR, origin: artpop_io::preview::Origin::Lower, max_size: 1024 })?;
                        let pid = id_of(&pv, "preview_artifact_id")?;
                        artifacts.extend(pv.artifacts);
                        let dest = format!("{dir}/{}_preview.png", obs.bandpass);
                        export(ctx, ExportRequest { artifact_id: pid, destination: dest.clone(), format: ExportFormat::Png, plane: None, columns: vec![], magnitude_frame: Default::default(), overwrite: req.outputs.overwrite })?;
                        files.push(dest);
                    }
                    ExportKind::Csv => {}
                }
            }
            entry["exports"] = json!(files);
        }
        observations.push(entry);
    }
    if let Some(dir) = &req.outputs.directory
        && req.outputs.export_formats.contains(&ExportKind::Csv) {
            let dest = format!("{dir}/population.csv");
            export(ctx, ExportRequest { artifact_id: pop_id.clone(), destination: dest.clone(), format: ExportFormat::Csv, plane: None, columns: vec![], magnitude_frame: Default::default(), overwrite: req.outputs.overwrite })?;
            stages.insert("population_csv".into(), json!(dest));
        }
    stages.insert("observations".into(), json!(observations));
    let mut out = OpOutput::new(json!({ "population_artifact_id": pop_id, "source_artifact_id": src_id, "psf_artifact_id": psf_id.map(|p| p.artifact_id), "stages": stages }));
    out.artifacts = artifacts;
    Ok(out)
}

