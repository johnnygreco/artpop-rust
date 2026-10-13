//! Artifact listing, inspection, import, export, and aliasing.

use crate::codec::{LazyPopulation, SourceLoad, image_member, load_positions, load_source_with, positions_bundle};
use crate::ops::population::MagFrame;
use artpop_io::csv::Column;
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
            let p = Positions { x, y, inside, index: None, generated: n as u64 };
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

/// Sink for [`write_atomic_streaming`]: buffered file writes plus a running digest.
pub struct AtomicSink {
    file: std::io::BufWriter<std::fs::File>,
    hasher: sha2::Sha256,
    path: PathBuf,
}

impl AtomicSink {
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        use sha2::Digest;
        use std::io::Write;
        let (written, ()) = rayon::join(|| self.file.write_all(bytes), || self.hasher.update(bytes));
        written.map_err(|e| Error::io(format!("Cannot write {}: {e}", self.path.display())))
    }
}

/// Write a file atomically from pieces produced by `fill` (temp file + rename),
/// returning the SHA-256 hex digest of the written bytes.
pub fn write_atomic_streaming(dest: &Path, overwrite: bool, fill: impl FnOnce(&mut AtomicSink) -> Result<()>) -> Result<String> {
    use sha2::Digest;
    use std::io::Write;
    if dest.exists() && !overwrite {
        return Err(Error::invalid("/destination", format!("{} exists; set overwrite to replace it.", dest.display())));
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| Error::io(format!("Cannot create {}: {e}", p.display())))?;
    }
    let tmp = tmp_path(dest);
    let file = std::fs::File::create(&tmp).map_err(|e| Error::io(format!("Cannot write {}: {e}", tmp.display())))?;
    let mut sink = AtomicSink { file: std::io::BufWriter::with_capacity(1 << 20, file), hasher: sha2::Sha256::new(), path: tmp.clone() };
    let outcome = fill(&mut sink).and_then(|()| sink.file.flush().map_err(|e| Error::io(format!("Cannot write {}: {e}", tmp.display()))));
    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(sink.file);
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("Cannot publish {}: {e}", dest.display()))
    })?;
    let mut hex = String::with_capacity(64);
    for b in sink.hasher.finalize() {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(hex)
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
    // Digest of the bytes written, taken from memory instead of re-reading the file.
    let mut written_digest: Option<String> = None;
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
            let info = l.info(&member)?;
            if info.shape.len() != 2 {
                return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{member}' is not two-dimensional.")));
            }
            let (height, width) = (info.shape[0], info.shape[1]);
            let unit = info.unit.clone().unwrap_or_default();
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
            // The plane is streamed from the artifact into the FITS file: each
            // chunk is digest-verified, byte-swapped, hashed, and written while
            // the next one is read. The file is published only after the
            // member's digest has verified.
            let tmp = tmp_path(&dest);
            let streamed = (|| -> Result<String> {
                let mut writer = artpop_io::fits::ImageWriter::create(&tmp, width, height, &keys, &cards)?;
                l.stream_f64_bytes(&member, |chunk| writer.push_le_f64_bytes(chunk))?;
                writer.finish()
            })();
            let digest = streamed.inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })?;
            std::fs::rename(&tmp, &dest).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::io(format!("Cannot publish {}: {e}", dest.display()))
            })?;
            written_digest = Some(digest);
            json!({ "plane": member, "width": width, "height": height })
        }
        ExportFormat::Csv => {
            let (names, cols): (Vec<String>, Vec<Column<'_>>);
            let lazy_pop;
            let source;
            let positions;
            let masses;
            let iso;
            match kind.as_str() {
                "population" => {
                    lazy_pop = LazyPopulation::load(ctx, &req.artifact_id, "/artifact_id")?;
                    let mut n = req.columns.clone();
                    if n.is_empty() {
                        n = vec!["star_id".into(), "component".into(), "initial_mass".into(), "current_mass".into()];
                        n.extend(lazy_pop.header.filters.iter().cloned());
                    }
                    cols = n.iter().enumerate().map(|(i, c)| population_export_column(&lazy_pop, c, req.magnitude_frame, &format!("/columns/{i}"))).collect::<Result<_>>()?;
                    names = n;
                }
                "source" => {
                    let mut n = req.columns.clone();
                    let load = SourceLoad {
                        bands: if n.is_empty() { None } else { Some(n.clone()) },
                        component: n.is_empty() || n.iter().any(|c| c == "component"),
                        star_index: n.iter().any(|c| c == "star_index"),
                        labels: n.iter().any(|c| c == "label"),
                    };
                    source = load_source_with(ctx, &req.artifact_id, "/artifact_id", &load)?;
                    let s = &source;
                    if n.is_empty() {
                        n = vec!["x".into(), "y".into(), "inside".into()];
                        n.extend(s.bands.iter().cloned());
                    }
                    cols = n
                        .iter()
                        .enumerate()
                        .map(|(i, c)| -> Result<Column<'_>> {
                            Ok(match c.as_str() {
                                "x" => Column::f64s(&s.x[..]),
                                "y" => Column::f64s(&s.y[..]),
                                "inside" => Column::bools(&s.inside[..]),
                                "component" => Column::u32s(&s.component[..]),
                                "star_index" => Column::u64s(&s.star_index[..]),
                                "label" => {
                                    let (lab, v) = s.labels.as_ref().ok_or_else(|| Error::new(ErrorCode::DataMissing, "This source has no labels.").at(&format!("/columns/{i}")))?;
                                    match lab {
                                        crate::codec::Labels::Numeric { .. } => Column::f64s(&v[..]),
                                        crate::codec::Labels::Categorical { categories, .. } => Column::Str(v.iter().map(|x| categories.get(*x as usize).cloned().unwrap_or_else(|| "nan".into())).collect()),
                                    }
                                }
                                band => Column::f64s(s.band_column(band, &format!("/columns/{i}"))?),
                            })
                        })
                        .collect::<Result<_>>()?;
                    names = n;
                }
                "positions" => {
                    positions = load_positions(ctx, &req.artifact_id, "/artifact_id")?.0;
                    let p = &positions;
                    let n = if req.columns.is_empty() { vec!["x".to_string(), "y".to_string()] } else { req.columns.clone() };
                    cols = n
                        .iter()
                        .enumerate()
                        .map(|(i, c)| -> Result<Column<'_>> {
                            Ok(match c.as_str() {
                                "x" => Column::f64s(&p.x[..]),
                                "y" => Column::f64s(&p.y[..]),
                                "inside" => Column::bools(&p.inside[..]),
                                "index" => Column::u64s(p.index_column()),
                                other => return Err(Error::invalid(&format!("/columns/{i}"), format!("Unknown positions column '{other}'.")).with_details(json!({ "available": ["x", "y", "inside", "index"] }))),
                            })
                        })
                        .collect::<Result<_>>()?;
                    names = n;
                }
                "masses" => {
                    masses = l.f64s("initial_mass")?;
                    names = vec!["initial_mass".into()];
                    cols = vec![Column::f64s(&masses[..])];
                }
                "isochrone" => {
                    iso = crate::codec::load_isochrone(ctx, &req.artifact_id, "/artifact_id")?;
                    let n = if req.columns.is_empty() { iso.column_names() } else { req.columns.clone() };
                    cols = n.iter().enumerate().map(|(i, c)| iso.column(c).map(Column::f64s).ok_or_else(|| Error::invalid(&format!("/columns/{i}"), format!("Unknown column '{c}'.")))).collect::<Result<_>>()?;
                    names = n;
                }
                _ => return Err(Error::new(ErrorCode::UnsupportedFormat, format!("{kind} artifacts have no CSV form.")).at("/format")),
            }
            let rows = cols.first().map_or(0, |c| c.len());
            // Rows are formatted in waves and each wave is written and hashed
            // while the next one is formatted.
            let digest = write_atomic_streaming(&dest, req.overwrite, |out| artpop_io::csv::write_columns_to(&names, &cols, |piece| out.push(piece)))?;
            written_digest = Some(digest);
            json!({ "rows": rows, "columns": names })
        }
        ExportFormat::Json => {
            let bytes = serde_json::to_vec_pretty(&l.manifest).unwrap();
            write_atomic(&dest, &bytes, req.overwrite)?;
            written_digest = Some(sha256_hex(&bytes));
            json!({ "content": "manifest" })
        }
        ExportFormat::Png | ExportFormat::Svg => {
            let (member, want) = if req.format == ExportFormat::Png { ("png", "preview") } else { ("svg", "plot") };
            if !l.has(member) {
                return Err(Error::new(ErrorCode::UnsupportedFormat, format!("Only {want} artifacts export as {member}; create one with image.preview / plot.render.")).at("/format"));
            }
            let bytes = l.bytes(member)?;
            write_atomic(&dest, &bytes, req.overwrite)?;
            written_digest = Some(sha256_hex(&bytes));
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
    let sha = match written_digest {
        Some(d) if dest.is_file() => json!(d),
        _ if dest.is_file() => json!(sha256_hex(&std::fs::read(&dest).unwrap_or_default())),
        _ => Value::Null,
    };
    Ok(OpOutput::new(json!({ "artifact_id": req.artifact_id, "destination": dest, "format": req.format, "bytes": bytes, "sha256": sha, "details": written })))
}

/// One population column for CSV export, read on demand. Apparent magnitudes
/// add each star's component offset exactly as `Population::apparent_mags`.
fn population_export_column<'a>(p: &'a LazyPopulation, name: &str, frame: MagFrame, pointer: &str) -> Result<Column<'a>> {
    let h = &p.header;
    let n = p.len();
    let l = &p.loaded;
    Ok(match name {
        "star_id" => Column::u64s(if l.has("star_id") { l.u64s("star_id")? } else { (0..n as u64).collect::<Vec<u64>>() }),
        "component" => Column::u32s(p.component()?.unwrap_or_else(|| vec![0; n])),
        "initial_mass" => Column::f64s(l.f64s("initial_mass")?),
        "current_mass" | "star_mass" => Column::f64s(l.f64s("current_mass")?),
        "eep" | "log_L" | "log_Teff" => {
            let member = if name == "eep" { "eep" } else { name };
            if !l.has(member) {
                return Err(Error::new(ErrorCode::DataMissing, format!("This population has no {name} column.")).at(pointer));
            }
            Column::f64s(l.f64s(member)?)
        }
        "phase" => {
            let eep = p.eep()?.ok_or_else(|| Error::new(ErrorCode::DataMissing, "Phases need EEP values.").at(pointer))?;
            Column::Str(eep.iter().map(|e| artpop_core::population::phase_label(*e).to_string()).collect())
        }
        band => {
            let b = h.filter_index(band, pointer)?;
            let mut col = p.abs_mags(b)?;
            if frame == MagFrame::Apparent {
                let offsets: Vec<f64> = h.components.iter().map(|c| c.distance_modulus() + c.extinction[b]).collect();
                match p.component()? {
                    Some(comp) => {
                        for (m, c) in col.iter_mut().zip(&comp) {
                            *m += offsets[*c as usize];
                        }
                    }
                    None => {
                        for m in col.iter_mut() {
                            *m += offsets[0];
                        }
                    }
                }
            }
            Column::f64s(col)
        }
    })
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
