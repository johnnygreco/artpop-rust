//! Adapter tests: CLI stream/exit-code contract, durable jobs, and an MCP
//! client session (official rmcp SDK) producing the same artifacts as the CLI.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_artpop-rust");

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("artpop-cli-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `artpop-rust invoke OP` with a JSON request on stdin.
fn invoke(dir: &Path, op: &str, req: Value) -> (i32, Value, String) {
    let mut child = Command::new(BIN)
        .args(["--artifact-dir", dir.join("art").to_str().unwrap(), "invoke", op, "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(req.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "exactly one JSON document on stdout: {stdout}");
    (out.status.code().unwrap(), serde_json::from_str(&stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn sample_request() -> Value {
    json!({ "schema_version": "1", "imf": { "kind": "kroupa" }, "count": 1000, "execution": { "seed": "00000000000000aa" } })
}

#[test]
fn exit_codes_and_clean_streams() {
    let d = Dir::new("exit");
    let (code, env, stderr) = invoke(&d.0, "imf.sample", sample_request());
    assert_eq!((code, env["status"].as_str().unwrap()), (0, "completed"));
    assert!(stderr.is_empty());
    let masses = env["result"]["masses_artifact_id"].clone();
    let (code, env, _) = invoke(&d.0, "imf.sample", json!({ "schema_version": "1", "count": 10 }));
    assert_eq!((code, env["error"]["path"].as_str().unwrap()), (2, "/execution/seed"));
    let (code, env, _) = invoke(&d.0, "isochrone.mist", json!({ "schema_version": "1", "log_age_yr": 9, "feh": 0, "photometric_systems": ["LSST"] }));
    assert_eq!((code, env["error"]["code"].as_str().unwrap()), (3, "DATA_MISSING"));
    let (code, env, _) = invoke(&d.0, "imf.sample", json!({ "schema_version": "1", "count": 1_000_000_000u64, "execution": { "seed": "0000000000000001", "max_stars": 100 } }));
    assert_eq!((code, env["error"]["code"].as_str().unwrap()), (4, "RESOURCE_LIMIT"));
    // Domain form with --set and --seed builds the same request as invoke.
    let out = Command::new(BIN).args(["--artifact-dir", d.0.join("art").to_str().unwrap(), "imf", "sample", "--set", "count=1000", "--set", "imf={\"kind\":\"kroupa\"}", "--seed", "00000000000000aa"]).output().unwrap();
    let env2: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env2["result"]["masses_artifact_id"], masses);
}

fn wait_for(d: &Path, job: &str, states: &[&str], timeout: Duration) -> Value {
    let start = Instant::now();
    loop {
        let (_, st, _) = invoke(d, "job.status", json!({ "schema_version": "1", "job_id": job }));
        let s = st["result"]["job"]["state"].as_str().unwrap_or("").to_string();
        if states.contains(&s.as_str()) {
            return st;
        }
        assert!(start.elapsed() < timeout, "job {job} stuck in {s}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn slow_psf() -> Value {
    // Adaptive 2D quadrature over 401x401 pixels: seconds of CPU, little memory.
    json!({ "schema_version": "1", "kind": "moffat", "fwhm": { "value": 40, "unit": "pixel" }, "width": 401, "height": 401, "mode": "integrate", "integration_rel_tol": 1e-13 })
}

#[test]
fn jobs_complete_dedupe_conflict_and_cancel() {
    let d = Dir::new("jobs");
    let (code, sub, _) = invoke(&d.0, "job.submit", json!({ "schema_version": "1", "operation": "imf.sample", "request": sample_request(), "idempotency_key": "k1" }));
    assert_eq!(code, 0, "{sub}");
    let job = sub["result"]["job_id"].as_str().unwrap().to_string();
    let (_, again, _) = invoke(&d.0, "job.submit", json!({ "schema_version": "1", "operation": "imf.sample", "request": sample_request(), "idempotency_key": "k1" }));
    assert_eq!(again["result"]["job_id"], job.as_str());
    assert_eq!(again["result"]["reused"], true);
    let mut other = sample_request();
    other["count"] = json!(2000);
    let (code, conflict, _) = invoke(&d.0, "job.submit", json!({ "schema_version": "1", "operation": "imf.sample", "request": other, "idempotency_key": "k1" }));
    assert_eq!((code, conflict["error"]["code"].as_str().unwrap()), (7, "IDEMPOTENCY_CONFLICT"));
    wait_for(&d.0, &job, &["succeeded"], Duration::from_secs(60));
    let (_, res, _) = invoke(&d.0, "job.result", json!({ "schema_version": "1", "job_id": job }));
    let direct = invoke(&d.0, "imf.sample", sample_request()).1;
    assert_eq!(res["result"]["envelope"]["result"]["masses_artifact_id"], direct["result"]["masses_artifact_id"]);

    // Cancellation: no PSF artifact may be published.
    let (_, sub, _) = invoke(&d.0, "job.submit", json!({ "schema_version": "1", "operation": "psf.build", "request": slow_psf() }));
    let job = sub["result"]["job_id"].as_str().unwrap().to_string();
    wait_for(&d.0, &job, &["running"], Duration::from_secs(30));
    let (_, c, _) = invoke(&d.0, "job.cancel", json!({ "schema_version": "1", "job_id": job }));
    assert_eq!(c["result"]["cancel_requested"], true);
    let st = wait_for(&d.0, &job, &["cancelled", "succeeded"], Duration::from_secs(60));
    assert_eq!(st["result"]["job"]["state"], "cancelled");
    let (code, r, _) = invoke(&d.0, "job.result", json!({ "schema_version": "1", "job_id": job }));
    assert_eq!((code, r["error"]["code"].as_str().unwrap()), (130, "CANCELLED"));
    let (_, list, _) = invoke(&d.0, "artifact.list", json!({ "schema_version": "1", "kind": "psf" }));
    assert_eq!(list["result"]["total"], 0);
}

#[test]
fn killed_worker_is_reported_interrupted() {
    let d = Dir::new("interrupt");
    let (_, sub, _) = invoke(&d.0, "job.submit", json!({ "schema_version": "1", "operation": "psf.build", "request": slow_psf() }));
    let job = sub["result"]["job_id"].as_str().unwrap().to_string();
    let st = wait_for(&d.0, &job, &["running"], Duration::from_secs(30));
    let pid = st["result"]["job"]["worker_pid"].as_u64().unwrap();
    Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
    let st = wait_for(&d.0, &job, &["interrupted"], Duration::from_secs(30));
    assert_eq!(st["result"]["resumable"], false);
    let (code, r, _) = invoke(&d.0, "job.result", json!({ "schema_version": "1", "job_id": job }));
    assert_eq!((code, r["error"]["code"].as_str().unwrap()), (7, "JOB_NOT_READY"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_session_matches_cli_artifacts() {
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    let d = Dir::new("mcp");
    let art = d.0.join("mcp-art");
    let transport = TokioChildProcess::new(tokio::process::Command::new(BIN).configure(|c| {
        c.args(["--artifact-dir", art.to_str().unwrap(), "mcp", "serve"]);
    }))
    .unwrap();
    let client = ().serve(transport).await.unwrap();
    let info = client.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "artpop-rust");
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 68);
    let t = tools.iter().find(|t| t.name == "artpop_population_build").unwrap();
    assert!(t.output_schema.is_some());
    assert_eq!(t.input_schema["additionalProperties"], json!(false));

    let args = sample_request().as_object().unwrap().clone();
    let r = client.call_tool(CallToolRequestParams::new("artpop_imf_sample").with_arguments(args)).await.unwrap();
    assert_eq!(r.is_error, Some(false));
    let env = r.structured_content.unwrap();
    let cli = invoke(&d.0, "imf.sample", sample_request()).1;
    assert_eq!(env["result"]["masses_artifact_id"], cli["result"]["masses_artifact_id"], "same request => same content-addressed artifact via MCP and CLI");

    let bad = client.call_tool(CallToolRequestParams::new("artpop_imf_sample").with_arguments(json!({ "schema_version": "1", "count": 5 }).as_object().unwrap().clone())).await.unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert_eq!(bad.structured_content.unwrap()["error"]["path"], "/execution/seed");

    // MCP file access is confined to configured roots.
    let outside = client
        .call_tool(CallToolRequestParams::new("artpop_artifact_export").with_arguments(json!({ "schema_version": "1", "artifact_id": env["result"]["masses_artifact_id"], "destination": "/etc/artpop-should-not-write.csv", "format": "csv" }).as_object().unwrap().clone()))
        .await
        .unwrap();
    assert_eq!(outside.is_error, Some(true));

    let caps = client.read_resource(ReadResourceRequestParams::new("artpop://capabilities")).await.unwrap();
    assert!(!caps.contents.is_empty());
    let manifest_uri = format!("artpop://artifacts/{}/manifest", env["result"]["masses_artifact_id"].as_str().unwrap());
    assert!(!client.read_resource(ReadResourceRequestParams::new(manifest_uri)).await.unwrap().contents.is_empty());
    client.cancel().await.unwrap();
}

/// Raw JSON-RPC over stdio: legacy `initialize` lifecycles and the 2026-07-28
/// stateless `server/discover` lifecycle are negotiated separately.
#[test]
fn mcp_protocol_revisions_negotiate() {
    let d = Dir::new("mcp-raw");
    let run = |lines: &[String]| -> Vec<Value> {
        let mut child = Command::new(BIN)
            .args(["--artifact-dir", d.0.join("art").to_str().unwrap(), "mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        for l in lines {
            writeln!(stdin, "{l}").unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).expect("stdout carries only JSON-RPC")).collect()
    };
    for v in ["2024-11-05", "2025-06-18", "2025-11-25"] {
        let msgs = run(&[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": v, "capabilities": {}, "clientInfo": { "name": "raw", "version": "0" } } }).to_string(),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
        ]);
        let init = msgs.iter().find(|m| m["id"] == 1).unwrap();
        assert_eq!(init["result"]["protocolVersion"], v);
        assert_eq!(msgs.iter().find(|m| m["id"] == 2).unwrap()["result"]["tools"].as_array().unwrap().len(), 68);
    }
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": { "name": "raw", "version": "0" }, "io.modelcontextprotocol/clientCapabilities": {} });
    let msgs = run(&[
        json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": meta } }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "_meta": meta, "name": "artpop_imf_integrate", "arguments": { "schema_version": "1", "m_min": 0.1, "m_max": 1.0 } } }).to_string(),
    ]);
    let disc = msgs.iter().find(|m| m["id"] == 1).unwrap();
    assert!(disc["result"]["supportedVersions"].as_array().unwrap().contains(&json!("2026-07-28")));
    let call = msgs.iter().find(|m| m["id"] == 2).unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert!(call["result"]["structuredContent"]["result"]["integral"].as_f64().unwrap() > 0.0);
}

#[test]
fn get_flag_prints_one_field_and_full_errors() {
    let d = Dir::new("get");
    let art = d.0.join("art");
    let run = |args: &[&str]| Command::new(BIN).arg("--artifact-dir").arg(&art).args(args).output().unwrap();
    let out = run(&["imf", "sample", "--set", "count=10", "--seed", "0000000000000001", "--get", "/result/masses_artifact_id"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success());
    assert!(text.trim().starts_with("sha256:") && text.lines().count() == 1, "{text}");
    let err = run(&["imf", "sample", "--set", "count=10", "--get", "/result/masses_artifact_id"]);
    assert_eq!(err.status.code(), Some(2));
    let env: Value = serde_json::from_slice(&err.stdout).unwrap();
    assert_eq!(env["error"]["path"], "/execution/seed");
}
