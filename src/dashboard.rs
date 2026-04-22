//! Minimal web dashboard for real-time team status monitoring.
//!
//! Provides JSON REST endpoints and an SSE event stream.
//! Gated behind the `dashboard` cargo feature.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/teams` | List all teams |
//! | GET | `/api/teams/{team}/status` | Team status (members, tasks) |
//! | GET | `/api/teams/{team}/events` | SSE stream of team events |
//!
//! # Usage
//!
//! ```ignore
//! use agent_teams::dashboard;
//!
//! let orch = TeamOrchestrator::builder().build()?;
//! let app = dashboard::router(orch);
//! let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
//! axum::serve(listener, app).await?;
//! ```

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Serialize;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::IntervalStream;
use tower_http::cors::CorsLayer;
use tracing::warn;

use crate::models::TaskStatus;
use crate::orchestrator::TeamOrchestrator;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

type AppState = Arc<TeamOrchestrator>;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TeamsListResponse {
    teams: Vec<String>,
}

#[derive(Serialize)]
struct TeamStatusResponse {
    team: String,
    members: Vec<MemberStatus>,
    tasks: TaskSummary,
}

#[derive(Serialize)]
struct MemberStatus {
    name: String,
    agent_type: String,
    alive: bool,
}

#[derive(Serialize)]
struct TaskSummary {
    total: usize,
    pending: usize,
    in_progress: usize,
    completed: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// Typed error that maps to proper HTTP status codes.
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn from_crate_error(e: crate::Error) -> Self {
        let status = match &e {
            crate::Error::TeamNotFound { .. } => StatusCode::NOT_FOUND,
            crate::Error::MemberNotFound { .. } => StatusCode::NOT_FOUND,
            crate::Error::TaskNotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            body: ErrorResponse {
                error: e.to_string(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Router constructors
// ---------------------------------------------------------------------------

/// Build an [`axum::Router`] serving the dashboard API with permissive CORS.
///
/// For production use, prefer [`router_with_cors`] to configure specific allowed origins.
pub fn router(orchestrator: TeamOrchestrator) -> Router {
    router_with_cors(orchestrator, CorsLayer::permissive())
}

/// Build an [`axum::Router`] serving the dashboard API with a custom CORS policy.
pub fn router_with_cors(orchestrator: TeamOrchestrator, cors: CorsLayer) -> Router {
    let state: AppState = Arc::new(orchestrator);

    Router::new()
        .route("/api/teams", get(list_teams))
        .route("/api/teams/{team}/status", get(team_status))
        .route("/api/teams/{team}/events", get(team_events))
        .layer(cors)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_teams(State(orch): State<AppState>) -> Result<Json<TeamsListResponse>, ApiError> {
    let teams = orch
        .list_teams()
        .await
        .map_err(ApiError::from_crate_error)?;
    Ok(Json(TeamsListResponse { teams }))
}

async fn team_status(
    State(orch): State<AppState>,
    Path(team): Path<String>,
) -> Result<Json<TeamStatusResponse>, ApiError> {
    let config = orch
        .read_team(&team)
        .await
        .map_err(ApiError::from_crate_error)?;
    let alive_map = orch
        .are_alive(&team)
        .await
        .map_err(ApiError::from_crate_error)?;

    let members: Vec<MemberStatus> = config
        .members
        .iter()
        .map(|m| MemberStatus {
            name: m.name().to_string(),
            agent_type: m.agent_type().to_string(),
            alive: alive_map.get(m.name()).copied().unwrap_or(false),
        })
        .collect();

    let tasks = orch
        .list_tasks(&team, None)
        .await
        .map_err(ApiError::from_crate_error)?;

    let pending = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .count();
    let in_progress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let completed = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();

    Ok(Json(TeamStatusResponse {
        team,
        members,
        tasks: TaskSummary {
            total: tasks.len(),
            pending,
            in_progress,
            completed,
        },
    }))
}

async fn team_events(
    State(orch): State<AppState>,
    Path(team): Path<String>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    // Poll team status every 2 seconds and emit SSE events.
    // The stream terminates when the team no longer exists (e.g., deleted),
    // sending a final "team_deleted" event so clients can handle it.
    let interval = IntervalStream::new(tokio::time::interval(Duration::from_secs(2)));

    let stream = interval
        .map(move |_| {
            let orch = orch.clone();
            let team = team.clone();
            (orch, team)
        })
        .then(|(orch, team)| async move {
            // Check if team still exists; terminate stream if not
            match orch.read_team(&team).await {
                Err(_) => None, // Team gone — signal end
                Ok(_) => {
                    let status = build_status_snapshot(&orch, &team).await;
                    match serde_json::to_string(&status) {
                        Ok(data) => Some(Event::default().event("status").data(data)),
                        Err(e) => {
                            warn!(team = %team, error = %e, "SSE: failed to serialize status");
                            Some(Event::default().event("error").data(e.to_string()))
                        }
                    }
                }
            }
        })
        // take_while stops the stream when the first None is produced
        .take_while(|item| item.is_some())
        .map(|item| Ok::<_, Infallible>(item.expect("take_while guarantees Some")));

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn build_status_snapshot(orch: &TeamOrchestrator, team: &str) -> TeamStatusResponse {
    let config = match orch.read_team(team).await {
        Ok(c) => c,
        Err(e) => {
            warn!(team, error = %e, "SSE snapshot: failed to read team config");
            return TeamStatusResponse {
                team: team.to_string(),
                members: vec![],
                tasks: TaskSummary {
                    total: 0,
                    pending: 0,
                    in_progress: 0,
                    completed: 0,
                },
            };
        }
    };

    let alive_map = match orch.are_alive(team).await {
        Ok(m) => m,
        Err(e) => {
            warn!(team, error = %e, "SSE snapshot: failed to get alive status");
            std::collections::HashMap::new()
        }
    };
    let members: Vec<MemberStatus> = config
        .members
        .iter()
        .map(|m| MemberStatus {
            name: m.name().to_string(),
            agent_type: m.agent_type().to_string(),
            alive: alive_map.get(m.name()).copied().unwrap_or(false),
        })
        .collect();

    let tasks = match orch.list_tasks(team, None).await {
        Ok(t) => t,
        Err(e) => {
            warn!(team, error = %e, "SSE snapshot: failed to list tasks");
            Vec::new()
        }
    };
    let pending = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .count();
    let in_progress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let completed = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();

    TeamStatusResponse {
        team: team.to_string(),
        members,
        tasks: TaskSummary {
            total: tasks.len(),
            pending,
            in_progress,
            completed,
        },
    }
}
