use team_mode_native::domain::{
    AdapterKind, DeliveryStatus, ExecutionProfile, MemberKind, MessageKind,
};
use team_mode_native::service::{AddMember, CreateTeam, RoomPost, TeamModeServices};

fn services() -> (tempfile::TempDir, TeamModeServices) {
    let dir = tempfile::tempdir().expect("tempdir");
    let services = TeamModeServices::new(dir.path()).expect("services");
    (dir, services)
}

#[test]
fn team_member_execution_and_dispatch_flow() {
    let (_dir, services) = services();
    let team = services
        .teams
        .create(CreateTeam {
            id: Some("dev-core".to_string()),
            name: "Dev Core".to_string(),
            description: None,
            lead_member_id: None,
        })
        .expect("create team");

    let lead = services
        .members
        .add(AddMember {
            id: Some("lead".to_string()),
            team_id: team.id.clone(),
            name: "Lead".to_string(),
            kind: MemberKind::Lead,
            handle: "lead".to_string(),
            role_label: "lead".to_string(),
            role_description: None,
            execution: None,
        })
        .expect("add lead");
    let reviewer = services
        .members
        .add(AddMember {
            id: Some("reviewer".to_string()),
            team_id: team.id.clone(),
            name: "Reviewer".to_string(),
            kind: MemberKind::Member,
            handle: "reviewer".to_string(),
            role_label: "review".to_string(),
            role_description: None,
            execution: None,
        })
        .expect("add reviewer");

    assert_eq!(services.teams.get(&team.id).unwrap().id, "dev-core");
    assert_eq!(services.rooms.get(&team.id, "main").unwrap().id, "main");
    assert_eq!(services.members.list(&team.id).unwrap().len(), 2);

    let profile = ExecutionProfile::terminal(
        reviewer.profile.id.clone(),
        AdapterKind::ClaudeCodeTerminal,
        "claude",
        "You are @reviewer.",
    );
    services
        .members
        .set_execution_profile(&team.id, "@reviewer", profile.clone())
        .expect("set execution profile");
    assert_eq!(
        services
            .members
            .execution_profile(&team.id, "reviewer")
            .unwrap(),
        profile
    );

    let dispatch = services
        .messages
        .room_post(RoomPost {
            team_id: team.id.clone(),
            room_id: None,
            sender_member_id: lead.profile.id.clone(),
            kind: MessageKind::Dispatch,
            subject: Some("review auth".to_string()),
            body: "@reviewer please inspect auth".to_string(),
            explicit_mentions: Vec::new(),
        })
        .expect("dispatch");
    assert_eq!(dispatch.delivery_status, DeliveryStatus::Delivered);
    assert_eq!(
        dispatch.effective_recipients,
        vec![reviewer.profile.id.clone()]
    );

    let counts = services
        .inbox
        .count(&team.id, &reviewer.profile.id)
        .expect("count inbox");
    assert_eq!(counts.total, 1);
    assert_eq!(counts.unread, 1);
    assert_eq!(counts.unacked, 1);

    let read = services
        .inbox
        .read(&team.id, &reviewer.profile.id, None)
        .expect("read inbox");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].id, dispatch.id);
    let acked = services
        .inbox
        .ack(&team.id, &reviewer.profile.id, &dispatch.id)
        .expect("ack inbox");
    assert!(
        acked
            .acked_by
            .iter()
            .any(|receipt| receipt.actor == reviewer.profile.id)
    );
}

#[test]
fn dispatch_without_valid_mention_fails() {
    let (_dir, services) = services();
    let team = services
        .teams
        .create(CreateTeam {
            id: Some("dev-core".to_string()),
            name: "Dev Core".to_string(),
            description: None,
            lead_member_id: None,
        })
        .unwrap();
    let lead = services
        .members
        .add(AddMember {
            id: Some("lead".to_string()),
            team_id: team.id.clone(),
            name: "Lead".to_string(),
            kind: MemberKind::Lead,
            handle: "lead".to_string(),
            role_label: "lead".to_string(),
            role_description: None,
            execution: None,
        })
        .unwrap();

    let err = services
        .messages
        .room_post(RoomPost {
            team_id: team.id.clone(),
            room_id: None,
            sender_member_id: lead.profile.id,
            kind: MessageKind::Dispatch,
            subject: None,
            body: "please inspect auth".to_string(),
            explicit_mentions: Vec::new(),
        })
        .expect_err("dispatch without mention should fail");
    assert!(err.to_string().contains("dispatch requires"));
}

#[test]
fn thread_reply_inherits_parent_thread() {
    let (_dir, services) = services();
    let (team_id, lead_id, reviewer_id) = seed_team(&services);

    let dispatch = services
        .messages
        .room_post(RoomPost {
            team_id: team_id.clone(),
            room_id: None,
            sender_member_id: lead_id.clone(),
            kind: MessageKind::Dispatch,
            subject: None,
            body: "@reviewer please inspect auth".to_string(),
            explicit_mentions: Vec::new(),
        })
        .unwrap();

    let reply = services
        .threads
        .reply(&team_id, &dispatch.thread_id, &reviewer_id, "Looks good.")
        .expect("thread reply");
    assert_eq!(reply.kind, MessageKind::Reply);
    assert_eq!(reply.thread_id, dispatch.thread_id);
    assert_eq!(reply.reply_to, Some(dispatch.id));
    assert_eq!(reply.effective_recipients, vec![lead_id]);

    let thread = services
        .threads
        .read(&team_id, &dispatch.thread_id)
        .expect("read thread");
    assert_eq!(thread.message_ids.len(), 2);
}

#[test]
fn direct_send_reply_read_and_list_are_participant_scoped() {
    let (_dir, services) = services();
    let (team_id, lead_id, reviewer_id) = seed_team(&services);
    let outsider = services
        .members
        .add(AddMember {
            id: Some("observer".to_string()),
            team_id: team_id.clone(),
            name: "Observer".to_string(),
            kind: MemberKind::Member,
            handle: "observer".to_string(),
            role_label: "observe".to_string(),
            role_description: None,
            execution: None,
        })
        .unwrap();

    let dm = services
        .direct
        .direct_send(&team_id, &lead_id, "@reviewer", "private note")
        .expect("direct send");
    assert_eq!(dm.kind, MessageKind::Direct);
    assert_eq!(dm.room_id, "direct");
    assert_eq!(dm.effective_recipients, vec![reviewer_id.clone()]);

    let reply = services
        .direct
        .direct_reply(&team_id, &dm.thread_id, &reviewer_id, "private reply")
        .expect("direct reply");
    assert_eq!(reply.thread_id, dm.thread_id);
    assert_eq!(reply.effective_recipients, vec![lead_id.clone()]);

    let read_for_lead = services
        .direct
        .direct_read(&team_id, &dm.thread_id, &lead_id)
        .expect("direct read");
    assert_eq!(read_for_lead.len(), 2);

    let lead_threads = services.direct.direct_list(&team_id, &lead_id).unwrap();
    assert_eq!(lead_threads.len(), 1);
    assert_eq!(lead_threads[0].thread_id, dm.thread_id);

    let outsider_threads = services
        .direct
        .direct_list(&team_id, &outsider.profile.id)
        .unwrap();
    assert!(outsider_threads.is_empty());
    assert!(
        services
            .direct
            .direct_read(&team_id, &dm.thread_id, &outsider.profile.id)
            .is_err()
    );
}

fn seed_team(services: &TeamModeServices) -> (String, String, String) {
    let team = services
        .teams
        .create(CreateTeam {
            id: Some("dev-core".to_string()),
            name: "Dev Core".to_string(),
            description: None,
            lead_member_id: None,
        })
        .unwrap();
    let lead = services
        .members
        .add(AddMember {
            id: Some("lead".to_string()),
            team_id: team.id.clone(),
            name: "Lead".to_string(),
            kind: MemberKind::Lead,
            handle: "lead".to_string(),
            role_label: "lead".to_string(),
            role_description: None,
            execution: None,
        })
        .unwrap();
    let reviewer = services
        .members
        .add(AddMember {
            id: Some("reviewer".to_string()),
            team_id: team.id.clone(),
            name: "Reviewer".to_string(),
            kind: MemberKind::Member,
            handle: "reviewer".to_string(),
            role_label: "review".to_string(),
            role_description: None,
            execution: None,
        })
        .unwrap();

    (team.id, lead.profile.id, reviewer.profile.id)
}
