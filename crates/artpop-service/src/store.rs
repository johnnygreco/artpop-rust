//! Content-addressed artifact store (portable bundle format v1).
//!
//! An artifact is a directory `objects/<sha256>/` holding `manifest.json`
//! and little-endian binary members. The artifact ID is the SHA-256 of the
//! canonical (sorted-key) JSON of its kind, format, algorithm profile,
//! metadata, inputs, producing operation, and member digests. Nothing
//! time-dependent enters the ID. Writes go to a staging directory that is
//! atomically renamed into place; identical content is deduplicated.

use artpop_core::error::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const BUNDLE_FORMAT: &str = "artpop-bundle-v1";
pub const BUNDLE_MEDIA_TYPE: &str = "application/vnd.artpop.bundle+json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    F64,
    U64,
    U32,
    U8,
    Bytes,
}

impl DType {
    pub fn size(self) -> usize {
        match self {
            DType::F64 | DType::U64 => 8,
            DType::U32 => 4,
            DType::U8 | DType::Bytes => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub name: String,
    pub dtype: DType,
    pub shape: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub bytes: u64,
    pub sha256: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: String,
    pub artifact_id: String,
    pub kind: String,
    pub format: String,
    pub algorithm_profile: String,
    pub engine_version: String,
    pub created_by: String,
    pub inputs: Vec<String>,
    pub metadata: Value,
    pub members: Vec<MemberInfo>,
}

pub struct Member {
    pub name: String,
    pub dtype: DType,
    pub shape: Vec<usize>,
    pub unit: Option<String>,
    pub media_type: Option<String>,
    pub bytes: Payload,
}

/// Member payload: raw little-endian bytes, or owned f64 values written
/// through a byte view (avoids copying large planes on little-endian hosts).
pub enum Payload {
    Bytes(Vec<u8>),
    F64(Vec<f64>),
}

impl Payload {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Payload::Bytes(b) => b,
            // SAFETY: f64 values viewed as bytes; only constructed on little-endian hosts.
            Payload::F64(v) => unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 8) },
        }
    }

    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl From<Vec<u8>> for Payload {
    fn from(b: Vec<u8>) -> Payload {
        Payload::Bytes(b)
    }
}

impl Member {
    pub fn f64(name: &str, values: &[f64], shape: Vec<usize>, unit: Option<&str>) -> Member {
        Member::f64_owned(name, values.to_vec(), shape, unit)
    }

    /// Take ownership of the values (no copy on little-endian hosts).
    pub fn f64_owned(name: &str, values: Vec<f64>, shape: Vec<usize>, unit: Option<&str>) -> Member {
        let bytes = if cfg!(target_endian = "little") { Payload::F64(values) } else { Payload::Bytes(values.iter().flat_map(|v| v.to_le_bytes()).collect()) };
        Member { name: name.into(), dtype: DType::F64, shape, unit: unit.map(String::from), media_type: None, bytes }
    }

    pub fn column(name: &str, values: &[f64], unit: Option<&str>) -> Member {
        Member::f64(name, values, vec![values.len()], unit)
    }

    /// A one-dimensional column that takes ownership of its values (no copy).
    pub fn column_owned(name: &str, values: Vec<f64>, unit: Option<&str>) -> Member {
        let n = values.len();
        Member::f64_owned(name, values, vec![n], unit)
    }

    pub fn u64(name: &str, values: &[u64]) -> Member {
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        Member { name: name.into(), dtype: DType::U64, shape: vec![values.len()], unit: None, media_type: None, bytes: bytes.into() }
    }

    pub fn u32(name: &str, values: &[u32]) -> Member {
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        Member { name: name.into(), dtype: DType::U32, shape: vec![values.len()], unit: None, media_type: None, bytes: bytes.into() }
    }

    pub fn bools(name: &str, values: &[bool]) -> Member {
        Member { name: name.into(), dtype: DType::U8, shape: vec![values.len()], unit: None, media_type: None, bytes: values.iter().map(|b| *b as u8).collect::<Vec<u8>>().into() }
    }

    pub fn file(name: &str, bytes: Vec<u8>, media_type: &str) -> Member {
        Member { name: name.into(), dtype: DType::Bytes, shape: vec![bytes.len()], unit: None, media_type: Some(media_type.into()), bytes: bytes.into() }
    }
}

pub struct Bundle {
    pub kind: &'static str,
    pub created_by: String,
    pub inputs: Vec<String>,
    pub metadata: Value,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub kind: String,
    pub uri: String,
    pub media_type: String,
    pub path: String,
    pub bytes: u64,
}

/// Canonical JSON: object keys sorted recursively, no whitespace.
pub fn canonical_json(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k).unwrap());
                    out.push(':');
                    walk(&m[*k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    walk(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).unwrap()),
        }
    }
    let mut s = String::new();
    walk(v, &mut s);
    s
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub struct Store {
    pub root: PathBuf,
}

pub struct Loaded {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

fn io_err(e: std::io::Error, what: &str, p: &Path) -> Error {
    Error::new(ErrorCode::IoError, format!("{what} {}: {e}", p.display()))
}

pub fn parse_artifact_id(id: &str, pointer: &str) -> Result<String> {
    let hex = id.strip_prefix("sha256:").unwrap_or("");
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        return Err(Error::invalid(pointer, "Artifact IDs have the form 'sha256:<64 lowercase hex digits>'.").with_details(serde_json::json!({ "received": id })));
    }
    Ok(hex.to_string())
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root }
    }

    fn objects(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn put(&self, bundle: Bundle) -> Result<(ArtifactRef, Manifest)> {
        use rayon::prelude::*;
        for m in &bundle.members {
            let expected: usize = m.shape.iter().product::<usize>() * m.dtype.size();
            if expected != m.bytes.len() {
                return Err(Error::internal(format!("Member {} has {} bytes; shape implies {expected}.", m.name, m.bytes.len())));
            }
        }
        // Members are independent: each is hashed and written to the staging
        // directory in one parallel pass, so file writes overlap with the
        // hashing of other members. Member file names depend only on the
        // position, so staging can start before the artifact ID is known; if
        // the content turns out to exist already, staging is discarded.
        let staging = self.root.join("staging").join(format!("{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&staging).map_err(|e| io_err(e, "Cannot create staging directory", &staging))?;
        let members: Vec<MemberInfo> = match bundle
            .members
            .par_iter()
            .enumerate()
            .map(|(i, m)| {
                let file = format!("m{i:03}.bin");
                let p = staging.join(&file);
                let (written, digest) = rayon::join(|| std::fs::write(&p, m.bytes.as_bytes()).map_err(|e| io_err(e, "Cannot write", &p)), || sha256_hex(m.bytes.as_bytes()));
                written?;
                Ok(MemberInfo {
                    name: m.name.clone(),
                    dtype: m.dtype,
                    shape: m.shape.clone(),
                    unit: m.unit.clone(),
                    media_type: m.media_type.clone(),
                    bytes: m.bytes.len() as u64,
                    sha256: digest,
                    file,
                })
            })
            .collect::<Result<Vec<MemberInfo>>>()
        {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let identity = serde_json::json!({
            "schema_version": "1",
            "kind": bundle.kind,
            "format": BUNDLE_FORMAT,
            "algorithm_profile": artpop_core::ALGORITHM_PROFILE,
            "engine_version": artpop_core::ENGINE_VERSION,
            "created_by": bundle.created_by,
            "inputs": bundle.inputs,
            "metadata": bundle.metadata,
            "members": members.iter().map(|m| serde_json::json!({ "name": m.name, "dtype": m.dtype, "shape": m.shape, "unit": m.unit, "sha256": m.sha256 })).collect::<Vec<_>>(),
        });
        let hex = sha256_hex(canonical_json(&identity).as_bytes());
        let id = format!("sha256:{hex}");
        let manifest = Manifest {
            schema_version: "1".into(),
            artifact_id: id.clone(),
            kind: bundle.kind.into(),
            format: BUNDLE_FORMAT.into(),
            algorithm_profile: artpop_core::ALGORITHM_PROFILE.into(),
            engine_version: artpop_core::ENGINE_VERSION.into(),
            created_by: bundle.created_by,
            inputs: bundle.inputs,
            metadata: bundle.metadata,
            members,
        };
        let dest = self.objects().join(&hex);
        if dest.join("manifest.json").is_file() {
            let _ = std::fs::remove_dir_all(&staging);
        } else {
            let mp = staging.join("manifest.json");
            if let Err(e) = std::fs::write(&mp, serde_json::to_vec_pretty(&manifest).unwrap()) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(io_err(e, "Cannot write", &mp));
            }
            std::fs::create_dir_all(self.objects()).map_err(|e| io_err(e, "Cannot create", &self.objects()))?;
            if let Err(e) = std::fs::rename(&staging, &dest) {
                // Another writer may have published identical content first.
                let _ = std::fs::remove_dir_all(&staging);
                if !dest.join("manifest.json").is_file() {
                    return Err(io_err(e, "Cannot publish artifact", &dest));
                }
            }
        }
        let bytes = manifest.members.iter().map(|m| m.bytes).sum();
        Ok((
            ArtifactRef {
                artifact_id: id.clone(),
                kind: manifest.kind.clone(),
                uri: format!("artpop://artifacts/{id}/manifest"),
                media_type: BUNDLE_MEDIA_TYPE.into(),
                path: dest.display().to_string(),
                bytes,
            },
            manifest,
        ))
    }

    pub fn load(&self, id: &str, pointer: &str, expected_kinds: &[&str]) -> Result<Loaded> {
        let hex = parse_artifact_id(id, pointer)?;
        let dir = self.objects().join(&hex);
        let mp = dir.join("manifest.json");
        let text = std::fs::read(&mp).map_err(|_| {
            Error::new(ErrorCode::ArtifactNotFound, format!("Artifact {id} is not present in the store at {}.", self.root.display()))
                .at(pointer)
                .with_details(serde_json::json!({ "artifact_id": id, "store": self.root.display().to_string() }))
        })?;
        let manifest: Manifest = serde_json::from_slice(&text).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Artifact manifest {id} is malformed: {e}")))?;
        if manifest.artifact_id != id {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Artifact manifest at {} declares a different ID.", mp.display())));
        }
        if !expected_kinds.is_empty() && !expected_kinds.contains(&manifest.kind.as_str()) {
            return Err(Error::invalid(pointer, format!("Artifact {id} is a '{}' artifact; expected {}.", manifest.kind, expected_kinds.join(" or ")))
                .with_details(serde_json::json!({ "received_kind": manifest.kind, "expected_kinds": expected_kinds })));
        }
        Ok(Loaded { manifest, dir })
    }

    pub fn list(&self) -> Result<Vec<Manifest>> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.objects()) else { return Ok(out) };
        for e in rd.flatten() {
            if let Ok(text) = std::fs::read(e.path().join("manifest.json"))
                && let Ok(m) = serde_json::from_slice::<Manifest>(&text) {
                    out.push(m);
                }
        }
        out.sort_by(|a, b| a.artifact_id.cmp(&b.artifact_id));
        Ok(out)
    }

    pub fn reference(&self, m: &Manifest) -> ArtifactRef {
        let hex = m.artifact_id.trim_start_matches("sha256:");
        ArtifactRef {
            artifact_id: m.artifact_id.clone(),
            kind: m.kind.clone(),
            uri: format!("artpop://artifacts/{}/manifest", m.artifact_id),
            media_type: BUNDLE_MEDIA_TYPE.into(),
            path: self.objects().join(hex).display().to_string(),
            bytes: m.members.iter().map(|x| x.bytes).sum(),
        }
    }
}

impl Loaded {
    pub fn has(&self, name: &str) -> bool {
        self.manifest.members.iter().any(|m| m.name == name)
    }

    pub fn info(&self, name: &str) -> Result<&MemberInfo> {
        self.manifest.members.iter().find(|m| m.name == name).ok_or_else(|| {
            Error::new(ErrorCode::DataCorrupt, format!("Artifact {} has no member '{name}'.", self.manifest.artifact_id))
                .with_details(serde_json::json!({ "available": self.manifest.members.iter().map(|m| &m.name).collect::<Vec<_>>() }))
        })
    }

    /// Read a member, verifying its length and digest.
    pub fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        let info = self.info(name)?;
        if info.file.contains('/') || info.file.contains('\\') || info.file.contains("..") {
            return Err(Error::new(ErrorCode::DataCorrupt, "Artifact member path escapes the bundle."));
        }
        let p = self.dir.join(&info.file);
        let b = std::fs::read(&p).map_err(|e| io_err(e, "Cannot read", &p))?;
        if b.len() as u64 != info.bytes || sha256_hex(&b) != info.sha256 {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{name}' of {} failed its length/digest check.", self.manifest.artifact_id)));
        }
        Ok(b)
    }

    fn typed(&self, name: &str, dtype: DType) -> Result<Vec<u8>> {
        let info = self.info(name)?;
        if info.dtype != dtype {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{name}' has dtype {:?}, expected {dtype:?}.", info.dtype)));
        }
        self.bytes(name)
    }

    pub fn f64s(&self, name: &str) -> Result<Vec<f64>> {
        let info = self.info(name)?;
        if info.dtype != DType::F64 {
            return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{name}' has dtype {:?}, expected F64.", info.dtype)));
        }
        if info.file.contains('/') || info.file.contains('\\') || info.file.contains("..") || info.bytes % 8 != 0 {
            return Err(Error::new(ErrorCode::DataCorrupt, "Artifact member is malformed."));
        }
        // Read little-endian bytes directly into an f64 buffer (no second copy).
        // The file is read in chunks and each chunk is hashed while the next
        // one is being read, so verification overlaps the I/O.
        let p = self.dir.join(&info.file);
        let mut out = artpop_core::mem::zeroed_f64((info.bytes / 8) as usize);
        {
            use std::io::Read;
            // SAFETY: f64 has no invalid bit patterns; the byte view covers exactly the buffer.
            let bytes = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, out.len() * 8) };
            let mut f = std::fs::File::open(&p).map_err(|e| io_err(e, "Cannot read", &p))?;
            const CHUNK: usize = 8 << 20;
            let mut hasher = Sha256::new();
            let mut rest: &mut [u8] = bytes;
            let mut pending: &[u8] = &[];
            while !rest.is_empty() {
                let n = rest.len().min(CHUNK);
                let (cur, tail) = std::mem::take(&mut rest).split_at_mut(n);
                let (read, ()) = rayon::join(|| f.read_exact(cur), || hasher.update(pending));
                read.map_err(|e| io_err(e, "Cannot read", &p))?;
                pending = cur;
                rest = tail;
            }
            hasher.update(pending);
            let digest = hasher.finalize();
            let mut hex = String::with_capacity(64);
            for b in digest {
                hex.push_str(&format!("{b:02x}"));
            }
            let mut extra = [0u8; 1];
            if f.read(&mut extra).map_err(|e| io_err(e, "Cannot read", &p))? != 0 || hex != info.sha256 {
                return Err(Error::new(ErrorCode::DataCorrupt, format!("Member '{name}' of {} failed its length/digest check.", self.manifest.artifact_id)));
            }
        }
        if cfg!(target_endian = "big") {
            for v in &mut out {
                *v = f64::from_bits(u64::from_le(v.to_bits()));
            }
        }
        Ok(out)
    }

    /// Read and verify several f64 members in parallel.
    pub fn f64s_many(&self, names: &[String]) -> Result<Vec<Vec<f64>>> {
        use rayon::prelude::*;
        names.par_iter().map(|n| self.f64s(n)).collect()
    }

    pub fn u64s(&self, name: &str) -> Result<Vec<u64>> {
        Ok(self.typed(name, DType::U64)?.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect())
    }

    pub fn u32s(&self, name: &str) -> Result<Vec<u32>> {
        Ok(self.typed(name, DType::U32)?.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
    }

    pub fn bools(&self, name: &str) -> Result<Vec<bool>> {
        Ok(self.typed(name, DType::U8)?.into_iter().map(|b| b != 0).collect())
    }

    pub fn meta<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<T> {
        let v = self.manifest.metadata.get(key).cloned().unwrap_or(Value::Null);
        serde_json::from_value(v).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Artifact {} metadata '{key}' is invalid: {e}", self.manifest.artifact_id)))
    }
}
