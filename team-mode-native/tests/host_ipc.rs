use std::net::TcpListener as StdTcpListener;

use serde_json::json;
use team_mode_native::domain::{AdapterKind, ExecutionProfile, MessageKind};
use team_mode_native::host::{
    DirectListRequest, DirectReadRequest, DirectSendRequest, ExecutionSetRequest, InboxAckRequest,
    InboxCountRequest, InboxPeekRequest, InboxReadRequest, IpcClient, LocalIpcConfig,
    MemberAddRequest, MemberAttachRequest, MemberSessionStatusRequest, MemberSpawnManagedRequest,
    MemberTailRequest, RoomListRequest, RoomPostRequest, RoomReadMessagesRequest,
    RunnerEventRequest, RunnerInjectRequest, TeamCreateRequest, TeamModeHost, ThreadReadRequest,
    run_local_ipc,
};
use tokio::sync::mpsc;

#[tokio::test]
async fn host_status_team_member_and_ipc_status_work() {
    let temp = tempfile::tempdir().unwrap();
    let host = TeamModeHost::new(temp.path());
    host.team_create(TeamCreateRequest {
        id: "dev".into(),
        name: "Dev".into(),
        description: None,
        lead_member_id: None,
    })
    .await
    .unwrap();
    host.member_add(member("dev", "lead", "lead"))
        .await
        .unwrap();

    let status = host.status().await;
    assert_eq!(status.team_count, 1);
    assert_eq!(status.member_count, 1);

    let addr = unused_addr();
    let server = tokio::spawn(run_local_ipc(
        host,
        LocalIpcConfig {
            listen: addr.clone(),
            token: None,
        },
    ));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let client = IpcClient::new(addr, None);
    let ipc_status = client.call("host/status", json!({})).await.unwrap();
    assert_eq!(ipc_status["teamCount"], 1);
    server.abort();
}

#[tokio::test]
async fn room_post_delivers_to_inbox_and_member_tail() {
    let (_temp, host) = seeded_host().await;
    let message = host
        .room_post(RoomPostRequest {
            team_id: "dev".into(),
            room_id: "main".into(),
            sender_member_id: "lead".into(),
            body: "@reviewer please check auth".into(),
            kind: Some(MessageKind::Dispatch),
            subject: None,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(message.effective_recipients, vec!["reviewer".to_string()]);

    let inbox = host
        .inbox_peek(InboxPeekRequest {
            member_id: "reviewer".into(),
            limit: None,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].message_id, message.id);

    let tail = host
        .member_tail(MemberTailRequest {
            member_id: "reviewer".into(),
            limit: None,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert!(
        tail.iter()
            .any(|line| line.data.contains("TEAM MODE MESSAGE"))
    );
}

#[tokio::test]
async fn mcp_caller_cannot_spoof_sender() {
    let (_temp, host) = seeded_host().await;
    let err = host
        .room_post(RoomPostRequest {
            team_id: "dev".into(),
            room_id: "main".into(),
            sender_member_id: "lead".into(),
            body: "spoof".into(),
            kind: None,
            subject: None,
            caller_member_id: Some("reviewer".into()),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot send as lead"));
}

#[tokio::test]
async fn local_ipc_uses_envelope_caller_not_params_caller() {
    let (_temp, host) = seeded_host().await;
    let addr = unused_addr();
    let server = tokio::spawn(run_local_ipc(
        host,
        LocalIpcConfig {
            listen: addr.clone(),
            token: None,
        },
    ));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let client = IpcClient::new(addr, None);

    let params_spoof = json!({
        "teamId": "dev",
        "roomId": "main",
        "senderMemberId": "lead",
        "callerMemberId": "lead",
        "body": "@reviewer spoof from params"
    });
    let err = client
        .call("room/post", params_spoof)
        .await
        .expect_err("callerMemberId inside params must not authenticate");
    assert!(
        err.to_string()
            .contains("requires top-level callerMemberId")
    );

    let params_sender_mismatch = json!({
        "teamId": "dev",
        "roomId": "main",
        "senderMemberId": "lead",
        "body": "@reviewer spoof from envelope"
    });
    let err = client
        .call_as("room/post", params_sender_mismatch, Some("reviewer".into()))
        .await
        .expect_err("envelope caller cannot send as another member");
    assert!(err.to_string().contains("cannot send as lead"));

    let ok = client
        .call_as(
            "room/post",
            json!({
                "teamId": "dev",
                "roomId": "main",
                "senderMemberId": "lead",
                "body": "@reviewer authorized"
            }),
            Some("lead".into()),
        )
        .await
        .unwrap();
    assert_eq!(ok["senderMemberId"], "lead");
    server.abort();
}

#[tokio::test]
async fn member_scoped_ipc_requires_matching_caller() {
    let (_temp, host) = seeded_host().await;
    host.room_post(RoomPostRequest {
        team_id: "dev".into(),
        room_id: "main".into(),
        sender_member_id: "lead".into(),
        body: "@reviewer secret".into(),
        kind: Some(MessageKind::Dispatch),
        subject: None,
        caller_member_id: None,
    })
    .await
    .unwrap();

    let addr = unused_addr();
    let server = tokio::spawn(run_local_ipc(
        host,
        LocalIpcConfig {
            listen: addr.clone(),
            token: None,
        },
    ));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let client = IpcClient::new(addr, None);

    let err = client
        .call_as(
            "inbox/peek",
            json!({ "memberId": "reviewer" }),
            Some("lead".into()),
        )
        .await
        .expect_err("members cannot inspect another member inbox");
    assert!(err.to_string().contains("cannot inbox_peek for reviewer"));

    let inbox = client
        .call_as(
            "inbox/peek",
            json!({ "memberId": "reviewer" }),
            Some("reviewer".into()),
        )
        .await
        .unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);

    let err = client
        .call_as(
            "member/tail",
            json!({ "memberId": "reviewer" }),
            Some("lead".into()),
        )
        .await
        .expect_err("members cannot tail another member log");
    assert!(err.to_string().contains("cannot member_tail for reviewer"));

    let err = client
        .call_as(
            "runner/inject",
            json!({ "memberId": "reviewer", "text": "bad" }),
            Some("lead".into()),
        )
        .await
        .expect_err("members cannot inject into another member runner");
    assert!(
        err.to_string()
            .contains("cannot runner_inject for reviewer")
    );
    server.abort();
}

#[tokio::test]
async fn runner_inject_reports_offline_and_online() {
    let (_temp, host) = seeded_host().await;
    let offline = host
        .runner_inject(RunnerInjectRequest {
            member_id: "reviewer".into(),
            text: "hello offline".into(),
            message_id: Some("msg_offline".into()),
            caller_member_id: None,
            strategy: None,
        })
        .await
        .unwrap();
    assert!(!offline.injected);

    let (tx, mut rx) = mpsc::unbounded_channel();
    host.runner_hello(
        RunnerEventRequest {
            member_id: "reviewer".into(),
            runner_id: Some("run_1".into()),
            pid: Some(123),
            child_pid: None,
            state: None,
            stream: None,
            data: None,
            message_id: None,
            ok: None,
            exit_code: None,
        },
        Some(tx),
    )
    .await
    .unwrap();

    let online = host
        .runner_inject(RunnerInjectRequest {
            member_id: "reviewer".into(),
            text: "hello online".into(),
            message_id: Some("msg_online".into()),
            caller_member_id: None,
            strategy: None,
        })
        .await
        .unwrap();
    assert!(online.injected);
    let frame = rx.recv().await.unwrap();
    assert_eq!(frame["type"], "host/inject_input");
    assert_eq!(frame["message_id"], "msg_online");
}

#[tokio::test]
async fn host_rebuilds_thread_and_inbox_from_persistent_transcript() {
    let (temp, host) = seeded_host().await;
    let message = host
        .room_post(RoomPostRequest {
            team_id: "dev".into(),
            room_id: "main".into(),
            sender_member_id: "lead".into(),
            body: "@reviewer persisted task".into(),
            kind: Some(MessageKind::Dispatch),
            subject: None,
            caller_member_id: None,
        })
        .await
        .unwrap();

    let rebuilt = TeamModeHost::new(temp.path());
    let inbox = rebuilt
        .inbox_peek(InboxPeekRequest {
            member_id: "reviewer".into(),
            limit: None,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].message_id, message.id);

    let reply = rebuilt
        .thread_reply(team_mode_native::host::ThreadReplyRequest {
            thread_id: message.thread_id,
            sender_member_id: "reviewer".into(),
            body: "persisted reply".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(reply.sender_member_id, "reviewer");
}

#[tokio::test]
async fn host_exposes_read_direct_and_managed_session_apis() {
    let (_temp, host) = seeded_host().await;

    let rooms = host
        .room_list(RoomListRequest {
            team_id: "dev".into(),
        })
        .await
        .unwrap();
    assert_eq!(rooms[0].id, "main");

    let message = host
        .room_post(RoomPostRequest {
            team_id: "dev".into(),
            room_id: "main".into(),
            sender_member_id: "lead".into(),
            body: "@reviewer please review managed APIs".into(),
            kind: Some(MessageKind::Dispatch),
            subject: None,
            caller_member_id: None,
        })
        .await
        .unwrap();

    let room_messages = host
        .room_read_messages(RoomReadMessagesRequest {
            team_id: "dev".into(),
            room_id: "main".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(room_messages.len(), 1);

    let thread = host
        .thread_read(ThreadReadRequest {
            thread_id: message.thread_id.clone(),
            team_id: None,
        })
        .await
        .unwrap();
    assert_eq!(thread.messages[0].id, message.id);

    let counts = host
        .inbox_count(InboxCountRequest {
            member_id: "reviewer".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(counts.unread, 1);
    let read = host
        .inbox_read(InboxReadRequest {
            member_id: "reviewer".into(),
            limit: Some(1),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(read[0].thread_id, message.thread_id);
    let acked = host
        .inbox_ack(InboxAckRequest {
            member_id: "reviewer".into(),
            message_id: message.id,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert!(
        acked
            .acked_by
            .iter()
            .any(|receipt| receipt.actor == "reviewer")
    );

    let dm = host
        .direct_send(DirectSendRequest {
            team_id: "dev".into(),
            sender_member_id: "lead".into(),
            recipient_member_id: "reviewer".into(),
            body: "private check".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    let dm_threads = host
        .direct_list(DirectListRequest {
            team_id: "dev".into(),
            member_id: "reviewer".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(dm_threads.len(), 1);
    let dm_messages = host
        .direct_read(DirectReadRequest {
            team_id: "dev".into(),
            thread_id: dm.thread_id,
            member_id: "reviewer".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(dm_messages[0].body, "private check");

    let profile = ExecutionProfile::terminal(
        "reviewer",
        AdapterKind::GeminiCliTerminal,
        "cmd",
        "You are @reviewer.",
    );
    host.execution_set(ExecutionSetRequest {
        member_id: "reviewer".into(),
        execution: profile,
    })
    .await
    .unwrap();

    let launch = host
        .member_spawn_managed(MemberSpawnManagedRequest {
            member_id: "reviewer".into(),
            host: Some("127.0.0.1:17891".into()),
            token_env: None,
            runner_id: Some("run_test".into()),
            dry_run: true,
            open_terminal: false,
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert!(!launch.launched);
    assert!(launch.command_line.contains("team_member_runner"));
    assert!(launch.prompt_file.unwrap().exists());
    assert!(launch.mcp_config_file.unwrap().exists());

    let session = host
        .member_session_status(MemberSessionStatusRequest {
            member_id: "reviewer".into(),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert_eq!(session.session.unwrap().state, "planned");

    let attach = host
        .member_attach(MemberAttachRequest {
            member_id: "reviewer".into(),
            host: Some("127.0.0.1:17891".into()),
            caller_member_id: None,
        })
        .await
        .unwrap();
    assert!(attach.command_line.contains("teamctl"));
}

async fn seeded_host() -> (tempfile::TempDir, TeamModeHost) {
    let temp = tempfile::tempdir().unwrap();
    let host = TeamModeHost::new(temp.path());
    host.team_create(TeamCreateRequest {
        id: "dev".into(),
        name: "Dev".into(),
        description: None,
        lead_member_id: None,
    })
    .await
    .unwrap();
    host.member_add(member("dev", "lead", "lead"))
        .await
        .unwrap();
    host.member_add(member("dev", "reviewer", "reviewer"))
        .await
        .unwrap();
    (temp, host)
}

fn member(team_id: &str, id: &str, handle: &str) -> MemberAddRequest {
    MemberAddRequest {
        team_id: team_id.into(),
        id: id.into(),
        handle: handle.into(),
        name: id.into(),
        kind: None,
        role_label: None,
        role_description: None,
        execution: None,
    }
}

fn unused_addr() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}
