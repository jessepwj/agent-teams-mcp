use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use crate::team_mode::mcp::runtime::TeamModeMcpRuntime;
use crate::team_mode::mcp::schemas::{JsonRpcErrorResponse, JsonRpcRequest};
use crate::team_mode::service::lead_pending::PENDING_FILENAME;
use crate::team_mode::storage::TeamStore;
use crate::util::SHELL_WRAPPER_NAMES;

const ERR_PARSE: i32 = -32700;
const ERR_INVALID_REQUEST: i32 = -32600;

#[derive(Clone)]
pub struct HttpMcpState {
    runtime: Arc<Mutex<TeamModeMcpRuntime>>,
    token: Arc<str>,
    /// Service data directory — root for per-team subdirs containing
    /// `team.json` and `lead_pending.jsonl`. Used by the
    /// `/lead-pending/my-teams` endpoint to enumerate teams and resolve
    /// pending file paths the hook should watch.
    base_dir: Arc<PathBuf>,
    runtime_dir: Arc<PathBuf>,
    lock_holder_pid: u32,
    started_at: Instant,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpCallContext {
    pub owner_cc_pid: Option<u32>,
    pub caller_team: Option<String>,
    pub caller_member: Option<String>,
}

impl HttpMcpState {
    pub fn new(
        runtime: TeamModeMcpRuntime,
        token: impl Into<String>,
        base_dir: impl Into<PathBuf>,
        runtime_dir: impl Into<PathBuf>,
        lock_holder_pid: u32,
        started_at: Instant,
    ) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(runtime)),
            token: Arc::from(token.into()),
            base_dir: Arc::new(base_dir.into()),
            runtime_dir: Arc::new(runtime_dir.into()),
            lock_holder_pid,
            started_at,
        }
    }
}

pub fn router(state: HttpMcpState) -> Router {
    Router::new()
        .route("/mcp", post(post_mcp).get(get_mcp).delete(delete_mcp))
        .route("/lead-pending/my-teams", get(get_my_teams))
        .route("/healthz", get(get_healthz))
        .with_state(state)
}

pub async fn post_mcp(
    State(state): State<HttpMcpState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(status) = validate_headers(&headers, &state.token) {
        return status.into_response();
    }

    let context = call_context_from_headers(&headers);

    // Tool handlers (worker_add etc.) call `self.async_runtime.block_on(...)`
    // internally to bridge sync→async. If we ran them on the axum/tokio
    // worker thread directly, that nested `block_on` panics with
    // "Cannot start a runtime from within a runtime". Hand off to a blocking
    // thread so the synchronous tool dispatch sees no enclosing runtime.
    //
    // The Mutex is `std::sync::Mutex`; locking it inside the blocking task
    // keeps the lock duration scoped to the actual work and avoids holding
    // it across await points in the request handler.
    let runtime = state.runtime.clone();
    let join_outcome: Result<Result<Result<Option<Value>, Value>, StatusCode>, _> =
        tokio::task::spawn_blocking(move || match runtime.lock() {
            Ok(mut guard) => Ok(handle_payload(&mut guard, payload, &context)),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        })
        .await;

    let outcome = match join_outcome {
        Ok(Ok(result)) => result,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match outcome {
        Ok(Some(value)) => (
            StatusCode::OK,
            [("MCP-Protocol-Version", "2025-06-18")],
            Json(value),
        )
            .into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(value) => (StatusCode::OK, Json(value)).into_response(),
    }
}

pub async fn get_mcp(State(state): State<HttpMcpState>, headers: HeaderMap) -> Response {
    if let Err(status) = validate_headers(&headers, &state.token) {
        return status.into_response();
    }
    let stream = tokio_stream::iter([Ok::<_, std::convert::Infallible>(
        axum::response::sse::Event::default().comment("connected"),
    )])
    .throttle(std::time::Duration::from_millis(1));
    Sse::new(stream).into_response()
}

pub async fn delete_mcp(State(state): State<HttpMcpState>, headers: HeaderMap) -> Response {
    if let Err(status) = validate_headers(&headers, &state.token) {
        return status.into_response();
    }
    StatusCode::ACCEPTED.into_response()
}

pub async fn get_healthz(State(state): State<HttpMcpState>) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_seconds": state.started_at.elapsed().as_secs(),
            "runtime_dir": state.runtime_dir.display().to_string(),
            "lock_holder_pid": state.lock_holder_pid,
        })),
    )
        .into_response()
}

fn validate_headers(headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    validate_origin(headers)?;
    let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(auth) = auth.to_str() else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if auth == format!("Bearer {token}") {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn validate_origin(headers: &HeaderMap) -> Result<(), StatusCode> {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return Ok(());
    };
    let Ok(origin) = origin.to_str() else {
        return Err(StatusCode::FORBIDDEN);
    };
    let allowed = origin == "null"
        || origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:");
    if allowed {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn call_context_from_headers(headers: &HeaderMap) -> HttpCallContext {
    HttpCallContext {
        owner_cc_pid: header_str(headers, "x-team-mode-owner-cc-pid")
            .and_then(|value| value.parse::<u32>().ok()),
        caller_team: header_str(headers, "x-team-mode-team").map(str::to_string),
        caller_member: header_str(headers, "x-team-mode-worker-id")
            .or_else(|| header_str(headers, "x-team-mode-member"))
            .map(str::to_string),
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().filter(|s| !s.is_empty())
}

fn handle_payload(
    runtime: &mut TeamModeMcpRuntime,
    payload: Value,
    context: &HttpCallContext,
) -> Result<Option<Value>, Value> {
    match payload {
        Value::Array(items) => {
            let mut responses = Vec::new();
            for item in items {
                if let Some(response) = handle_one(runtime, item, context)? {
                    responses.push(response);
                }
            }
            if responses.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::Array(responses)))
            }
        }
        item => handle_one(runtime, item, context),
    }
}

fn handle_one(
    runtime: &mut TeamModeMcpRuntime,
    mut payload: Value,
    context: &HttpCallContext,
) -> Result<Option<Value>, Value> {
    inject_http_context(&mut payload, context);
    let request: JsonRpcRequest = serde_json::from_value(payload).map_err(|err| {
        json_rpc_error(
            Value::Null,
            ERR_PARSE,
            format!("invalid JSON-RPC request: {err}"),
        )
    })?;
    runtime
        .handle_request(request)
        .map(|(response, _notifications)| response)
        .map_err(|err| json_rpc_error(Value::Null, ERR_INVALID_REQUEST, err.to_string()))
}

pub fn inject_http_context(payload: &mut Value, context: &HttpCallContext) {
    let Value::Object(request) = payload else {
        return;
    };
    if request.get("method").and_then(Value::as_str) != Some("tools/call") {
        return;
    }
    let params = request
        .entry("params")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    let Some(params) = params else {
        return;
    };
    let arguments = params.entry("arguments").or_insert_with(|| json!({}));
    let Value::Object(arguments) = arguments else {
        return;
    };
    if let Some(pid) = context.owner_cc_pid {
        arguments.insert("_owner_cc_pid".into(), json!(pid));
    }
    let caller = context
        .caller_member
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("lead");
    arguments.insert("_caller_member".into(), json!(caller));
    if let Some(team) = context.caller_team.as_deref().filter(|s| !s.is_empty()) {
        arguments.insert("_caller_team".into(), json!(team));
    }
}

// ---------------------------------------------------------------------------
// Lead-pending identity endpoint
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MyTeamsQuery {
    /// Caller process PID (the hook's `process.pid`). Service walks its
    /// ancestor chain via sysinfo to find the owning Claude Code PID,
    /// then matches that PID against `team.owner_cc_pid` for each known
    /// team to decide which teams the caller owns.
    pub pid: u32,
    /// Optional Claude Code session_id. Currently echoed back for hook
    /// caching; future versions may use it for cross-validation.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `GET /lead-pending/my-teams?pid=<n>&session_id=<sid>`
///
/// Returns the list of teams whose `owner_cc_pid` matches the CC process
/// resolved from the caller's PID. Hook scripts call this once at startup
/// to learn which `<team_id>/lead_pending.jsonl` files to poll.
///
/// Response shape:
/// ```json
/// {
///   "cc_pid": 12345,
///   "session_id": "abc-...",  // echoed
///   "teams": [
///     {"id": "agent-teams-v2", "pending_path": "<base>/agent-teams-v2/lead_pending.jsonl"}
///   ]
/// }
/// ```
///
/// Auth: same Bearer token + Origin checks as `/mcp`. No fallback — if
/// sysinfo or storage fails, returns 5xx so the hook fails loudly.
pub async fn get_my_teams(
    State(state): State<HttpMcpState>,
    headers: HeaderMap,
    Query(q): Query<MyTeamsQuery>,
) -> Response {
    if let Err(status) = validate_headers(&headers, &state.token) {
        return status.into_response();
    }

    let base_dir = state.base_dir.clone();
    let result: Result<Value, String> = tokio::task::spawn_blocking(move || {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
        );
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        // Trust the caller's already-resolved CC PID. Hook/relay run the same
        // ancestor walk before calling us; re-walking from `q.pid` would
        // climb past the CC into IDE/terminal hosts (e.g. `Cursor.exe` above
        // `node.exe`), breaking owner matching. Sanity-check it isn't an
        // obvious shell wrapper to catch buggy callers loudly instead of
        // silently mis-routing.
        let cc_pid = q.pid;
        let proc = sys
            .process(Pid::from_u32(cc_pid))
            .ok_or_else(|| format!("caller-supplied cc_pid={cc_pid} not found in process list"))?;
        let name_lc = proc.name().to_string_lossy().to_lowercase();
        let stem = name_lc.trim_end_matches(".exe");
        if SHELL_WRAPPER_NAMES.contains(&stem) {
            return Err(format!(
                "caller-supplied cc_pid={cc_pid} is a shell wrapper ({stem}); \
                 caller must walk past wrappers before calling /my-teams"
            ));
        }

        let store = TeamStore::new((*base_dir).clone());
        let teams = store
            .list()
            .map_err(|err| format!("team_store.list failed: {err}"))?;

        let matched: Vec<Value> = teams
            .into_iter()
            .filter(|t| t.owner_cc_pid == Some(cc_pid))
            .map(|t| {
                let pending_path = base_dir.join(&t.id).join(PENDING_FILENAME);
                json!({
                    "id": t.id,
                    "pending_path": pending_path,
                })
            })
            .collect();

        tracing::info!(
            event = "lead_pending.my_teams_query",
            caller_pid = q.pid,
            resolved_cc_pid = cc_pid,
            session_id = ?q.session_id,
            team_count = matched.len(),
            "served /lead-pending/my-teams"
        );

        Ok(json!({
            "cc_pid": cc_pid,
            "session_id": q.session_id,
            "teams": matched,
        }))
    })
    .await
    .map_err(|err| format!("spawn_blocking join failed: {err}"))
    .and_then(|inner| inner);

    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(message) => {
            tracing::error!(
                event = "lead_pending.my_teams_failed",
                caller_pid = q.pid,
                error = %message,
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": message})),
            )
                .into_response()
        }
    }
}

fn json_rpc_error(id: Value, code: i32, message: String) -> Value {
    serde_json::to_value(JsonRpcErrorResponse::new(id, code, message)).unwrap_or_else(
        |_| json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":"internal error"}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TeamModeToolset;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use futures::executor::block_on;
    use tower::util::ServiceExt;

    #[test]
    fn injects_owner_and_worker_context_into_tools_call() {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "team_create",
                "arguments": { "name": "demo" }
            }
        });
        let context = HttpCallContext {
            owner_cc_pid: Some(1234),
            caller_team: Some("demo".into()),
            caller_member: Some("alice".into()),
        };

        inject_http_context(&mut payload, &context);

        let args = &payload["params"]["arguments"];
        assert_eq!(args["_owner_cc_pid"], json!(1234));
        assert_eq!(args["_caller_member"], json!("alice"));
        assert_eq!(args["_caller_team"], json!("demo"));
    }

    #[test]
    fn rejects_cross_origin_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            "https://example.com".parse().unwrap(),
        );
        assert_eq!(validate_origin(&headers), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn healthz_returns_expected_json_shape() {
        let dir = tempfile::tempdir().unwrap();
        let toolset =
            TeamModeToolset::new_with_project_root(dir.path(), Some(dir.path().to_path_buf()));
        let runtime = TeamModeMcpRuntime::with_tool_executor(dir.path(), Box::new(toolset));
        let app = router(HttpMcpState::new(
            runtime,
            "test-token",
            dir.path().to_path_buf(),
            dir.path().join("runtime"),
            std::process::id(),
            Instant::now(),
        ));

        block_on(async {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/healthz")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let json: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["status"], json!("ok"));
            assert_eq!(json["version"], json!(env!("CARGO_PKG_VERSION")));
            assert_eq!(
                json["runtime_dir"],
                json!(dir.path().join("runtime").display().to_string())
            );
            assert_eq!(json["lock_holder_pid"], json!(std::process::id()));
            assert!(json["uptime_seconds"].as_u64().unwrap() <= 1);
        });
    }
}
