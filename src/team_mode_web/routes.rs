use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.processed.html"));
const BUNDLE_REVISION: &str = env!("TEAM_MODE_WEB_BUNDLE_REVISION");
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/web/team-mode/app.js"));
const APP_STATE_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-state.js"
));
const APP_API_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-api.js"
));
const APP_UTILS_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-utils.js"
));
const APP_DIAGNOSTICS_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-diagnostics.js"
));
const APP_RENDER_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-render.js"
));
const APP_CONVERSATION_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-conversation.js"
));
const APP_DASHBOARD_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-dashboard.js"
));
const APP_DASHBOARD_RENDER_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/app-dashboard-render.js"
));
const STYLES_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/styles.css"
));
const DASHBOARD_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/dashboard.css"
));
const JS_ASSETS: &[(&str, &str)] = &[
    ("app.js", APP_JS),
    ("app-state.js", APP_STATE_JS),
    ("app-api.js", APP_API_JS),
    ("app-utils.js", APP_UTILS_JS),
    ("app-diagnostics.js", APP_DIAGNOSTICS_JS),
    ("app-render.js", APP_RENDER_JS),
    ("app-conversation.js", APP_CONVERSATION_JS),
    ("app-dashboard.js", APP_DASHBOARD_JS),
    ("app-dashboard-render.js", APP_DASHBOARD_RENDER_JS),
];

use super::error::{ErrorBody, StatusCode, WebError};
use super::read_model;
use super::state::{StaticBundleMode, TeamModeWebState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebResponse {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl WebResponse {
    pub fn text(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn html(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn javascript(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "application/javascript; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn css(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/css; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn json<T: Serialize>(status: StatusCode, body: &T) -> Self {
        match serde_json::to_vec(body) {
            Ok(body) => Self {
                status,
                content_type: "application/json; charset=utf-8",
                body,
            },
            Err(err) => error_response(WebError::internal(err.to_string())),
        }
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

pub fn handle_request(state: &Arc<TeamModeWebState>, method: &str, target: &str) -> WebResponse {
    handle_request_with_body(state, method, target, &[])
}

/// Body-aware variant. The HTTP layer parses the request body (Content-Length
/// bytes after the headers) and passes it through. GET handlers ignore the
/// `body` slice; mutating routes (POST) read JSON from it.
pub fn handle_request_with_body(
    state: &Arc<TeamModeWebState>,
    method: &str,
    target: &str,
    body: &[u8],
) -> WebResponse {
    let (path, query) = split_target(target);
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if method != "GET" {
        if method == "POST"
            && matches!(
                segments.as_slice(),
                ["api", "teams", _, "rooms", "main", "messages"]
            )
        {
            let result = match segments.as_slice() {
                ["api", "teams", team, "rooms", "main", "messages"] => {
                    read_model::post_main_room_message(state, team, body)
                        .map(|body| WebResponse::json(StatusCode::Created, &body))
                }
                _ => unreachable!("POST message route was checked before dispatch"),
            };
            return match result {
                Ok(response) => response,
                Err(err) => error_response(err),
            };
        }

        if is_known_get_only_api_path(segments.as_slice()) {
            return method_not_allowed_response(method, path);
        }

        if is_common_write_method(method) {
            return error_response(WebError::not_found(format!(
                "route '{method} {path}' not found"
            )));
        }

        return method_not_allowed_response(method, path);
    }

    if let Some(response) = javascript_asset(state, path) {
        return match response {
            Ok(response) => response,
            Err(err) => error_response(err),
        };
    }

    let result = match segments.as_slice() {
        [] => return static_html_response(state),
        ["index.html"] => return static_html_response(state),
        ["styles.css"] => return static_css_response(state, "styles.css"),
        ["dashboard.css"] => return static_css_response(state, "dashboard.css"),
        ["healthz"] => return WebResponse::text(StatusCode::Ok, "ok"),
        ["api", "bundle-revision"] => {
            return WebResponse::json(StatusCode::Ok, &bundle_revision_response(state));
        }
        ["api", "teams"] => {
            read_model::list_teams(state).map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team] => {
            read_model::read_team(state, team).map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team, "diagnostics"] => read_model::read_diagnostics(state, team)
            .map(|body| WebResponse::json(StatusCode::Ok, &body)),
        ["api", "teams", team, "events"] => {
            let params = parse_query(query);
            read_model::read_events(
                state,
                team,
                params.get("cursor").map(String::as_str),
                params
                    .get("limit")
                    .and_then(|value| value.parse::<usize>().ok()),
            )
            .map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team, "rooms", "main"] => {
            let params = parse_query(query);
            let limit = params
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(100);
            read_model::read_main_room(
                state,
                team,
                limit,
                params.get("sender").map(String::as_str),
                params.get("mentioned").map(String::as_str),
            )
            .map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team, "members"] => read_model::list_members(state, team)
            .map(|body| WebResponse::json(StatusCode::Ok, &body)),
        ["api", "teams", team, "members", name] => read_model::read_member(state, team, name)
            .map(|body| WebResponse::json(StatusCode::Ok, &body)),
        ["api", "teams", team, "members", name, "activity"] => {
            read_model::read_member_activity(state, team, name)
                .map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team, "members", name, "conversation"] => {
            read_model::read_member_conversation(state, team, name)
                .map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        _ => Err(WebError::not_found(format!("route '{path}' not found"))),
    };

    match result {
        Ok(response) => response,
        Err(err) => error_response(err),
    }
}

fn javascript_asset(
    state: &Arc<TeamModeWebState>,
    path: &str,
) -> Option<Result<WebResponse, WebError>> {
    let asset_name = path.trim_start_matches('/');
    if asset_name.contains('/') || !asset_name.ends_with(".js") {
        return None;
    }
    let baked = JS_ASSETS
        .iter()
        .find_map(|(name, body)| (*name == asset_name).then_some(*body))?;
    Some(match state.static_bundle() {
        StaticBundleMode::Baked => Ok(WebResponse::javascript(StatusCode::Ok, baked)),
        StaticBundleMode::Dev { root } => read_dev_static(root, asset_name)
            .map(|body| WebResponse::javascript(StatusCode::Ok, body)),
    })
}

fn static_html_response(state: &Arc<TeamModeWebState>) -> WebResponse {
    match state.static_bundle() {
        StaticBundleMode::Baked => WebResponse::html(StatusCode::Ok, INDEX_HTML),
        StaticBundleMode::Dev { root } => match read_dev_static(root, "index.html") {
            Ok(body) => WebResponse::html(
                StatusCode::Ok,
                body.replace("__TEAM_MODE_WEB_BUNDLE_REVISION__", "dev"),
            ),
            Err(err) => error_response(err),
        },
    }
}

fn static_css_response(state: &Arc<TeamModeWebState>, asset_name: &str) -> WebResponse {
    match state.static_bundle() {
        StaticBundleMode::Baked => match asset_name {
            "styles.css" => WebResponse::css(StatusCode::Ok, STYLES_CSS),
            "dashboard.css" => WebResponse::css(StatusCode::Ok, DASHBOARD_CSS),
            _ => error_response(WebError::not_found(format!(
                "route '/{asset_name}' not found"
            ))),
        },
        StaticBundleMode::Dev { root } => match asset_name {
            "styles.css" | "dashboard.css" => match read_dev_static(root, asset_name) {
                Ok(body) => WebResponse::css(StatusCode::Ok, body),
                Err(err) => error_response(err),
            },
            _ => error_response(WebError::not_found(format!(
                "route '/{asset_name}' not found"
            ))),
        },
    }
}

fn read_dev_static(root: &std::path::Path, asset_name: &str) -> Result<String, WebError> {
    std::fs::read_to_string(root.join(asset_name)).map_err(|err| {
        WebError::internal(format!(
            "failed to read dev static asset '{}': {err}",
            root.join(asset_name).display()
        ))
    })
}

fn bundle_revision_response(state: &Arc<TeamModeWebState>) -> BundleRevisionResponse {
    BundleRevisionResponse {
        bundle_revision: match state.static_bundle() {
            StaticBundleMode::Baked => BUNDLE_REVISION.into(),
            StaticBundleMode::Dev { .. } => "dev".into(),
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleRevisionResponse {
    bundle_revision: String,
}

fn is_known_get_only_api_path(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["api", "bundle-revision"]
            | ["api", "teams"]
            | ["api", "teams", _]
            | ["api", "teams", _, "diagnostics"]
            | ["api", "teams", _, "events"]
            | ["api", "teams", _, "events", "stream"]
            | ["api", "teams", _, "rooms", "main"]
            | ["api", "teams", _, "members"]
            | ["api", "teams", _, "members", _]
            | ["api", "teams", _, "members", _, "activity"]
            | ["api", "teams", _, "members", _, "conversation"]
    )
}

fn is_common_write_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

fn method_not_allowed_response(method: &str, path: &str) -> WebResponse {
    WebResponse::json(
        StatusCode::MethodNotAllowed,
        &ErrorBody {
            error: format!(
                "method '{method}' not supported on '{path}'; team-mode-web accepts \
                 GET for reads and POST for sending messages"
            ),
        },
    )
}

pub(crate) fn validate_events_cursor(
    state: &Arc<TeamModeWebState>,
    team: &str,
    cursor: Option<&str>,
) -> Result<(), WebError> {
    read_model::read_events(state, team, cursor, Some(1)).map(|_| ())
}

pub(crate) fn error_response(err: WebError) -> WebResponse {
    WebResponse::json(
        err.status_code(),
        &ErrorBody {
            error: err.message().to_string(),
        },
    )
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    target
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((target, None))
}

fn parse_query(query: Option<&str>) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let Some(query) = query else {
        return params;
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(key.to_string(), value.to_string());
    }
    params
}
