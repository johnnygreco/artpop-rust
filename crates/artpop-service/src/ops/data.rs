//! Model datasets: explicit registration, verification, and download.
//!
//! A dataset is usable only after a manifest exists in
//! `<data_dir>/manifests/<dataset_id>.json` (written by `data.import` or
//! `data.fetch`). A directory without a manifest is never a cache hit.
//! Files are re-hashed against the manifest whenever they are read.

use crate::store::sha256_hex;
use crate::{Ctx, OpOutput};
use artpop_core::error::{Error, ErrorCode, Result};
use artpop_core::mist;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Canonical MIST v1.2 archive location (the historical waps.cfa.harvard.edu
/// URL now redirects here; redirects are followed).
pub const DEFAULT_MIST_BASE_URL: &str = "https://mist.science/data/tarballs_v1.2";

/// Archive digests observed with `data.fetch` (dataset, SHA-256, bytes, date).
/// Used automatically when a request does not pin `expected_sha256`.
pub const PINNED_ARCHIVES: &[(&str, &str, u64, &str)] = &[("mist-v1.2-vvcrit0.4-LSST", "3d80c4ded7b7f9677984d48ac3fa379b2e959f05029967cc24c332bcb410f4a1", 85_410_956, "2026-10-12")];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub feh: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub schema_version: String,
    pub dataset_id: String,
    pub provider: String,
    pub version: String,
    pub rotation: f64,
    pub alpha_enhancement: f64,
    pub photometric_system: String,
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    pub missing_artpop_grid_feh: Vec<f64>,
    pub complete: bool,
    pub registered_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_bytes: Option<u64>,
    pub manifest_id: String,
}

pub fn dataset_id(rotation: f64, system: &str) -> String {
    format!("mist-v1.2-vvcrit{rotation:.1}-{system}")
}

fn data_dir(ctx: &Ctx, override_dir: &Option<String>, pointer: &str) -> Result<PathBuf> {
    if let Some(d) = override_dir {
        return ctx.read_path(d, pointer);
    }
    ctx.config().data_dir.clone().ok_or_else(|| {
        Error::new(ErrorCode::DataMissing, "No data directory is configured. Set ARTPOP_DATA_DIR, pass --data-dir, or give data_dir in the request.")
            .at(pointer)
    })
}

fn manifest_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("manifests").join(format!("{id}.json"))
}

pub fn registered(ctx: &Ctx) -> Result<Vec<Value>> {
    let Some(dir) = ctx.config().data_dir.clone() else { return Ok(vec![]) };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("manifests")) {
        for e in rd.flatten() {
            if let Ok(m) = std::fs::read(e.path()).map_err(|_| ()).and_then(|b| serde_json::from_slice::<DatasetManifest>(&b).map_err(|_| ())) {
                out.push(json!({ "dataset_id": m.dataset_id, "photometric_system": m.photometric_system, "rotation": m.rotation, "complete": m.complete, "files": m.files.len(), "root": m.root, "manifest_id": m.manifest_id }));
            }
        }
    }
    out.sort_by(|a, b| a["dataset_id"].as_str().cmp(&b["dataset_id"].as_str()));
    Ok(out)
}

pub fn load_manifest(ctx: &Ctx, data_dir_override: &Option<String>, rotation: f64, system: &str, pointer: &str) -> Result<DatasetManifest> {
    let id = dataset_id(rotation, system);
    let dir = data_dir(ctx, data_dir_override, pointer)?;
    let p = manifest_path(&dir, &id);
    let bytes = std::fs::read(&p).map_err(|_| {
        Error::new(ErrorCode::DataMissing, format!("MIST dataset {id} is not registered in {}.", dir.display()))
            .at(pointer)
            .with_details(json!({
                "dataset_id": id,
                "fetch_request": { "operation": "data.fetch", "request": { "schema_version": "1", "photometric_system": system, "rotation": rotation } },
                "import_request": { "operation": "data.import", "request": { "schema_version": "1", "path": format!("/path/to/{}", mist::grid_directory(rotation, system)) } },
            }))
    })?;
    serde_json::from_slice(&bytes).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Dataset manifest {} is malformed: {e}", p.display())))
}

/// Read one registered file, verifying size and digest.
pub fn read_verified(m: &DatasetManifest, name: &str) -> Result<String> {
    let entry = m.files.iter().find(|f| f.name == name).ok_or_else(|| {
        Error::new(ErrorCode::DataMissing, format!("{name} is not part of dataset {}.", m.dataset_id)).with_details(json!({ "available": m.files.iter().map(|f| &f.name).collect::<Vec<_>>() }))
    })?;
    let p = m.root.join(name);
    let bytes = std::fs::read(&p).map_err(|e| Error::new(ErrorCode::DataMissing, format!("Cannot read {}: {e}", p.display())))?;
    if bytes.len() as u64 != entry.bytes || sha256_hex(&bytes) != entry.sha256 {
        return Err(Error::new(ErrorCode::DataCorrupt, format!("{} does not match its registered size/digest; re-run data.verify or data.import.", p.display())));
    }
    String::from_utf8(bytes).map_err(|_| Error::new(ErrorCode::DataCorrupt, format!("{} is not UTF-8 text.", p.display())))
}

fn parse_dir_name(name: &str) -> Option<(f64, String)> {
    let rest = name.strip_prefix("MIST_v1.2_vvcrit")?;
    let (rot, system) = rest.split_once('_')?;
    Some((rot.parse().ok()?, system.to_string()))
}

/// Scan a MIST grid directory and build (but do not write) its manifest.
fn scan(root: &Path, registered_by: &str) -> Result<DatasetManifest> {
    let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let (rotation, system) = parse_dir_name(name).ok_or_else(|| {
        Error::invalid("/path", format!("'{name}' is not a MIST v1.2 grid directory (expected MIST_v1.2_vvcrit<rot>_<SYSTEM>)."))
    })?;
    let catalog = artpop_core::filters::catalog();
    let system = catalog.canonical_system(&system).map(String::from).ok_or_else(|| Error::invalid("/path", format!("Unknown photometric system '{system}'.")).with_details(json!({ "available": catalog.systems })))?;
    if !mist::ROTATIONS.contains(&rotation) {
        return Err(Error::invalid("/path", "MIST rotation must be 0.0 or 0.4."));
    }
    let mut files = Vec::new();
    let mut rd: Vec<_> = std::fs::read_dir(root).map_err(|e| Error::io(format!("Cannot list {}: {e}", root.display())))?.flatten().collect();
    rd.sort_by_key(|e| e.file_name());
    for e in rd {
        let fname = e.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".iso.cmd") {
            continue;
        }
        let bytes = std::fs::read(e.path()).map_err(|err| Error::io(format!("Cannot read {}: {err}", e.path().display())))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::new(ErrorCode::DataCorrupt, format!("{fname} is not UTF-8.")))?;
        let h = mist::parse_header(text).map_err(|err| Error::new(ErrorCode::DataCorrupt, format!("{fname}: {}", err.message)))?;
        if (h.v_vcrit - rotation).abs() > 1e-6 || !h.photometric_system.starts_with(&system) && !h.photometric_system.to_lowercase().contains(&system.to_lowercase()) {
            return Err(Error::new(ErrorCode::ModelAlignment, format!("{fname} header ({} v/vcrit={}) does not match the directory.", h.photometric_system, h.v_vcrit)));
        }
        files.push(FileEntry { name: fname, bytes: bytes.len() as u64, sha256: sha256_hex(&bytes), feh: h.feh });
    }
    if files.is_empty() {
        return Err(Error::new(ErrorCode::DataMissing, format!("{} contains no .iso.cmd files.", root.display())));
    }
    let missing: Vec<f64> = mist::feh_grid().into_iter().filter(|f| !files.iter().any(|e| e.name == mist::file_name(*f, rotation, &system))).collect();
    let id = dataset_id(rotation, &system);
    let mut m = DatasetManifest {
        schema_version: "1".into(),
        dataset_id: id,
        provider: "MIST".into(),
        version: "1.2".into(),
        rotation,
        alpha_enhancement: 0.0,
        photometric_system: system,
        root: root.to_path_buf(),
        complete: missing.is_empty(),
        missing_artpop_grid_feh: missing,
        files,
        registered_by: registered_by.into(),
        source_url: None,
        archive_sha256: None,
        archive_bytes: None,
        manifest_id: String::new(),
    };
    m.manifest_id = content_id(&m);
    Ok(m)
}

fn content_id(m: &DatasetManifest) -> String {
    let files: Vec<Value> = m.files.iter().map(|f| json!([f.name, f.bytes, f.sha256])).collect();
    format!("sha256:{}", sha256_hex(crate::store::canonical_json(&json!({ "dataset_id": m.dataset_id, "files": files })).as_bytes()))
}

fn write_manifest(dir: &Path, m: &DatasetManifest) -> Result<PathBuf> {
    let p = manifest_path(dir, &m.dataset_id);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| Error::io(e.to_string()))?;
    let tmp = p.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(m).unwrap()).map_err(|e| Error::io(e.to_string()))?;
    std::fs::rename(&tmp, &p).map_err(|e| Error::io(e.to_string()))?;
    Ok(p)
}

fn summary(m: &DatasetManifest) -> Value {
    json!({
        "dataset_id": m.dataset_id,
        "manifest_id": m.manifest_id,
        "photometric_system": m.photometric_system,
        "rotation": m.rotation,
        "root": m.root,
        "files": m.files.len(),
        "bytes": m.files.iter().map(|f| f.bytes).sum::<u64>(),
        "complete": m.complete,
        "missing_artpop_grid_feh": m.missing_artpop_grid_feh,
    })
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    #[serde(default)]
    pub photometric_system: Option<String>,
}

pub fn list(ctx: &Ctx, req: ListRequest) -> Result<OpOutput> {
    let mut sets = registered(ctx)?;
    if let Some(s) = &req.photometric_system {
        sets.retain(|d| d["photometric_system"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(s)));
    }
    let catalog = artpop_core::filters::catalog();
    Ok(OpOutput::new(json!({
        "data_dir": ctx.config().data_dir,
        "registered": sets,
        "available_remote": catalog.systems.iter().flat_map(|s| mist::ROTATIONS.iter().map(move |r| json!({ "dataset_id": dataset_id(*r, s), "photometric_system": s, "rotation": r }))).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// Existing `MIST_v1.2_vvcrit<rot>_<SYSTEM>` directory of `.iso.cmd` files.
    pub path: String,
    /// Data directory receiving the manifest (default: service data dir).
    #[serde(default)]
    pub data_dir: Option<String>,
}

pub fn import(ctx: &Ctx, req: ImportRequest) -> Result<OpOutput> {
    let root = ctx.read_path(&req.path, "/path")?;
    if !root.is_dir() {
        return Err(Error::new(ErrorCode::DataMissing, format!("{} is not a directory.", root.display())).at("/path"));
    }
    let dir = data_dir(ctx, &req.data_dir, "/data_dir")?;
    let m = scan(&root, "data.import")?;
    if !m.complete {
        ctx.warn("INCOMPLETE_GRID", format!("Missing ArtPop grid metallicities: {:?}; isochrone.mist will fail for [Fe/H] needing them.", m.missing_artpop_grid_feh));
    }
    let p = write_manifest(&dir, &m)?;
    ctx.note_data(m.manifest_id.clone());
    let mut out = summary(&m);
    out["manifest_path"] = json!(p);
    Ok(OpOutput::new(out))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    pub dataset_id: String,
    #[serde(default)]
    pub data_dir: Option<String>,
}

pub fn verify(ctx: &Ctx, req: VerifyRequest) -> Result<OpOutput> {
    let dir = data_dir(ctx, &req.data_dir, "/data_dir")?;
    let p = manifest_path(&dir, &req.dataset_id);
    let m: DatasetManifest = serde_json::from_slice(&std::fs::read(&p).map_err(|_| Error::new(ErrorCode::DataMissing, format!("Dataset {} is not registered.", req.dataset_id)).at("/dataset_id"))?)
        .map_err(|e| Error::new(ErrorCode::DataCorrupt, e.to_string()))?;
    let mut missing = Vec::new();
    let mut corrupt = Vec::new();
    for f in &m.files {
        match std::fs::read(m.root.join(&f.name)) {
            Err(_) => missing.push(f.name.clone()),
            Ok(b) => {
                if b.len() as u64 != f.bytes || sha256_hex(&b) != f.sha256 {
                    corrupt.push(f.name.clone());
                }
            }
        }
    }
    ctx.note_data(m.manifest_id.clone());
    Ok(OpOutput::new(json!({ "dataset_id": m.dataset_id, "verified": missing.is_empty() && corrupt.is_empty(), "files_checked": m.files.len(), "missing": missing, "corrupt": corrupt })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchRequest {
    pub photometric_system: String,
    #[serde(default = "rot")]
    pub rotation: f64,
    /// Pinned archive SHA-256; when absent the computed digest is recorded and a warning issued.
    #[serde(default)]
    pub expected_sha256: Option<String>,
    /// Override the archive base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub data_dir: Option<String>,
    /// Maximum total extracted bytes (default 4 GiB).
    #[serde(default)]
    pub max_extracted_bytes: Option<u64>,
}

fn rot() -> f64 {
    0.4
}

pub fn fetch(ctx: &Ctx, req: FetchRequest) -> Result<OpOutput> {
    if !ctx.config().allow_network {
        return Err(Error::new(ErrorCode::NetworkError, "Network access is disabled for this service (start without --offline to enable data.fetch)."));
    }
    let catalog = artpop_core::filters::catalog();
    let system = catalog.canonical_system(&req.photometric_system).ok_or_else(|| Error::invalid("/photometric_system", "Unknown photometric system.").with_details(json!({ "available": catalog.systems })))?.to_string();
    if !mist::ROTATIONS.contains(&req.rotation) {
        return Err(Error::invalid("/rotation", "MIST rotation must be 0.0 or 0.4."));
    }
    let dir = data_dir(ctx, &req.data_dir, "/data_dir")?;
    let id = dataset_id(req.rotation, &system);
    std::fs::create_dir_all(dir.join("locks")).map_err(|e| Error::io(e.to_string()))?;
    let lock = dir.join("locks").join(format!("{id}.lock"));
    let _guard = LockFile::acquire(&lock)?;
    let base = req.base_url.clone().unwrap_or_else(|| DEFAULT_MIST_BASE_URL.to_string());
    let url = format!("{}/MIST_v1.2_vvcrit{:.1}_{system}.txz", base.trim_end_matches('/'), req.rotation);
    let staging = dir.join("staging").join(format!("{id}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&staging).map_err(|e| Error::io(e.to_string()))?;
    let cleanup = Cleanup(staging.clone());
    let archive = staging.join("archive.txz");
    let resp = ureq::get(&url).call().map_err(|e| Error::new(ErrorCode::NetworkError, format!("Download of {url} failed: {e}")).with_details(json!({ "url": url })))?;
    if resp.status() != 200 {
        return Err(Error::new(ErrorCode::NetworkError, format!("Download of {url} returned HTTP {}.", resp.status())));
    }
    let mut reader = resp.into_body().into_with_config().limit(2 * 1024 * 1024 * 1024).reader();
    let mut file = std::fs::File::create(&archive).map_err(|e| Error::io(e.to_string()))?;
    let mut hasher_bytes = 0u64;
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::new(ErrorCode::NetworkError, format!("Download interrupted: {e}")))?;
        if n == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buf[..n]);
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| Error::io(e.to_string()))?;
        hasher_bytes += n as u64;
    }
    drop(file);
    let digest: String = sha2::Digest::finalize(hasher).iter().map(|b| format!("{b:02x}")).collect();
    let pinned = PINNED_ARCHIVES.iter().find(|p| p.0 == id).map(|p| p.1.to_string());
    match req.expected_sha256.clone().or(pinned) {
        Some(expected) if !expected.eq_ignore_ascii_case(&digest) => {
            return Err(Error::new(ErrorCode::DataCorrupt, "Downloaded archive does not match its pinned SHA-256.").with_details(json!({ "expected": expected, "received": digest, "url": url })));
        }
        Some(_) => {}
        None => ctx.warn("UNPINNED_CHECKSUM", format!("No pinned digest exists for {id}; recorded archive digest {digest}. Pass expected_sha256 to verify.")),
    }
    let limit = req.max_extracted_bytes.unwrap_or(4 * 1024 * 1024 * 1024);
    let extract_root = staging.join("extract");
    extract_txz(&archive, &extract_root, limit)?;
    let grid_name = mist::grid_directory(req.rotation, &system);
    let extracted = extract_root.join(&grid_name);
    if !extracted.is_dir() {
        return Err(Error::new(ErrorCode::DataCorrupt, format!("Archive did not contain {grid_name}/.")));
    }
    let final_dir = dir.join(&grid_name);
    if final_dir.exists() {
        let old = dir.join("staging").join(format!("{grid_name}-replaced-{}", uuid::Uuid::new_v4().simple()));
        std::fs::rename(&final_dir, &old).map_err(|e| Error::io(e.to_string()))?;
    }
    std::fs::rename(&extracted, &final_dir).map_err(|e| Error::io(e.to_string()))?;
    let mut m = scan(&final_dir, "data.fetch")?;
    m.source_url = Some(url.clone());
    m.archive_sha256 = Some(digest.clone());
    m.archive_bytes = Some(hasher_bytes);
    let p = write_manifest(&dir, &m)?;
    drop(cleanup);
    ctx.note_data(m.manifest_id.clone());
    let mut out = summary(&m);
    out["manifest_path"] = json!(p);
    out["source_url"] = json!(url);
    out["archive_sha256"] = json!(digest);
    out["archive_bytes"] = json!(hasher_bytes);
    Ok(OpOutput::new(out))
}

/// Extract a `.tar.xz` with path, link, and size limits.
fn extract_txz(archive: &Path, dest: &Path, max_bytes: u64) -> Result<()> {
    let f = std::fs::File::open(archive).map_err(|e| Error::io(e.to_string()))?;
    let xz = lzma_rust2::XzReader::new(std::io::BufReader::new(f), true);
    let mut tar = tar::Archive::new(xz);
    let mut total = 0u64;
    let mut count = 0u64;
    for entry in tar.entries().map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Archive is not a valid tar.xz: {e}")))? {
        let mut entry = entry.map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Corrupt archive entry: {e}")))?;
        let path = entry.path().map_err(|e| Error::new(ErrorCode::DataCorrupt, e.to_string()))?.to_path_buf();
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Archive entry escapes the destination: {}", path.display())));
        }
        count += 1;
        if count > 100_000 {
            return Err(Error::new(ErrorCode::ResourceLimit, "Archive has too many entries."));
        }
        let kind = entry.header().entry_type();
        let out = dest.join(&path);
        if kind.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| Error::io(e.to_string()))?;
        } else if kind.is_file() {
            total += entry.header().size().unwrap_or(0);
            if total > max_bytes {
                return Err(Error::new(ErrorCode::ResourceLimit, "Archive expands beyond max_extracted_bytes.").with_details(json!({ "max_extracted_bytes": max_bytes })));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(e.to_string()))?;
            }
            let mut w = std::fs::File::create(&out).map_err(|e| Error::io(e.to_string()))?;
            std::io::copy(&mut entry, &mut w).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Extraction failed: {e}")))?;
        }
        // Links and special files are skipped deliberately.
    }
    Ok(())
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct LockFile(PathBuf);

impl LockFile {
    pub fn acquire(p: &Path) -> Result<LockFile> {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(p) {
            Ok(_) => Ok(LockFile(p.to_path_buf())),
            Err(_) => Err(Error::new(ErrorCode::IdempotencyConflict, format!("Another fetch holds {}; retry after it finishes.", p.display())).with_details(json!({ "lock": p }))),
        }
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
