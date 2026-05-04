use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

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
    // Pass home=None to isolate from the developer's actual ~/.claude.json so
    // the test is hermetic regardless of whether Team Mode is install-global'd.
    assert!(!project_has_team_registration_with_home(dir.path(), None).unwrap());
}

#[test]
fn project_registration_detection_accepts_global_mcp_install() {
    // BUG-3 regression: a project with no local .mcp.json / .claude config
    // should still trigger the Stop hook when the user has install-global'd
    // team-mode at user scope (~/.claude.json mcpServers.team-mode).
    let project = tempdir().unwrap();
    let home = tempdir().unwrap();
    fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"team-mode":{"command":"team_mode_service","args":["relay"],"env":{}}}}"#,
    )
    .unwrap();
    assert!(project_has_team_registration_with_home(project.path(), Some(home.path())).unwrap());
}

#[test]
fn project_registration_detection_accepts_global_hook_install() {
    // BUG-3 regression: a project with no local config but global Stop hook
    // installed should also be considered registered.
    let project = tempdir().unwrap();
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    fs::write(
        home.path().join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"team_mode_service hook async-wake"}]}]}}"#,
    )
    .unwrap();
    assert!(project_has_team_registration_with_home(project.path(), Some(home.path())).unwrap());
}

#[test]
fn read_cc_session_cwd_returns_cwd_field_from_session_file() {
    // BUG-1 fix: relay must trust the CC session file's `cwd` over its own
    // process cwd, because Claude Code on Windows can spawn MCP subprocesses
    // with cwd unrelated to the user's actual workspace.
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join(".claude/sessions")).unwrap();
    fs::write(
        home.path().join(".claude/sessions/12345.json"),
        r#"{"pid":12345,"sessionId":"abc","cwd":"E:\\projects\\foo","startedAt":1,"kind":"interactive","entrypoint":"cli"}"#,
    )
    .unwrap();
    let cwd = read_cc_session_cwd_at(home.path(), 12345).unwrap();
    assert_eq!(cwd, PathBuf::from("E:\\projects\\foo"));
}

#[test]
fn read_cc_session_cwd_returns_none_when_session_missing() {
    let home = tempdir().unwrap();
    assert!(read_cc_session_cwd_at(home.path(), 99999).is_none());
}

#[test]
fn read_cc_session_cwd_returns_none_when_cwd_field_missing() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join(".claude/sessions")).unwrap();
    fs::write(
        home.path().join(".claude/sessions/12345.json"),
        r#"{"pid":12345,"sessionId":"abc"}"#,
    )
    .unwrap();
    assert!(read_cc_session_cwd_at(home.path(), 12345).is_none());
}

#[test]
fn project_registration_detection_ignores_unrelated_global_config() {
    // Defensive: a global ~/.claude.json that has no team-mode entry must
    // still resolve to "not registered" — don't accidentally match unrelated
    // mcpServers entries.
    let project = tempdir().unwrap();
    let home = tempdir().unwrap();
    fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"some-other-server":{"command":"x"}}}"#,
    )
    .unwrap();
    assert!(!project_has_team_registration_with_home(project.path(), Some(home.path())).unwrap());
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

#[test]
fn runtime_info_candidates_use_explicit_data_dir_before_project_local() {
    let dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let candidates = runtime_info_path_candidates(dir.path(), Some(data_dir.path())).unwrap();
    assert_eq!(
        candidates[0],
        data_dir.path().join("runtime").join("http-mcp.json")
    );
    assert_eq!(
        candidates[1],
        dir.path()
            .join(".agent-teams")
            .join("runtime")
            .join("http-mcp.json")
    );
    assert_eq!(candidates.len(), 2);
}

#[test]
fn runtime_info_candidates_default_to_global_then_project_local() {
    let dir = tempdir().unwrap();
    let candidates = runtime_info_path_candidates(dir.path(), None).unwrap();
    assert!(candidates[0].ends_with(".team-mode/runtime/http-mcp.json"));
    assert_eq!(
        candidates[1],
        dir.path()
            .join(".agent-teams")
            .join("runtime")
            .join("http-mcp.json")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_lazy_spawn_polls_healthz_and_forwards_rpc() {
    let project_root = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let runtime_dir = data_dir.path().join("runtime");
    let service_pid = std::process::id();

    let port_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    port_listener.set_nonblocking(true).unwrap();
    let port = port_listener.local_addr().unwrap().port();
    let legacy_runtime_dir = project_root.path().join(".agent-teams/runtime");
    fs::create_dir_all(&legacy_runtime_dir).unwrap();
    fs::write(
        legacy_runtime_dir.join("http-mcp.json"),
        serde_json::to_string_pretty(&json!({
            "pid": service_pid,
            "host": "127.0.0.1",
            "port": port,
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "token_file": legacy_runtime_dir.join("http-mcp.token"),
            "started_at": "2026-05-02T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let health_hits = Arc::new(AtomicUsize::new(0));
    let mcp_hits = Arc::new(AtomicUsize::new(0));
    let spawn_count = Arc::new(AtomicUsize::new(0));
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let start_tx = Arc::new(Mutex::new(Some(start_tx)));
    let health_hits_clone = Arc::clone(&health_hits);
    let mcp_hits_clone = Arc::clone(&mcp_hits);
    let spawn_count_clone = Arc::clone(&spawn_count);
    let health_hits_for_server = Arc::clone(&health_hits_clone);
    let mcp_hits_for_server = Arc::clone(&mcp_hits_clone);
    let runtime_dir_for_server = runtime_dir.clone();
    let project_root_for_header = project_root.path().to_path_buf();
    let server = tokio::spawn(async move {
        let app = Router::new()
            .route(
                "/healthz",
                get(move || {
                    let health_hits = Arc::clone(&health_hits_for_server);
                    async move {
                        health_hits.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "status": "ok",
                            "version": env!("CARGO_PKG_VERSION"),
                            "uptime_seconds": 1,
                            "runtime_dir": runtime_dir_for_server.display().to_string(),
                            "lock_holder_pid": service_pid,
                        }))
                    }
                }),
            )
            .route(
                "/mcp",
                post(move |Json(payload): Json<Value>| {
                    let mcp_hits = Arc::clone(&mcp_hits_for_server);
                    async move {
                        mcp_hits.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "jsonrpc": "2.0",
                            "id": payload["id"].clone(),
                            "result": {"ok": true}
                        }))
                    }
                }),
            );
        start_rx.await.unwrap();
        let listener = tokio::net::TcpListener::from_std(port_listener).unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let runtime = tokio::task::spawn_blocking(move || {
        let client = Box::leak(Box::new(http_client().unwrap()));
        ensure_runtime_for_relay_with_spawn(
            project_root.path(),
            Some(data_dir.path()),
            client,
            move |spec| {
                spawn_count_clone.fetch_add(1, Ordering::SeqCst);
                assert_eq!(spec.runtime_dir, runtime_dir);
                assert_eq!(spec.port, port);
                let runtime_dir = spec.runtime_dir.clone();
                let start_tx = Arc::clone(&start_tx);
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(200));
                    fs::create_dir_all(&runtime_dir).unwrap();
                    fs::write(runtime_dir.join("http-mcp.token"), "relay-test-token").unwrap();
                    fs::write(
                        runtime_dir.join("http-mcp.json"),
                        serde_json::to_string_pretty(&json!({
                            "pid": service_pid,
                            "host": "127.0.0.1",
                            "port": port,
                            "url": format!("http://127.0.0.1:{port}/mcp"),
                            "token_file": runtime_dir.join("http-mcp.token"),
                            "started_at": "2026-05-02T00:00:00Z"
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                    start_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                });

                Ok(())
            },
        )
        .unwrap()
    })
    .await
    .unwrap();

    assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    assert!(health_hits.load(Ordering::SeqCst) > 0);
    assert_eq!(runtime.pid, service_pid);

    let headers = HttpHeaders {
        authorization: "Bearer relay-test-token".into(),
        owner_cc_pid: Some(4321),
        project_root: project_root_for_header,
    };
    let response = tokio::task::spawn_blocking(move || {
        let client = Box::leak(Box::new(http_client().unwrap()));
        forward_json_rpc_message(
            client,
            &runtime_url(&runtime),
            &headers,
            json!({"jsonrpc":"2.0","id":7,"method":"ping"}),
        )
        .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(response.unwrap()["result"]["ok"], json!(true));
    assert_eq!(mcp_hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_double_cc_race_survives_single_service_instance() {
    let project_root = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let runtime_dir = data_dir.path().join("runtime");
    let service_pid = std::process::id();

    let port_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    port_listener.set_nonblocking(true).unwrap();
    let port = port_listener.local_addr().unwrap().port();
    let legacy_runtime_dir = project_root.path().join(".agent-teams/runtime");
    fs::create_dir_all(&legacy_runtime_dir).unwrap();
    fs::write(
        legacy_runtime_dir.join("http-mcp.json"),
        serde_json::to_string_pretty(&json!({
            "pid": service_pid,
            "host": "127.0.0.1",
            "port": port,
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "token_file": legacy_runtime_dir.join("http-mcp.token"),
            "started_at": "2026-05-02T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let health_hits = Arc::new(AtomicUsize::new(0));
    let spawn_attempts = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let start_tx = Arc::new(Mutex::new(Some(start_tx)));
    let health_hits_for_server = Arc::clone(&health_hits);
    let runtime_dir_for_server = runtime_dir.clone();
    let server = tokio::spawn(async move {
        let app = Router::new().route(
            "/healthz",
            get(move || {
                let health_hits = Arc::clone(&health_hits_for_server);
                async move {
                    health_hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "status": "ok",
                        "version": env!("CARGO_PKG_VERSION"),
                        "uptime_seconds": 1,
                        "runtime_dir": runtime_dir_for_server.display().to_string(),
                        "lock_holder_pid": service_pid,
                    }))
                }
            }),
        );
        start_rx.await.unwrap();
        let listener = tokio::net::TcpListener::from_std(port_listener).unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let run_relay = |barrier: Arc<Barrier>, spawn_attempts: Arc<AtomicUsize>| {
        let project_root = project_root.path().to_path_buf();
        let data_dir = data_dir.path().to_path_buf();
        let start_tx = Arc::clone(&start_tx);
        tokio::task::spawn_blocking(move || {
            let client = Box::leak(Box::new(http_client().unwrap()));
            ensure_runtime_for_relay_with_spawn(
                &project_root,
                Some(&data_dir),
                client,
                move |spec| {
                    let attempt = spawn_attempts.fetch_add(1, Ordering::SeqCst);
                    let runtime_dir = spec.runtime_dir.clone();
                    let start_tx = Arc::clone(&start_tx);
                    barrier.wait();
                    if attempt == 0 {
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(200));
                            fs::create_dir_all(&runtime_dir).unwrap();
                            fs::write(runtime_dir.join("http-mcp.token"), "relay-test-token")
                                .unwrap();
                            fs::write(
                                runtime_dir.join("http-mcp.json"),
                                serde_json::to_string_pretty(&json!({
                                    "pid": service_pid,
                                    "host": "127.0.0.1",
                                    "port": port,
                                    "url": format!("http://127.0.0.1:{port}/mcp"),
                                    "token_file": runtime_dir.join("http-mcp.token"),
                                    "started_at": "2026-05-02T00:00:00Z"
                                }))
                                .unwrap(),
                            )
                            .unwrap();
                            start_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                        });
                    }

                    Ok(())
                },
            )
            .unwrap()
        })
    };

    let first = run_relay(Arc::clone(&barrier), Arc::clone(&spawn_attempts));
    let second = run_relay(Arc::clone(&barrier), Arc::clone(&spawn_attempts));

    let (first_runtime, second_runtime) = tokio::join!(first, second);
    let first_runtime = first_runtime.unwrap();
    let second_runtime = second_runtime.unwrap();
    assert_eq!(first_runtime.pid, service_pid);
    assert_eq!(second_runtime.pid, service_pid);
    assert_eq!(first_runtime.pid, second_runtime.pid);
    assert_eq!(spawn_attempts.load(Ordering::SeqCst), 2);
    assert!(health_hits.load(Ordering::SeqCst) > 0);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_identity_mismatch_fails_friendly() {
    let project_root = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let runtime_dir = data_dir.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    let service_pid = std::process::id();

    let port_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = port_listener.local_addr().unwrap().port();
    drop(port_listener);

    let token_path = runtime_dir.join("http-mcp.token");
    fs::write(&token_path, "relay-test-token").unwrap();

    let wrong_runtime_dir = tempdir().unwrap();
    let server = tokio::spawn({
        let wrong_runtime_dir = wrong_runtime_dir.path().to_path_buf();
        async move {
            let app = Router::new().route(
                "/healthz",
                get(move || {
                    let wrong_runtime_dir = wrong_runtime_dir.clone();
                    async move {
                        Json(json!({
                            "status": "ok",
                            "version": env!("CARGO_PKG_VERSION"),
                            "uptime_seconds": 1,
                            "runtime_dir": wrong_runtime_dir.display().to_string(),
                            "lock_holder_pid": service_pid + 1,
                        }))
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .unwrap();
            axum::serve(listener, app).await.unwrap();
        }
    });

    let err = thread::spawn(move || {
        let client = Box::leak(Box::new(http_client().unwrap()));
        let err = ensure_runtime_for_relay_with_spawn(
            project_root.path(),
            Some(data_dir.path()),
            client,
            move |spec| {
                assert_eq!(spec.runtime_dir, runtime_dir);
                fs::create_dir_all(&runtime_dir).unwrap();
                fs::write(runtime_dir.join("http-mcp.token"), "relay-test-token").unwrap();
                fs::write(
                    runtime_dir.join("http-mcp.json"),
                    serde_json::to_string_pretty(&json!({
                        "pid": service_pid,
                        "host": "127.0.0.1",
                        "port": port,
                        "url": format!("http://127.0.0.1:{port}/mcp"),
                        "token_file": runtime_dir.join("http-mcp.token"),
                        "started_at": "2026-05-02T00:00:00Z"
                    }))
                    .unwrap(),
                )
                .unwrap();
                Ok(())
            },
        )
        .unwrap_err();
        err.to_string()
    })
    .join()
    .unwrap();

    let err = err.to_string();
    assert!(err.contains("service identity mismatch"), "{err}");
    server.abort();
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

    let identity = thread::spawn({
        let project_root = project_root.clone();
        let service_url = format!("http://{addr}");
        move || {
            let client = http_client().unwrap();
            let headers = HttpHeaders {
                authorization: "Bearer test-token".into(),
                owner_cc_pid: Some(4321),
                project_root: project_root.clone(),
            };
            resolve_mid_turn_identity(&project_root, session_id, &client, &service_url, &headers)
                .unwrap()
        }
    })
    .join()
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
    let (mid_turn, async_wake) = thread::spawn(move || {
        let client = http_client().unwrap();
        let headers = HttpHeaders {
            authorization: "Bearer test-token".into(),
            owner_cc_pid: Some(4321),
            project_root: PathBuf::from("E:/project"),
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
    .join()
    .unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(mid_turn.code, 1);
    assert_eq!(async_wake.code, 1);
    assert!(mid_turn.message.contains("lead-pending-mid-turn"));
    assert!(async_wake.message.contains("lead-pending-async-wake"));
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_my_teams_strips_mcp_suffix_from_service_url() {
    // Regression for hook 100% non-fire bug: runtime JSON stores
    // url = "http://host:port/mcp" for stdio relay forwarding, but
    // /lead-pending/my-teams hangs off the service base, NOT under /mcp.
    // The hook used to call <url>/lead-pending/my-teams, hitting
    // /mcp/lead-pending/my-teams (404) and exiting before draining any
    // pending replies. Verify the strip suffix happens correctly.
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let mcp_hits = Arc::new(AtomicUsize::new(0));
    let mcp_hits_clone = Arc::clone(&mcp_hits);
    let app = Router::new()
        .route(
            "/lead-pending/my-teams",
            get(move || {
                let hits = Arc::clone(&hits_clone);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "cc_pid": 4321,
                        "session_id": "s",
                        "teams": [],
                    }))
                }
            }),
        )
        .route(
            "/mcp/lead-pending/my-teams",
            get(move || {
                let mcp_hits = Arc::clone(&mcp_hits_clone);
                async move {
                    mcp_hits.fetch_add(1, Ordering::SeqCst);
                    (axum::http::StatusCode::NOT_FOUND, "wrong path")
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Caller passes the relay/MCP URL (with /mcp suffix), as runtime JSON
    // stores it. fetch_my_teams must strip /mcp before appending the
    // /lead-pending/my-teams path.
    let mcp_url = format!("http://{addr}/mcp");
    let result = thread::spawn(move || {
        let client = http_client().unwrap();
        let headers = HttpHeaders {
            authorization: "Bearer test-token".into(),
            owner_cc_pid: Some(4321),
            project_root: PathBuf::from("E:/project"),
        };
        fetch_my_teams_checked(
            &client,
            &mcp_url,
            &headers,
            Some("s"),
            "lead-pending-async-wake",
        )
    })
    .join()
    .unwrap();

    assert!(
        result.is_ok(),
        "fetch_my_teams must succeed with /mcp-suffixed url, got: {result:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the correct base /lead-pending/my-teams handler must be hit"
    );
    assert_eq!(
        mcp_hits.load(Ordering::SeqCst),
        0,
        "the wrong /mcp/lead-pending/my-teams handler must NOT be hit"
    );
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
    let response = thread::spawn(move || -> Result<Option<Value>, String> {
        let client = http_client().map_err(|err| err.to_string())?;
        let headers = HttpHeaders {
            authorization: "Bearer test-token".into(),
            owner_cc_pid: Some(4321),
            project_root: PathBuf::from("E:/project"),
        };
        forward_json_rpc_message(
            &client,
            &url,
            &headers,
            json!({"jsonrpc":"2.0","id":7,"method":"ping"}),
        )
        .map_err(|err| err.to_string())
    })
    .join()
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
