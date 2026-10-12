//! `artpop-rust`: CLI adapter over the shared operation registry.
//!
//! stdout carries exactly one JSON result envelope per operation (or JSONL in
//! batch mode); diagnostics go to stderr. Exit codes follow the contract:
//! 0 ok, 2 validation, 3 missing data, 4 resource limit, 5 I/O/network,
//! 6 numerical, 7 job conflict, 130 cancelled, 1 internal.

use artpop_service::config::{FilePolicy, ServiceConfig};
use artpop_service::{Envelope, Service, registry};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "artpop-rust",
    version,
    about = "Artificial stellar populations and images (Rust port of ArtPop 0.1.2)",
    long_about = None,
    after_help = "Operations can also be called as `artpop-rust <domain> <action> [--input FILE|-] [--set KEY=JSON]... [--seed HEX]`,\n\
e.g. `artpop-rust population build --input pop.json --seed 000000000000002a`. Run `artpop-rust operations` for all 68,\n\
and `artpop-rust system schema --set operation=population.build` for a request schema."
)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct Global {
    /// Artifact store root (alias: --output-dir). Default: $ARTPOP_ARTIFACT_DIR or ./artpop-artifacts.
    #[arg(long, global = true, alias = "output-dir", value_name = "DIR")]
    artifact_dir: Option<PathBuf>,
    /// Model data directory (MIST manifests). Default: $ARTPOP_DATA_DIR.
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,
    /// Output format for results.
    #[arg(long, global = true, value_enum, default_value_t = Format::Json)]
    format: Format,
    /// Pretty-print JSON (default when stdout is a terminal).
    #[arg(long, global = true)]
    pretty: bool,
    /// Disable network access (data.fetch fails with NETWORK_ERROR).
    #[arg(long, global = true)]
    offline: bool,
    /// Print only this JSON pointer of the envelope (strings unquoted), e.g.
    /// `--get /result/population_artifact_id`; the exit code still reflects the status.
    #[arg(long, global = true, value_name = "POINTER")]
    get: Option<String>,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Format {
    Json,
    Text,
}

#[derive(Subcommand)]
enum Command {
    /// Run any registered operation: `invoke population.build --input req.json`.
    Invoke(InvokeArgs),
    /// Run newline-delimited {"operation", "request"} objects; one envelope per line.
    Batch {
        #[arg(long, value_name = "PATH|-")]
        input: String,
        /// Keep going after a failed request (default: stop).
        #[arg(long)]
        continue_on_error: bool,
    },
    /// List operations (alias for `system capabilities` in text form).
    Operations,
    /// Model Context Protocol server.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Background job worker (started by job.submit).
    Worker {
        #[command(subcommand)]
        action: WorkerAction,
    },
    /// Domain commands: `artpop-rust <domain> <action> [--input FILE] [--set key=value]...`
    #[command(external_subcommand)]
    Domain(Vec<String>),
}

#[derive(Args)]
struct InvokeArgs {
    /// Operation ID (e.g. `imf.sample`) or MCP tool name.
    operation: String,
    #[command(flatten)]
    request: RequestArgs,
}

#[derive(Args, Default)]
struct RequestArgs {
    /// Request JSON file, or `-` for stdin.
    #[arg(long, value_name = "PATH|-")]
    input: Option<String>,
    /// Set request fields not present in --input: `--set count=1000 --set imf.kind=kroupa`.
    #[arg(long = "set", value_name = "KEY=JSON")]
    set: Vec<String>,
    /// Execution seed override (16 hex digits); recorded in resolved_request.
    #[arg(long)]
    seed: Option<String>,
}

#[derive(Subcommand)]
enum McpAction {
    /// Serve over stdio (stdout is reserved for protocol messages).
    Serve {
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Extra directories requests may read or write (artifact and data dirs are always allowed).
        #[arg(long = "allow-path", value_name = "DIR")]
        allow_path: Vec<PathBuf>,
        /// Allow any local path (trusted local use only).
        #[arg(long)]
        unrestricted_paths: bool,
    },
}

#[derive(Subcommand)]
enum WorkerAction {
    RunJob {
        #[arg(long)]
        job_dir: PathBuf,
    },
}

fn config(g: &Global) -> ServiceConfig {
    let root = g.artifact_dir.clone().or_else(|| std::env::var_os("ARTPOP_ARTIFACT_DIR").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("artpop-artifacts"));
    let root = if root.is_absolute() { root } else { std::env::current_dir().map(|c| c.join(&root)).unwrap_or(root) };
    let mut c = ServiceConfig::new(root);
    if let Some(d) = &g.data_dir {
        c.data_dir = Some(if d.is_absolute() { d.clone() } else { std::env::current_dir().map(|c| c.join(d)).unwrap_or(d.clone()) });
    }
    c.allow_network = !g.offline;
    c
}

fn read_input(spec: &str) -> Result<Value, String> {
    let text = if spec == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| format!("Cannot read stdin: {e}"))?;
        s
    } else {
        std::fs::read_to_string(spec).map_err(|e| format!("Cannot read {spec}: {e}"))?
    };
    serde_json::from_str(&text).map_err(|e| format!("Request is not valid JSON: {e}"))
}

/// Insert `value` at a dotted path, refusing to override existing fields.
fn set_path(v: &mut Value, parts: &[&str], value: Value, key: &str) -> Result<(), (String, String)> {
    let obj = v.as_object_mut().ok_or_else(|| (format!("--set {key}: '{}' is not inside an object.", parts[0]), String::new()))?;
    if parts.len() == 1 {
        if obj.contains_key(parts[0]) {
            return Err((format!("--set {key} would override a value from --input; edit the input file instead."), format!("/{}", key.replace('.', "/"))));
        }
        obj.insert(parts[0].to_string(), value);
        return Ok(());
    }
    let child = obj.entry(parts[0].to_string()).or_insert_with(|| json!({}));
    set_path(child, &parts[1..], value, key)
}

/// Build a request from --input, --set, and --seed without silent overrides.
fn build_request(args: &RequestArgs) -> Result<Value, (String, String)> {
    let mut req = match &args.input {
        Some(spec) => read_input(spec).map_err(|m| (m, "/input".to_string()))?,
        None => json!({ "schema_version": "1" }),
    };
    if !req.is_object() {
        return Err(("A request must be a JSON object.".into(), String::new()));
    }
    for s in &args.set {
        let (key, raw) = s.split_once('=').ok_or_else(|| (format!("--set expects KEY=VALUE, got '{s}'."), "/set".to_string()))?;
        let value: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        let parts: Vec<&str> = key.split('.').collect();
        set_path(&mut req, &parts, value, key)?;
    }
    if let Some(seed) = &args.seed {
        let exec = req.as_object_mut().unwrap().entry("execution").or_insert_with(|| json!({}));
        match exec.as_object_mut() {
            Some(e) => {
                if e.get("seed").is_some_and(|s| s.as_str() != Some(seed)) {
                    return Err(("--seed conflicts with execution.seed in --input.".into(), "/execution/seed".into()));
                }
                e.insert("seed".into(), json!(seed));
            }
            None => return Err(("execution must be an object.".into(), "/execution".into())),
        }
    }
    Ok(req)
}

fn error_envelope(operation: &str, code: &str, message: &str, path: &str) -> Value {
    json!({
        "schema_version": "1",
        "status": "error",
        "operation": operation,
        "request_id": null,
        "artifacts": [],
        "warnings": [],
        "error": { "code": code, "message": message, "path": if path.is_empty() { Value::Null } else { json!(path) }, "retryable": false },
        "provenance": { "engine_version": artpop_core::ENGINE_VERSION, "algorithm_profile": artpop_core::ALGORITHM_PROFILE },
        "metrics": {},
    })
}

fn emit(g: &Global, v: &Value) {
    let pretty = g.pretty || std::io::stdout().is_terminal();
    let mut out = std::io::stdout().lock();
    if let Some(ptr) = &g.get {
        if v["status"] == "error" {
            // Errors are always reported in full so they are never lost.
            let _ = writeln!(out, "{}", serde_json::to_string(v).unwrap());
            return;
        }
        match v.pointer(ptr) {
            Some(Value::String(s)) => {
                let _ = writeln!(out, "{s}");
            }
            Some(other) => {
                let _ = writeln!(out, "{other}");
            }
            None => eprintln!("--get: no value at {ptr}"),
        }
        return;
    }
    if g.format == Format::Text {
        let _ = writeln!(out, "{}", text_summary(v));
    } else if pretty {
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(v).unwrap());
    } else {
        let _ = writeln!(out, "{}", serde_json::to_string(v).unwrap());
    }
}

fn text_summary(v: &Value) -> String {
    let mut s = format!("{} {}", v["operation"].as_str().unwrap_or("?"), v["status"].as_str().unwrap_or("?"));
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        s.push_str(&format!("\n  error {}: {}", e["code"].as_str().unwrap_or(""), e["message"].as_str().unwrap_or("")));
        if let Some(p) = e["path"].as_str() {
            s.push_str(&format!(" (at {p})"));
        }
    }
    if let Some(r) = v.get("result").and_then(Value::as_object) {
        for (k, val) in r.iter().take(24) {
            let t = val.to_string();
            s.push_str(&format!("\n  {k}: {}", if t.len() > 160 { format!("{}…", &t[..160]) } else { t }));
        }
    }
    for a in v["artifacts"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n  artifact {} {}", a["kind"].as_str().unwrap_or(""), a["artifact_id"].as_str().unwrap_or("")));
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n  warning {}: {}", w["code"].as_str().unwrap_or(""), w["message"].as_str().unwrap_or("")));
    }
    s
}

fn run_operation(g: &Global, svc: &Service, operation: &str, args: &RequestArgs) -> i32 {
    match build_request(args) {
        Ok(req) => {
            let env: Envelope = svc.execute(operation, req);
            emit(g, &serde_json::to_value(&env).unwrap());
            env.exit_code()
        }
        Err((msg, path)) => {
            emit(g, &error_envelope(operation, "INVALID_PARAMETER", &msg, &path));
            2
        }
    }
}

fn parse_domain(args: &[String], g: &mut Global) -> Result<(String, RequestArgs), String> {
    let (domain, rest) = args.split_first().ok_or("missing domain")?;
    let (action, flags) = rest.split_first().ok_or_else(|| format!("Usage: artpop-rust {domain} <action> [--input FILE] [--set KEY=VALUE]..."))?;
    let op = format!("{domain}.{action}");
    let mut ra = RequestArgs::default();
    let mut it = flags.iter();
    // `system schema --operation X` style shortcuts map to --set.
    while let Some(f) = it.next() {
        let mut take = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match f.as_str() {
            "--input" => ra.input = Some(take("--input")?),
            "--set" => ra.set.push(take("--set")?),
            "--seed" => ra.seed = Some(take("--seed")?),
            // Global flags are also accepted after the action.
            "--artifact-dir" | "--output-dir" => g.artifact_dir = Some(PathBuf::from(take(f)?)),
            "--data-dir" => g.data_dir = Some(PathBuf::from(take(f)?)),
            "--format" => {
                g.format = match take(f)?.as_str() {
                    "json" => Format::Json,
                    "text" => Format::Text,
                    other => return Err(format!("Unknown format '{other}' (json or text).")),
                }
            }
            "--pretty" => g.pretty = true,
            "--get" => g.get = Some(take(f)?),
            "--offline" => g.offline = true,
            other if other.starts_with("--") => {
                let key = other.trim_start_matches("--").replace('-', "_");
                let value = take(other)?;
                ra.set.push(format!("{key}={value}"));
            }
            other => return Err(format!("Unexpected argument '{other}'.")),
        }
    }
    Ok((op, ra))
}

fn main() {
    let cli = Cli::parse();
    let mut g = cli.global.clone();
    let code = match cli.command {
        Command::Invoke(a) => {
            let svc = Service::new(config(&g));
            run_operation(&g, &svc, &a.operation, &a.request)
        }
        Command::Domain(args) => match parse_domain(&args, &mut g) {
            Ok((op, ra)) => {
                if registry::find(&op).is_none() {
                    eprintln!("Unknown command '{}'. Run `artpop-rust operations` for the list.", args.join(" "));
                    emit(&g, &error_envelope(&op, "INVALID_PARAMETER", &format!("Unknown operation '{op}'."), "/operation"));
                    2
                } else {
                    let svc = Service::new(config(&g));
                    run_operation(&g, &svc, &op, &ra)
                }
            }
            Err(m) => {
                eprintln!("{m}");
                2
            }
        },
        Command::Operations => {
            for d in registry::all() {
                println!("{:28} {:34} {}", d.id, d.mcp_name(), d.summary);
            }
            0
        }
        Command::Batch { input, continue_on_error } => {
            let svc = Service::new(config(&g));
            let text = if input == "-" {
                let mut s = String::new();
                let _ = std::io::stdin().read_to_string(&mut s);
                s
            } else {
                std::fs::read_to_string(&input).unwrap_or_default()
            };
            let mut worst = 0;
            for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                let env = match serde_json::from_str::<Value>(line) {
                    Ok(v) => match (v.get("operation").and_then(Value::as_str), v.get("request")) {
                        (Some(op), Some(req)) => serde_json::to_value(svc.execute(op, req.clone())).unwrap(),
                        _ => error_envelope("batch", "INVALID_PARAMETER", &format!("Line {} needs \"operation\" and \"request\".", n + 1), ""),
                    },
                    Err(e) => error_envelope("batch", "INVALID_PARAMETER", &format!("Line {} is not JSON: {e}", n + 1), ""),
                };
                println!("{}", serde_json::to_string(&env).unwrap());
                if env["status"] == "error" {
                    worst = 2;
                    if !continue_on_error {
                        break;
                    }
                }
            }
            worst
        }
        Command::Mcp { action: McpAction::Serve { transport, allow_path, unrestricted_paths } } => {
            if transport != "stdio" {
                eprintln!("Only the stdio transport is available in this build.");
                std::process::exit(2);
            }
            let mut c = config(&g);
            if !unrestricted_paths {
                let mut roots = vec![c.artifact_root.clone()];
                roots.extend(c.data_dir.clone());
                roots.extend(allow_path.into_iter().map(|p| if p.is_absolute() { p } else { std::env::current_dir().unwrap().join(p) }));
                c.file_policy = FilePolicy::Roots(roots);
            }
            let rt = tokio_runtime();
            match rt.block_on(artpop_mcp::serve_stdio(Service::new(c))) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("MCP server error: {e}");
                    1
                }
            }
        }
        Command::Worker { action: WorkerAction::RunJob { job_dir } } => {
            let svc = Service::new(config(&g));
            artpop_service::ops::jobs::run_job(&svc, &job_dir)
        }
    };
    std::process::exit(code);
}

fn tokio_runtime() -> artpop_mcp::Runtime {
    artpop_mcp::runtime()
}
