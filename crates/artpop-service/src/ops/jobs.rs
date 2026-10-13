//! Durable background jobs backed by the artifact root.
//!
//! `job.submit` validates the request, records it under
//! `<artifact_root>/jobs/<job_id>/`, and starts a detached worker process
//! (`artpop-rust worker run-job`) so a short-lived CLI does not orphan the
//! computation. Cancellation is cooperative: a marker file is polled by the
//! worker, which exits without publishing a result. Artifacts are written
//! atomically, so an interrupted job never leaves a half-written artifact.

use crate::store::{canonical_json, sha256_hex};
use crate::{Ctx, OpOutput, Service};
use artpop_core::error::{Error, ErrorCode, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobState {
    pub job_id: String,
    pub operation: String,
    /// queued, running, succeeded, failed, cancelled, interrupted.
    pub state: String,
    pub request_sha256: String,
    pub submitted_unix_ms: u128,
    #[serde(default)]
    pub started_unix_ms: Option<u128>,
    #[serde(default)]
    pub finished_unix_ms: Option<u128>,
    #[serde(default)]
    pub worker_pid: Option<u32>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub error: Option<Value>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn jobs_dir(root: &Path) -> PathBuf {
    root.join("jobs")
}

fn job_dir(root: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 36 || !id.starts_with("job-") || !id[4..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid("/job_id", "Job IDs look like 'job-<32 hex digits>'."));
    }
    Ok(jobs_dir(root).join(id))
}

fn read_state(dir: &Path) -> Result<JobState> {
    let b = std::fs::read(dir.join("state.json")).map_err(|_| Error::new(ErrorCode::ArtifactNotFound, format!("No job at {}.", dir.display())).at("/job_id"))?;
    serde_json::from_slice(&b).map_err(|e| Error::new(ErrorCode::DataCorrupt, format!("Job state is malformed: {e}")))
}

pub fn write_state(dir: &Path, s: &JobState) -> Result<()> {
    let tmp = dir.join(format!(".state.tmp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap()).map_err(|e| Error::io(e.to_string()))?;
    std::fs::rename(&tmp, dir.join("state.json")).map_err(|e| Error::io(e.to_string()))
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() && std::fs::read_to_string(format!("/proc/{pid}/stat")).map(|s| !s.contains(") Z ")).unwrap_or(false)
}

/// PID recorded by the submitter in `worker.pid` (separate from the state
/// file, so the worker's own state writes are never overwritten).
fn spawned_pid(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("worker.pid")).ok().and_then(|s| s.trim().parse().ok())
}

/// Refresh state: a running job whose worker vanished is `interrupted`.
fn refresh(dir: &Path) -> Result<JobState> {
    let mut s = read_state(dir)?;
    if s.worker_pid.is_none() {
        s.worker_pid = spawned_pid(dir);
    }
    if (s.state == "running" || s.state == "queued") && s.worker_pid.is_some_and(|p| !pid_alive(p)) && !dir.join("result.json").exists() {
        s.state = if dir.join("cancel").exists() { "cancelled".into() } else { "interrupted".into() };
        s.finished_unix_ms = Some(now_ms());
        write_state(dir, &s)?;
    }
    Ok(s)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    /// Producing operation ID, e.g. `population.build`.
    pub operation: String,
    /// The full operation request (including schema_version and execution).
    pub request: Value,
    /// Reusing a key with the same request returns the same job.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub fn submit(ctx: &Ctx, req: SubmitRequest) -> Result<OpOutput> {
    let def = crate::registry::find(&req.operation).ok_or_else(|| Error::invalid("/operation", format!("Unknown operation '{}'.", req.operation)))?;
    if def.domain() == "job" {
        return Err(Error::invalid("/operation", "Jobs cannot submit job operations."));
    }
    // Validate fully before accepting.
    let (obj, _) = ctx.svc.prepare(req.request.clone()).map_err(|e| e.under("/request"))?;
    (def.validate)(Value::Object(obj)).map_err(|e| e.under("/request"))?;
    let root = &ctx.config().artifact_root;
    let canonical = canonical_json(&json!({ "operation": def.id, "request": req.request }));
    let digest = sha256_hex(canonical.as_bytes());
    std::fs::create_dir_all(jobs_dir(root).join("keys")).map_err(|e| Error::io(e.to_string()))?;
    if let Some(key) = &req.idempotency_key {
        let kp = jobs_dir(root).join("keys").join(sha256_hex(key.as_bytes()));
        if let Ok(existing) = std::fs::read_to_string(&kp) {
            let dir = job_dir(root, existing.trim())?;
            let s = refresh(&dir)?;
            if s.request_sha256 != digest {
                return Err(Error::new(ErrorCode::IdempotencyConflict, "This idempotency key was used with a different request.").at("/idempotency_key").with_details(json!({ "job_id": s.job_id })));
            }
            return Ok(OpOutput::new(json!({ "job_id": s.job_id, "state": s.state, "reused": true })));
        }
    }
    let id = format!("job-{}", uuid::Uuid::new_v4().simple());
    let dir = job_dir(root, &id)?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(e.to_string()))?;
    std::fs::write(dir.join("request.json"), serde_json::to_vec_pretty(&json!({ "operation": def.id, "request": req.request })).unwrap()).map_err(|e| Error::io(e.to_string()))?;
    let state = JobState {
        job_id: id.clone(),
        operation: def.id.into(),
        state: "queued".into(),
        request_sha256: digest,
        submitted_unix_ms: now_ms(),
        started_unix_ms: None,
        finished_unix_ms: None,
        worker_pid: None,
        idempotency_key: req.idempotency_key.clone(),
        error: None,
    };
    write_state(&dir, &state)?;
    if let Some(key) = &req.idempotency_key {
        let kp = jobs_dir(root).join("keys").join(sha256_hex(key.as_bytes()));
        // create_new makes concurrent submissions with one key race-safe.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&kp) {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(id.as_bytes());
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(Error::new(ErrorCode::IdempotencyConflict, "A concurrent submission claimed this idempotency key; retry to receive its job.").at("/idempotency_key"));
            }
        }
    }
    let exe = std::env::current_exe().map_err(|e| Error::internal(format!("Cannot locate the worker executable: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("worker").arg("run-job").arg("--job-dir").arg(&dir).arg("--artifact-dir").arg(root);
    if let Some(d) = &ctx.config().data_dir {
        cmd.arg("--data-dir").arg(d);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::fs::File::create(dir.join("worker.log")).map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null()));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| Error::internal(format!("Cannot start the job worker: {e}")))?;
    // The worker rewrites state.json (running, its own PID) as soon as it
    // starts, possibly before this line; recording the PID in a separate file
    // avoids a read-modify-write race that would revert its state to queued.
    std::fs::write(dir.join("worker.pid"), child.id().to_string()).map_err(|e| Error::io(e.to_string()))?;
    Ok(OpOutput::new(json!({ "job_id": id, "state": "queued", "worker_pid": child.id(), "status_request": { "operation": "job.status", "request": { "schema_version": "1", "job_id": id } } })))
}

/// Worker entry point: execute the stored request and publish the envelope.
pub fn run_job(svc: &Service, dir: &Path) -> i32 {
    let Ok(mut state) = read_state(dir) else { return 3 };
    if dir.join("cancel").exists() {
        state.state = "cancelled".into();
        state.finished_unix_ms = Some(now_ms());
        let _ = write_state(dir, &state);
        return 130;
    }
    state.state = "running".into();
    state.started_unix_ms = Some(now_ms());
    state.worker_pid = Some(std::process::id());
    let _ = write_state(dir, &state);
    // Cooperative cancellation watcher.
    let watch = dir.to_path_buf();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if watch.join("cancel").exists() {
                if let Ok(mut s) = read_state(&watch)
                    && s.state == "running" {
                        s.state = "cancelled".into();
                        s.finished_unix_ms = Some(now_ms());
                        let _ = write_state(&watch, &s);
                    }
                std::process::exit(130);
            }
        }
    });
    let stored: Value = match std::fs::read(dir.join("request.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => v,
        None => return 3,
    };
    let op = stored["operation"].as_str().unwrap_or_default().to_string();
    let env = svc.execute(&op, stored["request"].clone());
    let tmp = dir.join(format!(".result.tmp-{}", uuid::Uuid::new_v4().simple()));
    let _ = std::fs::write(&tmp, serde_json::to_vec_pretty(&env).unwrap());
    if dir.join("cancel").exists() {
        let _ = std::fs::remove_file(&tmp);
        return 130;
    }
    let _ = std::fs::rename(&tmp, dir.join("result.json"));
    state.state = if env.status == "completed" { "succeeded".into() } else { "failed".into() };
    state.error = env.error.as_ref().map(|e| serde_json::to_value(e).unwrap());
    state.finished_unix_ms = Some(now_ms());
    let _ = write_state(dir, &state);
    0
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusRequest {
    pub job_id: String,
}

pub fn status(ctx: &Ctx, req: StatusRequest) -> Result<OpOutput> {
    let dir = job_dir(&ctx.config().artifact_root, &req.job_id)?;
    let s = refresh(&dir)?;
    let elapsed = s.started_unix_ms.map(|t| s.finished_unix_ms.unwrap_or_else(now_ms) - t);
    Ok(OpOutput::new(json!({ "job": s, "elapsed_ms": elapsed, "result_available": dir.join("result.json").exists(), "resumable": false })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub job_id: String,
}

pub fn cancel(ctx: &Ctx, req: CancelRequest) -> Result<OpOutput> {
    let dir = job_dir(&ctx.config().artifact_root, &req.job_id)?;
    let s = refresh(&dir)?;
    if matches!(s.state.as_str(), "succeeded" | "failed" | "cancelled" | "interrupted") {
        return Ok(OpOutput::new(json!({ "job_id": s.job_id, "state": s.state, "cancelled": false, "note": "The job had already finished; completion won the race." })));
    }
    std::fs::write(dir.join("cancel"), b"cancel").map_err(|e| Error::io(e.to_string()))?;
    Ok(OpOutput::new(json!({ "job_id": s.job_id, "state": s.state, "cancel_requested": true, "note": "Poll job.status; the final state reports whether cancellation or completion won." })))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResultRequest {
    pub job_id: String,
}

pub fn result(ctx: &Ctx, req: ResultRequest) -> Result<OpOutput> {
    let dir = job_dir(&ctx.config().artifact_root, &req.job_id)?;
    let s = refresh(&dir)?;
    match s.state.as_str() {
        "succeeded" | "failed" => {
            let env: Value = serde_json::from_slice(&std::fs::read(dir.join("result.json")).map_err(|e| Error::io(e.to_string()))?).map_err(|e| Error::new(ErrorCode::DataCorrupt, e.to_string()))?;
            Ok(OpOutput::new(json!({ "job_id": s.job_id, "state": s.state, "envelope": env })))
        }
        "cancelled" => Err(Error::new(ErrorCode::Cancelled, "The job was cancelled; no result was published.").with_details(json!({ "job_id": s.job_id }))),
        "interrupted" => Err(Error::new(ErrorCode::JobNotReady, "The job's worker stopped before finishing (interrupted); resubmit it.").with_details(json!({ "job_id": s.job_id, "state": "interrupted" }))),
        other => Err(Error::new(ErrorCode::JobNotReady, format!("The job is {other}.")).with_details(json!({ "job_id": s.job_id, "state": other }))),
    }
}

