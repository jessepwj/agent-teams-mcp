use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Json;
use axum::routing::{get, post};
use serde_json::{Value, json};
use tempfile::tempdir;

use super::*;

#[test]
fn owner_pid_from_tree_skips_wrappers_and_fails_closed_on_missing_parent() {
    let mut tree = HashMap::new();
    tree.insert(
        10,
        ProcessRow {
            ppid: 20,
            name: "relay".into(),
        },
    );
    tree.insert(
        20,
        ProcessRow {
            ppid: 30,
            name: "cmd.exe".into(),
        },
    );
    tree.insert(
        30,
        ProcessRow {
            ppid: 40,
            name: "node.exe".into(),
        },
    );
    tree.insert(
        40,
        ProcessRow {
            ppid: 0,
            name: "claude.exe".into(),
        },
    );
    assert_eq!(owner_cc_pid_from_tree(&tree, 10), Some(30));

    let mut broken = tree.clone();
    broken.remove(&30);
    assert_eq!(owner_cc_pid_from_tree(&broken, 10), None);
}

#[test]
fn service_lock_is_idempotent() {
    let dir = tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    let lock1 = try_acquire_service_lock(&runtime_dir).unwrap();
    assert!(lock1.is_some());
    let lock2 = try_acquire_service_lock(&runtime_dir).unwrap();
    assert!(lock2.is_none());
    drop(lock1);
    assert!(try_acquire_service_lock(&runtime_dir).unwrap().is_some());
}

#[test]
fn project_registration_detection_ignores_empty_project() {
    let dir = tempdir().unwrap();
    assert!(!project_has_team_registration(dir.path()).unwrap());
}

#[test]
fn identity_cache_round_trip() {
    let dir = tempdir().unwrap();
    let cache = IdentityCache {
        session_id: "sess-1".into(),
        cached_at: "2026-05-02T00:00:00Z".into(),
        cc_pid: Some(123),
        teams: vec![TeamEntry {
            id: "demo".into(),
            pending_path: dir.path().join("demo.jsonl"),
        }],
    };
    write_identity_cache(dir.path(), &cache).unwrap();
    let loaded = read_identity_cache(dir.path(), "sess-1").unwrap().unwrap();
    assert_eq!(loaded.session_id, "sess-1");
    assert_eq!(loaded.teams.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_turn_corrupt_identity_cache_falls_through_to_my_teams() {
    let dir = tempdir().unwrap();
    let project_root = dir.path().to_path_buf();
    let session_id = "sess-1";
    let cache_path = project_root
        .join(".agent-teams")
        .join(".cc-identity.sess-1.json");
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    fs::write(&cache_path, "{not-json").unwrap();

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let pending_path = project_root.join("demo.jsonl");
    let app = Router::new().route(
        "/lead-pending/my-teams",
        get(move || {
            let hits = Arc::clone(&hits_clone);
            let pending_path = pending_path.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "cc_pid": 4321,
                    "teams": [{
                        "id": "demo",
                        "pending_path": pending_path,
                    }],
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let identity = tokio::task::spawn_blocking({
        let project_root = project_root.clone();
        let service_url = format!("http://{addr}");
        move || {
            let client = http_client().unwrap();
            let headers = HttpHeaders {
                authorization: "Bearer test-token".into(),
                owner_cc_pid: Some(4321),
            };
            resolve_mid_turn_identity(&project_root, session_id, &client, &service_url, &headers)
                .unwrap()
        }
    })
    .await
    .unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(identity.session_id, session_id);
    assert_eq!(identity.teams.len(), 1);
    assert_eq!(identity.teams[0].id, "demo");
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn my_teams_failure_exits_one_for_mid_turn_and_async_wake() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let app = Router::new().route(
        "/lead-pending/my-teams",
        get(move || {
            let hits = Arc::clone(&hits_clone);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (axum::http::StatusCode::SERVICE_UNAVAILABLE, "boom")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let service_url = format!("http://{addr}");
    let (mid_turn, async_wake) = tokio::task::spawn_blocking(move || {
        let client = http_client().unwrap();
        let headers = HttpHeaders {
            authorization: "Bearer test-token".into(),
            owner_cc_pid: Some(4321),
        };
        let mid_turn = fetch_my_teams_checked(
            &client,
            &service_url,
            &headers,
            Some("sess-1"),
            "lead-pending-mid-turn",
        )
        .unwrap_err();
        let async_wake = fetch_my_teams_checked(
            &client,
            &service_url,
            &headers,
            Some("sess-1"),
            "lead-pending-async-wake",
        )
        .unwrap_err();
        (mid_turn, async_wake)
    })
    .await
    .unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(mid_turn.code, 1);
    assert_eq!(async_wake.code, 1);
    assert!(mid_turn.message.contains("lead-pending-mid-turn"));
    assert!(async_wake.message.contains("lead-pending-async-wake"));
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_forwards_json_rpc_payloads() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen_clone = Arc::clone(&seen);
    let app = Router::new().route(
        "/mcp",
        post(move |Json(payload): Json<Value>| {
            let seen = Arc::clone(&seen_clone);
            async move {
                seen.lock().unwrap().push(payload.clone());
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload["id"].clone(),
                    "result": {"ok": true}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("http://{addr}/mcp");
    let response = tokio::task::spawn_blocking(move || -> Result<Option<Value>, String> {
        let client = http_client().map_err(|err| err.to_string())?;
        let headers = HttpHeaders {
            authorization: "Bearer test-token".into(),
            owner_cc_pid: Some(4321),
        };
        forward_json_rpc_message(
            &client,
            &url,
            &headers,
            json!({"jsonrpc":"2.0","id":7,"method":"ping"}),
        )
        .map_err(|err| err.to_string())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.unwrap()["result"]["ok"], json!(true));
    let recorded = seen.lock().unwrap();
    assert_eq!(recorded[0]["method"], json!("ping"));
    server.abort();
}

#[test]
fn reminder_rendering_matches_expected_labels() {
    let entries = vec![json!({
        "team": "demo",
        "from": "alice",
        "kind": "message",
        "text": "hello"
    })];
    let text = render_mid_turn_reminder(&entries);
    assert!(text.contains("mid-turn 团队消息"));
    assert!(text.contains("[team=demo] alice (message):"));
}
