//! Service configuration shared by the CLI and MCP adapters.

use artpop_core::error::{Error, ErrorCode, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub max_stars: u64,
    pub max_grid_pixels: u64,
    pub max_image_pixels: u64,
    pub max_memory_bytes: u64,
    pub max_inline_values: usize,
    pub max_page_rows: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_stars: 50_000_000,
            max_grid_pixels: 100_000_000,
            max_image_pixels: 64_000_000,
            max_memory_bytes: 8 * 1024 * 1024 * 1024,
            max_inline_values: 100_000,
            max_page_rows: 1_000,
        }
    }
}

/// Which local paths requests may read or write.
#[derive(Debug, Clone, Serialize)]
pub enum FilePolicy {
    /// Any path the process can access (local CLI operator).
    Unrestricted,
    /// Only beneath these roots (MCP server default).
    Roots(Vec<PathBuf>),
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceConfig {
    pub artifact_root: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub limits: Limits,
    pub file_policy: FilePolicy,
    /// Whether `data.fetch` may use the network.
    pub allow_network: bool,
}

impl ServiceConfig {
    pub fn new(artifact_root: PathBuf) -> ServiceConfig {
        ServiceConfig {
            artifact_root,
            data_dir: std::env::var_os("ARTPOP_DATA_DIR").map(PathBuf::from),
            limits: Limits::default(),
            file_policy: FilePolicy::Unrestricted,
            allow_network: true,
        }
    }

    /// Resolve a request path and check it against the file policy.
    pub fn check_path(&self, raw: &str, pointer: &str) -> Result<PathBuf> {
        if raw.is_empty() {
            return Err(Error::invalid(pointer, "Path must not be empty."));
        }
        let p = PathBuf::from(raw);
        let abs = if p.is_absolute() { p } else { std::env::current_dir().map_err(|e| Error::io(e.to_string()))?.join(p) };
        let normalized = normalize(&abs);
        if let FilePolicy::Roots(roots) = &self.file_policy {
            let resolved = resolve_existing_prefix(&normalized);
            let ok = roots.iter().any(|r| {
                let rr = resolve_existing_prefix(&normalize(r));
                resolved.starts_with(&rr)
            });
            if !ok {
                return Err(Error::new(ErrorCode::InvalidParameter, format!("Path {} is outside the configured file roots.", normalized.display()))
                    .at(pointer)
                    .with_details(serde_json::json!({ "allowed_roots": roots })));
            }
        }
        Ok(normalized)
    }
}

/// Lexically remove `.` and `..` components.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the longest existing prefix so symlinks cannot escape roots.
fn resolve_existing_prefix(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                rest.push(name.to_os_string());
                existing.pop();
            }
            None => break,
        }
    }
    let mut base = existing.canonicalize().unwrap_or(existing);
    for r in rest.iter().rev() {
        base.push(r);
    }
    base
}
