//! Artifact listing, inspection, import, export, and aliasing.

use crate::codec::{image_member, load_population, load_positions, load_source, positions_bundle, read_image};
use crate::ops::population::{MagFrame, population_column};
use crate::store::{Manifest, Member, Store, sha256_hex};
use crate::types::{Geometry, Page};
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::spatial::Positions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub page: Page,
}

fn brief(store: &Store, m: &Manifest) -> Value {
    let r = store.reference(m);
    json!({ "artifact_id": m.artifact_id, "kind": m.kind, "created_by": m.created_by, "bytes": r.bytes, "members": m.members.len(), "uri": r.uri })
}

pub fn list(ctx: &Ctx, req: ListRequest) -> Result<OpOutput> {
    let mut all = ctx.store().list()?;
    if let Some(k) = &req.kind {
        all.retain(|m| &m.kind == k);
    }
    let (start, limit) = req.page.resolve(ctx.config().limits.max_page_rows)?;
    let end = (start + limit).min(all.len());
    let items: Vec<Value> = all[start.min(end)..end].iter().map(|m| brief(ctx.store(), m)).collect();
    Ok(OpOutput::new(json!({ "artifacts": items, "total": all.len(), "next_cursor": if end < all.len() { json!(format!("row:{end}")) } else { Value::Null }, "retention": "kept until explicitly deleted from the artifact root" })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub artifact_id: String,
}

pub fn inspect(ctx: &Ctx, req: InspectRequest) -> Result<OpOutput> {
    let l = ctx.store().load(&req.artifact_id, "/artifact_id", &[])?;
    let r = ctx.store().reference(&l.manifest);
    Ok(OpOutput::new(json!({ "manifest": l.manifest, "reference": r })))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// FITS image -> `image` artifact (header cards retained).
    Fits,
    /// CSV x/y -> `positions` artifact.
    CsvPositions,
    /// An exported `artpop-bundle-v1` directory.
    Bundle,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub format: ImportFormat,
    pub path: String,
    #[serde(default)]
    pub hdu: Option<usize>,
    #[serde(default)]
    pub extname: Option<String>,
    /// Geometry for positions.
    #[serde(default)]
    pub geometry: Option<Geometry>,
    #[serde(default)]
    pub x_column: Option<String>,
    #[serde(default)]
    pub y_column: Option<String>,
}

pub fn import(ctx: &Ctx, req: ImportRequest) -> Result<OpOutput> {
    let path = ctx.read_path(&req.path, "/path")?;
    match req.format {
        ImportFormat::Fits => {
            let f = artpop_io::fits::read_image(&path, req.hdu, req.extname.as_deref()).map_err(|e| e.at("/path"))?;
            let bunit = f.cards.iter().find(|c| c.starts_with("BUNIT")).and_then(|c| c.split('\'').nth(1)).map(|s| s.trim().to_string()).unwrap_or_else(|| "unknown".into());
            let finite = f.image.data.iter().filter(|v| v.is_finite()).count();
            if finite < f.image.data.len() {
                ctx.warn("NONFINITE_PIXELS", format!("{} pixels are not finite (e.g. BLANK).", f.image.data.len() - finite));
            }
            let meta = json!({ "width": f.image.width, "height": f.image.height, "unit": bunit, "fits_cards": f.cards, "source": { "path": path, "hdu": f.hdu_index, "image_type": f.image_type } });
            let r = ctx.put("image", meta, vec![image_member("image", &f.image, &bunit)], vec![])?;
            Ok(OpOutput::new(json!({ "image_artifact_id": r.artifact_id, "width": f.image.width, "height": f.image.height, "hdu": f.hdu_index, "header_cards": f.cards.len() })).with(r))
        }
        ImportFormat::CsvPositions => {
            let g = req.geometry.clone().ok_or_else(|| Error::invalid("/geometry", "Positions need geometry."))?;
            let text = ctx.read_text(&req.path, "/path")?;
            let t = artpop_io::csv::parse(&text, None)?;
            let x = t.require(req.x_column.as_deref().unwrap_or("x"), "/x_column")?.to_vec();
            let y = t.require(req.y_column.as_deref().unwrap_or("y"), "/y_column")?.to_vec();
            let inside: Vec<bool> = x.iter().zip(&y).map(|(a, b)| g.coordinates.inside(*a, *b, g.width, g.height)).collect();
            let n = x.len();
            let p = Positions { x, y, inside, index: (0..n as u64).collect(), generated: n as u64 };
            let outside = p.outside_count();
            let (meta, members) = positions_bundle(p, &g, json!({ "kind": "imported", "path": path }));
            let r = ctx.put("positions", meta, members, vec![])?;
            Ok(OpOutput::new(json!({ "positions_artifact_id": r.artifact_id, "rows": n, "outside": outside })).with(r))
        }
        ImportFormat::Bundle => {
            let mtext = std::fs::read(path.join("manifest.json")).map_err(|e| Error::new(ErrorCode::DataMissing, format!("No manifest.json in {}: {e}", path.display())))?;
            let m: Manifest = serde_json::from_slice(&mtext).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Invalid manifest: {e}")))?;
            let mut members = Vec::new();
            for info in &m.members {
                if info.file.contains('/') || info.file.contains('\\') || info.file.contains("..") {
                    return Err(Error::new(ErrorCode::DataCorrupt, "Bundle member path escapes the bundle."));
                }
                let b = std::fs::read(path.join(&info.file)).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Missing member {}: {e}", info.file)))?;
                if b.len() as u64 != info.bytes || sha256_hex(&b) != info.sha256 {
                    return Err(Error::new(ErrorCode::DataCorrupt, format!("Member {} failed its digest check.", info.name)));
                }
                members.push(Member { name: info.name.clone(), dtype: info.dtype, shape: info.shape.clone(), unit: info.unit.clone(), media_type: info.media_type.clone(), bytes: b.into() });
            }
            let kind: &'static str = match m.kind.as_str() {
                "isochrone" => "isochrone",
                "population" => "population",
                "source" => "source",
                "psf" => "psf",
                "image" => "image",
                "observation" => "observation",
                "spatial_grid" => "spatial_grid",
                "positions" => "positions",
                "masses" => "masses",
                "filters" => "filters",
                "mask" => "mask",
                "preview" => "preview",
                "plot" => "plot",
                other => return Err(Error::new(ErrorCode::UnsupportedFormat, format!("Unknown artifact kind '{other}'."))),
            };
            let (r, stored) = ctx.store().put(crate::store::Bundle { kind, created_by: m.created_by.clone(), inputs: m.inputs.clone(), metadata: m.metadata.clone(), members })?;
            let same = stored.artifact_id == m.artifact_id;
            if !same {
                ctx.warn("ID_RECOMPUTED", format!("Bundle declared {} but its content hashes to {} under this engine version.", m.artifact_id, stored.artifact_id));
            }
            Ok(OpOutput::new(json!({ "artifact_id": r.artifact_id, "kind": kind, "declared_id_matches": same })).with(r))
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Fits,
    Csv,
    Json,
    Png,
    Svg,
    Bundle,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub artifact_id: String,
    pub destination: String,
    pub format: ExportFormat,
    /// Image plane for FITS (observation planes; default `image`).
    #[serde(default)]
    pub plane: Option<String>,
    /// Catalog columns for CSV.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub magnitude_frame: MagFrame,
    #[serde(default)]
    pub overwrite: bool,
}

/// Write `bytes` to `dest` atomically (temp file + rename).
pub fn write_atomic(dest: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if dest.exists() && !overwrite {
        return Err(Error::invalid("/destination", format!("{} exists; set overwrite to replace it.", dest.display())));
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| Error::io(format!("Cannot create {}: {e}", p.display())))?;
    }
    let tmp = tmp_path(dest);
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(format!("Cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("Cannot publish {}: {e}", dest.display()))
    })
}

fn tmp_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    dest.with_file_name(format!(".{name}.tmp-{}", uuid::Uuid::new_v4().simple()))
}

fn fits_key_str(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii() && !c.is_control()).take(68).collect()
}

pub fn export(ctx: &Ctx, req: ExportRequest) -> Result<OpOutput> {
    let l = ctx.store().load(&req.artifact_id, "/artifact_id", &[])?;
    let dest = ctx.read_path(&req.destination, "/destination")?;
    let kind = l.manifest.kind.clone();
    let meta = &l.manifest.metadata;
    let written: Value = match req.format {
        ExportFormat::Fits => {
            let member = match kind.as_str() {
                "observation" => req.plane.clone().unwrap_or_else(|| "image".into()),
                "image" => "image".into(),
                "psf" => "kernel".into(),
                "spatial_grid" => "weights".into(),
                _ => return Err(Error::new(ErrorCode::UnsupportedFormat, format!("{kind} artifacts cannot be exported as FITS images.")).at("/format")),
            };
            if !l.has(&member) {
                return Err(Error::invalid("/plane", format!("No plane '{member}'.")).with_details(json!({ "available": l.manifest.members.iter().map(|m| &m.name).collect::<Vec<_>>() })));
            }
            let img = read_image(&l, &member)?;
            let unit = l.info(&member)?.unit.clone().unwrap_or_default();
            use artpop_io::fits::Key;
            let mut keys: Vec<(&str, Key, &str)> = vec![
                ("BUNIT", Key::Str(fits_key_str(&unit)), "pixel unit"),
                ("PLANE", Key::Str(fits_key_str(&member)), "artpop-rust plane"),
                ("ARTPOPID", Key::Str(req.artifact_id.trim_start_matches("sha256:").to_string()), "artifact SHA-256"),
                ("ARTPOPV", Key::Str(format!("artpop-rust {}", artpop_core::ENGINE_VERSION)), "engine"),
                ("ALGPROF", Key::Str(artpop_core::ALGORITHM_PROFILE.into()), "algorithm profile"),
            ];
            if let Some(b) = meta.get("bandpass").and_then(Value::as_str) {
                keys.push(("BANDPASS", Key::Str(fits_key_str(b)), "filter"));
            }
            for (k, field, c) in [("ZPT", "zpt", "magnitude zero point"), ("EXPTIME", "exposure_s", "exposure [s]"), ("SKYCNTS", "sky_counts", "sky counts per pixel"), ("CALIB", "calibration", "flux per count"), ("RDNOISE", "read_noise", "read noise [e-]")] {
                if let Some(v) = meta.get(field).and_then(Value::as_f64) {
                    keys.push((k, Key::F64(v), c));
                }
            }
            if let Some(c) = meta.pointer("/geometry/coordinates").and_then(Value::as_str) {
                keys.push(("PIXCONV", Key::Str(c.into()), "pixel coordinate convention"));
            }
            let cards: Vec<String> = meta.get("fits_cards").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
            if dest.exists() && !req.overwrite {
                return Err(Error::invalid("/destination", format!("{} exists; set overwrite to replace it.", dest.display())));
            }
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).map_err(|e| Error::io(e.to_string()))?;
            }
            let tmp = tmp_path(&dest);
            artpop_io::fits::write_image(&tmp, &img, &keys, &cards)?;
            std::fs::rename(&tmp, &dest).map_err(|e| Error::io(format!("Cannot publish {}: {e}", dest.display())))?;
            json!({ "plane": member, "width": img.width, "height": img.height })
        }
        ExportFormat::Csv => {
            let (names, cols): (Vec<String>, Vec<Vec<Value>>) = match kind.as_str() {
                "population" => {
                    let p = load_population(ctx, &req.artifact_id, "/artifact_id")?;
                    let mut names = req.columns.clone();
                    if names.is_empty() {
                        names = vec!["star_id".into(), "component".into(), "initial_mass".into(), "current_mass".into()];
                        names.extend(p.filters.iter().cloned());
                    }
                    let cols = names.iter().enumerate().map(|(i, n)| population_column(&p, n, req.magnitude_frame, &format!("/columns/{i}"))).collect::<Result<_>>()?;
                    (names, cols)
                }
                "source" => {
                    let s = load_source(ctx, &req.artifact_id, "/artifact_id")?;
                    let mut names = req.columns.clone();
                    if names.is_empty() {
                        names = vec!["x".into(), "y".into(), "inside".into()];
                        names.extend(s.bands.iter().cloned());
                    }
                    let cols = names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| -> Result<Vec<Value>> {
                            Ok(match n.as_str() {
                                "x" => s.x.iter().map(|v| json!(v)).collect(),
                                "y" => s.y.iter().map(|v| json!(v)).collect(),
                                "inside" => s.inside.iter().map(|v| json!(*v as u8)).collect(),
                                "component" => s.component.iter().map(|v| json!(v)).collect(),
                                "star_index" => s.star_index.iter().map(|v| json!(v)).collect(),
                                "label" => {
                                    let (lab, v) = s.labels.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This source has no labels.").at(&format!("/columns/{i}")))?;
                                    v.iter().map(|x| crate::codec::label_json(lab, *x)).collect()
                                }
                                band => s.mags[s.band_index(band).map_err(|e| e.at(&format!("/columns/{i}")))?].iter().map(|v| json!(v)).collect(),
                            })
                        })
                        .collect::<Result<_>>()?;
                    (names, cols)
                }
                "positions" => {
                    let (p, _) = load_positions(ctx, &req.artifact_id, "/artifact_id")?;
                    let names = if req.columns.is_empty() { vec!["x".into(), "y".into()] } else { req.columns.clone() };
                    let cols = names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| -> Result<Vec<Value>> {
                            Ok(match n.as_str() {
                                "x" => p.x.iter().map(|v| json!(v)).collect(),
                                "y" => p.y.iter().map(|v| json!(v)).collect(),
                                "inside" => p.inside.iter().map(|v| json!(*v as u8)).collect(),
                                "index" => p.index.iter().map(|v| json!(v)).collect(),
                                other => return Err(Error::invalid(&format!("/columns/{i}"), format!("Unknown positions column '{other}'.")).with_details(json!({ "available": ["x", "y", "inside", "index"] }))),
                            })
                        })
                        .collect::<Result<_>>()?;
                    (names, cols)
                }
                "masses" => (vec!["initial_mass".into()], vec![l.f64s("initial_mass")?.iter().map(|v| json!(v)).collect()]),
                "isochrone" => {
                    let iso = crate::codec::load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
                    let names = if req.columns.is_empty() { iso.column_names() } else { req.columns.clone() };
                    let cols = names.iter().enumerate().map(|(i, n)| iso.column(n).map(|c| c.iter().map(|v| json!(v)).collect()).ok_or_else(|| Error::invalid(&format!("/columns/{i}"), format!("Unknown column '{n}'.")))).collect::<Result<_>>()?;
                    (names, cols)
                }
                _ => return Err(Error::new(ErrorCode::UnsupportedFormat, format!("{kind} artifacts have no CSV form.")).at("/format")),
            };
            let rows = cols.first().map_or(0, |c| c.len());
            let mut text = String::with_capacity(rows * names.len() * 20 + 64);
            text.push_str(&names.join(","));
            text.push('\n');
            let mut buf = artpop_io::csv::RyuBuffer::new();
            for r in 0..rows {
                for (c, col) in cols.iter().enumerate() {
                    if c > 0 {
                        text.push(',');
                    }
                    match &col[r] {
                        Value::Number(n) => match n.as_f64() {
                            Some(f) if n.is_f64() => text.push_str(artpop_io::csv::fmt_f64(f, &mut buf)),
                            _ => text.push_str(&n.to_string()),
                        },
                        Value::String(s) => text.push_str(s),
                        Value::Bool(b) => text.push_str(if *b { "1" } else { "0" }),
                        Value::Null => text.push_str("nan"),
                        other => text.push_str(&other.to_string()),
                    }
                }
                text.push('\n');
            }
            write_atomic(&dest, text.as_bytes(), req.overwrite)?;
            json!({ "rows": rows, "columns": names })
        }
        ExportFormat::Json => {
            write_atomic(&dest, &serde_json::to_vec_pretty(&l.manifest).unwrap(), req.overwrite)?;
            json!({ "content": "manifest" })
        }
        ExportFormat::Png | ExportFormat::Svg => {
            let (member, want) = if req.format == ExportFormat::Png { ("png", "preview") } else { ("svg", "plot") };
            if !l.has(member) {
                return Err(Error::new(ErrorCode::UnsupportedFormat, format!("Only {want} artifacts export as {member}; create one with image.preview / plot.render.")).at("/format"));
            }
            write_atomic(&dest, &l.bytes(member)?, req.overwrite)?;
            json!({ "member": member })
        }
        ExportFormat::Bundle => {
            if dest.exists() && !req.overwrite {
                return Err(Error::invalid("/destination", format!("{} exists; set overwrite to replace it.", dest.display())));
            }
            let staging = tmp_path(&dest);
            std::fs::create_dir_all(&staging).map_err(|e| Error::io(e.to_string()))?;
            std::fs::write(staging.join("manifest.json"), serde_json::to_vec_pretty(&l.manifest).unwrap()).map_err(|e| Error::io(e.to_string()))?;
            for m in &l.manifest.members {
                std::fs::write(staging.join(&m.file), l.bytes(&m.name)?).map_err(|e| Error::io(e.to_string()))?;
            }
            if dest.exists() {
                std::fs::remove_dir_all(&dest).map_err(|e| Error::io(e.to_string()))?;
            }
            std::fs::rename(&staging, &dest).map_err(|e| Error::io(e.to_string()))?;
            json!({ "members": l.manifest.members.len() })
        }
    };
    let bytes = file_or_dir_bytes(&dest);
    Ok(OpOutput::new(json!({ "artifact_id": req.artifact_id, "destination": dest, "format": req.format, "bytes": bytes, "sha256": if dest.is_file() { json!(sha256_hex(&std::fs::read(&dest).unwrap_or_default())) } else { Value::Null }, "details": written })))
}

fn file_or_dir_bytes(p: &Path) -> u64 {
    if p.is_file() {
        return p.metadata().map(|m| m.len()).unwrap_or(0);
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneRequest {
    pub artifact_id: String,
    /// Optional physical copy as a portable bundle directory.
    #[serde(default)]
    pub export_destination: Option<String>,
}

pub fn clone(ctx: &Ctx, req: CloneRequest) -> Result<OpOutput> {
    let l = ctx.store().load(&req.artifact_id, "/artifact_id", &[])?;
    let r = ctx.store().reference(&l.manifest);
    let mut res = json!({ "artifact_id": req.artifact_id, "alias": true, "note": "Artifacts are immutable and content-addressed; the alias is the same ID." });
    if let Some(d) = &req.export_destination {
        let out = export(ctx, ExportRequest { artifact_id: req.artifact_id.clone(), destination: d.clone(), format: ExportFormat::Bundle, plane: None, columns: vec![], magnitude_frame: MagFrame::Apparent, overwrite: false })?;
        res["export"] = out.result;
    }
    Ok(OpOutput::new(res).with(r))
}
