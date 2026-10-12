//! MCP adapter: every registry operation becomes a tool with its generated
//! input schema and the shared result-envelope output schema. Tool results
//! carry the envelope as `structuredContent`; scientific failures are tool
//! execution errors (`isError: true`), not protocol errors.
//!
//! The server never writes anything but protocol messages to stdout.

// Errors carry JSON details for agents; they are cold-path values, so the
// large-Err lint is not useful. `!(x > 0.0)` comparisons deliberately reject NaN.
#![allow(clippy::result_large_err, clippy::neg_cmp_op_on_partial_ord)]

use artpop_service::registry::{self, OperationDef};
use artpop_service::{Service, store::parse_artifact_id};
use base64::Engine as _;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, InitializeResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
pub struct ArtpopMcp {
    svc: Arc<Service>,
}

impl ArtpopMcp {
    pub fn new(svc: Service) -> ArtpopMcp {
        ArtpopMcp { svc: Arc::new(svc) }
    }
}

fn object(v: Value) -> Arc<serde_json::Map<String, Value>> {
    match v {
        Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

pub fn tool_for(d: &OperationDef) -> Tool {
    let e = d.effects;
    let ann = ToolAnnotations::with_title(d.id)
        .read_only(e.read_only)
        .destructive(e.destructive)
        .idempotent(e.idempotent)
        .open_world(e.network);
    let description = format!(
        "{} (operation `{}`; {}{}cost: {}).",
        d.summary,
        d.id,
        if d.stochastic { "stochastic, requires execution.seed; " } else { "" },
        if e.writes_artifacts { "writes content-addressed artifacts; " } else if e.writes_files { "writes files; " } else { "" },
        d.cost
    );
    Tool::new(d.mcp_name(), description, object(d.input_schema()))
        .with_title(d.id)
        .with_raw_output_schema(object(registry::envelope_schema()))
        .with_annotations(ann)
}

/// Compact text companion of a structured envelope.
fn summary_text(env: &Value) -> String {
    let mut s = json!({
        "status": env["status"],
        "operation": env["operation"],
        "artifacts": env["artifacts"].as_array().map(|a| a.iter().map(|x| json!({ "artifact_id": x["artifact_id"], "kind": x["kind"] })).collect::<Vec<_>>()),
    });
    if let Some(e) = env.get("error") {
        s["error"] = e.clone();
    } else {
        s["result"] = env["result"].clone();
    }
    let text = s.to_string();
    if text.len() > 20_000 { format!("{}… (truncated; see structuredContent)", &text[..20_000]) } else { text }
}

impl ServerHandler for ArtpopMcp {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("artpop-rust", artpop_core::ENGINE_VERSION).with_title("artpop-rust stellar population and image simulator"))
            .with_instructions(
                "Scientific tools for artificial stellar populations and images (Rust port of ArtPop 0.1.2). \
                 Start with artpop_system_capabilities; every request needs \"schema_version\": \"1\"; stochastic tools need \
                 execution.seed (16 hex digits). Producing tools return artifact IDs (sha256:...) to pass to later tools; \
                 export files with artpop_artifact_export. MIST data must be registered (artpop_data_import/artpop_data_fetch) before artpop_isochrone_mist.",
            )
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(registry::all().iter().map(tool_for).collect()))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        let def = registry::find(&request.name).ok_or_else(|| McpError::invalid_params(format!("Unknown tool '{}'.", request.name), Some(json!({ "tools": "see tools/list" }))))?;
        let args = Value::Object(request.arguments.unwrap_or_default());
        let svc = self.svc.clone();
        let id = def.id;
        let env = tokio::task::spawn_blocking(move || svc.execute(id, args)).await.map_err(|e| McpError::internal_error(format!("Operation worker failed: {e}"), None))?;
        let value = serde_json::to_value(&env).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let mut result = if env.status == "error" { CallToolResult::structured_error(value.clone()) } else { CallToolResult::structured(value.clone()) };
        result.content = vec![rmcp::model::ContentBlock::text(summary_text(&value))];
        Ok(result.into())
    }

    async fn list_resources(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListResourcesResult, McpError> {
        let mut resources = vec![Resource::new("artpop://capabilities", "capabilities").with_description("Engine versions, operations, algorithms, limits, and datasets.").with_mime_type("application/json")];
        if let Ok(list) = self.svc.store.list() {
            for m in list.iter().take(200) {
                resources.push(Resource::new(format!("artpop://artifacts/{}/manifest", m.artifact_id), format!("{} {}", m.kind, &m.artifact_id[7..19])).with_mime_type("application/json"));
            }
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn list_resource_templates(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListResourceTemplatesResult, McpError> {
        let t = |uri: &str, name: &str, mime: &str, d: &str| ResourceTemplate::new(uri, name).with_mime_type(mime).with_description(d);
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            t("artpop://schemas/{operation}/1", "operation-schema", "application/json", "Input JSON Schema of an operation."),
            t("artpop://artifacts/{artifact_id}/manifest", "artifact-manifest", "application/json", "Artifact manifest with member digests and metadata."),
            t("artpop://artifacts/{artifact_id}/preview", "artifact-preview", "image/png", "PNG preview (preview artifacts, or an automatic preview of image/observation artifacts)."),
            t("artpop://datasets/{dataset_id}/manifest", "dataset-manifest", "application/json", "Registered model dataset manifest."),
            t("artpop://jobs/{job_id}/status", "job-status", "application/json", "Background job state."),
        ]))
    }

    async fn read_resource(&self, request: ReadResourceRequestParams, _context: RequestContext<RoleServer>) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.clone();
        let svc = self.svc.clone();
        let contents = tokio::task::spawn_blocking(move || read(&svc, &uri)).await.map_err(|e| McpError::internal_error(e.to_string(), None))??;
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

fn json_contents(v: &Value, uri: &str) -> ResourceContents {
    ResourceContents::text(serde_json::to_string_pretty(v).unwrap_or_default(), uri).with_mime_type("application/json")
}

fn op_result(svc: &Service, op: &str, req: Value) -> Result<Value, McpError> {
    let env = svc.execute(op, req);
    if env.status == "error" {
        let e = env.error.as_ref().map(|e| serde_json::to_value(e).unwrap()).unwrap_or(Value::Null);
        return Err(McpError::resource_not_found(e["message"].as_str().unwrap_or("resource unavailable").to_string(), Some(e)));
    }
    Ok(env.result.unwrap_or(Value::Null))
}

fn read(svc: &Service, uri: &str) -> Result<ResourceContents, McpError> {
    let rest = uri.strip_prefix("artpop://").ok_or_else(|| McpError::resource_not_found("Unknown resource scheme.", None))?;
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        ["capabilities"] => Ok(json_contents(&op_result(svc, "system.capabilities", json!({ "schema_version": "1" }))?, uri)),
        ["schemas", op, "1"] => Ok(json_contents(&op_result(svc, "system.schema", json!({ "schema_version": "1", "operation": op }))?, uri)),
        ["artifacts", id, "manifest"] => Ok(json_contents(&op_result(svc, "artifact.inspect", json!({ "schema_version": "1", "artifact_id": id }))?, uri)),
        ["artifacts", id, "preview"] => {
            parse_artifact_id(id, "/uri").map_err(|e| McpError::invalid_params(e.message, None))?;
            let l = svc.store.load(id, "/uri", &[]).map_err(|e| McpError::resource_not_found(e.message, None))?;
            let png_id = if l.manifest.kind == "preview" {
                id.to_string()
            } else {
                let r = op_result(svc, "image.preview", json!({ "schema_version": "1", "image": { "artifact_id": id }, "max_size": 512 }))?;
                r["preview_artifact_id"].as_str().unwrap_or_default().to_string()
            };
            let p = svc.store.load(&png_id, "/uri", &["preview"]).map_err(|e| McpError::resource_not_found(e.message, None))?;
            let bytes = p.bytes("png").map_err(|e| McpError::internal_error(e.message, None))?;
            Ok(ResourceContents::blob(base64::engine::general_purpose::STANDARD.encode(bytes), uri).with_mime_type("image/png"))
        }
        ["datasets", id, "manifest"] => {
            let list = op_result(svc, "data.list", json!({ "schema_version": "1" }))?;
            let found = list["registered"].as_array().and_then(|a| a.iter().find(|d| d["dataset_id"] == *id).cloned()).ok_or_else(|| McpError::resource_not_found(format!("Dataset {id} is not registered."), None))?;
            Ok(json_contents(&found, uri))
        }
        ["jobs", id, "status"] => Ok(json_contents(&op_result(svc, "job.status", json!({ "schema_version": "1", "job_id": id }))?, uri)),
        _ => Err(McpError::resource_not_found(format!("Unknown resource {uri}."), Some(json!({ "templates": "see resources/templates/list" })))),
    }
}

/// Serve MCP over stdio until the client disconnects.
pub async fn serve_stdio(svc: Service) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = ArtpopMcp::new(svc).serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

pub type Runtime = tokio::runtime::Runtime;

/// Multi-threaded runtime for the MCP server (protocol I/O stays responsive
/// while scientific work runs on blocking threads).
pub fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime")
}
