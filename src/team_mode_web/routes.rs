use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

const INDEX_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/index.html"
));
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/web/team-mode/app.js"));
const STYLES_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/web/team-mode/styles.css"
));

use super::error::{ErrorBody, StatusCode, WebError};
use super::read_model;
use super::state::TeamModeWebState;

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

    // POST routes — kept apart from the GET match so a typo in a GET path
    // doesn't accidentally accept a body it didn't ask for.
    if method == "POST" {
        let result = match segments.as_slice() {
            ["api", "teams", team, "rooms", "main", "messages"] => {
                read_model::post_main_room_message(state, team, body)
                    .map(|body| WebResponse::json(StatusCode::Created, &body))
            }
            _ => Err(WebError::not_found(format!(
                "route 'POST {path}' not found"
            ))),
        };
        return match result {
            Ok(response) => response,
            Err(err) => error_response(err),
        };
    }

    if method != "GET" {
        return WebResponse::json(
            StatusCode::MethodNotAllowed,
            &ErrorBody {
                error: format!(
                    "method '{method}' not supported on '{path}'; team-mode-web accepts \
                     GET for reads and POST for sending messages"
                ),
            },
        );
    }

    let result = match segments.as_slice() {
        [] => return WebResponse::html(StatusCode::Ok, INDEX_HTML),
        ["index.html"] => return WebResponse::html(StatusCode::Ok, INDEX_HTML),
        ["app.js"] => return WebResponse::javascript(StatusCode::Ok, APP_JS),
        ["styles.css"] => return WebResponse::css(StatusCode::Ok, STYLES_CSS),
        ["healthz"] => return WebResponse::text(StatusCode::Ok, "ok"),
        ["api", "teams"] => {
            read_model::list_teams(state).map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team] => {
            read_model::read_team(state, team).map(|body| WebResponse::json(StatusCode::Ok, &body))
        }
        ["api", "teams", team, "diagnostics"] => read_model::read_diagnostics(state, team)
            .map(|body| WebResponse::json(StatusCode::Ok, &body)),
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

fn error_response(err: WebError) -> WebResponse {
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
