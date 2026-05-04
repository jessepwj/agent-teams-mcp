use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use serde_json::json;

use crate::error::{Error, Result};
use crate::team_mode::mcp::schemas::{
    ReadResourceResult, ResourceDescriptor, TextResourceContents,
};
use crate::team_mode::service::{
    InboxService, MemberService, MessageService, RoomService, TeamService, ThreadService,
};
use crate::team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};
use crate::util::validate_name;

pub fn team_uri(team_id: &str) -> String {
    format!("team://{team_id}")
}

pub fn room_uri(team_id: &str, room_id: &str) -> String {
    format!("team://{team_id}/rooms/{room_id}")
}

pub fn thread_uri(team_id: &str, thread_id: &str) -> String {
    format!("team://{team_id}/threads/{thread_id}")
}

pub fn inbox_uri(team_id: &str, member_name: &str) -> String {
    format!("team://{team_id}/members/{member_name}/inbox")
}

pub fn session_uri(team_id: &str, member_name: &str) -> String {
    format!("team://{team_id}/members/{member_name}/session")
}

#[derive(Debug, Clone)]
pub struct TeamModeResourceRegistry {
    team_service: TeamService,
    member_service: MemberService,
    room_service: RoomService,
    message_service: MessageService,
    inbox_service: InboxService,
    thread_service: ThreadService,
    subscriptions: HashSet<String>,
}

impl TeamModeResourceRegistry {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::new(base_dir);

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        let message_service = MessageService::new(
            message_store.clone(),
            member_store.clone(),
            room_store.clone(),
            team_store.clone(),
        );
        let inbox_service = InboxService::new(projection_store.clone(), message_store.clone());
        let thread_service =
            ThreadService::new(projection_store, message_store, message_service.clone());

        Self {
            team_service,
            member_service,
            room_service,
            message_service,
            inbox_service,
            thread_service,
            subscriptions: HashSet::new(),
        }
    }

    pub fn list_resources(&self) -> Result<Vec<ResourceDescriptor>> {
        let teams = self.team_service.list()?;
        let mut thread_keys = BTreeSet::new();
        let mut resources = Vec::new();

        for team in teams {
            resources.push(ResourceDescriptor {
                uri: team_uri(&team.id),
                name: format!("Team {}", team.name),
                description: Some("Team aggregate".into()),
                mime_type: "application/json".into(),
            });

            for member in self.member_service.list_by_team(&team.id)? {
                resources.push(ResourceDescriptor {
                    uri: inbox_uri(&team.id, &member.profile.name),
                    name: format!("Inbox {}", member.profile.name),
                    description: Some(format!(
                        "Inbox projection for member {}",
                        member.profile.name
                    )),
                    mime_type: "application/json".into(),
                });
                resources.push(ResourceDescriptor {
                    uri: session_uri(&team.id, &member.profile.name),
                    name: format!("Session {}", member.profile.name),
                    description: Some(format!(
                        "Managed session state for member {}",
                        member.profile.name
                    )),
                    mime_type: "application/json".into(),
                });
            }

            // Room (always "main" per team)
            if self.room_service.get(&team.id)?.is_some() {
                resources.push(ResourceDescriptor {
                    uri: room_uri(&team.id, "main"),
                    name: format!("Room main ({})", team.id),
                    description: Some("Room transcript".into()),
                    mime_type: "application/json".into(),
                });

                for message in self.message_service.list_by_room(&team.id, "main")? {
                    if let Some(thread_id) = message.thread_id.as_deref() {
                        thread_keys.insert((team.id.clone(), thread_id.to_string()));
                    }
                }
            }
        }

        for (team_id, thread_id) in thread_keys {
            resources.push(ResourceDescriptor {
                uri: thread_uri(&team_id, &thread_id),
                name: format!("Thread {}", thread_id),
                description: Some("Thread transcript".into()),
                mime_type: "application/json".into(),
            });
        }

        Ok(resources)
    }

    pub fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        let parsed = parse_uri(uri)?;
        let value = match parsed {
            ResourceTarget::Team { team_id } => {
                let team = self
                    .team_service
                    .get(&team_id)?
                    .ok_or_else(|| Error::TeamNotFound {
                        name: team_id.clone(),
                    })?;
                json!(team)
            }
            ResourceTarget::Room { team_id, room_id } => {
                let room = self.room_service.get(&team_id)?.ok_or_else(|| {
                    Error::Other(format!("room '{room_id}' not found in team '{team_id}'"))
                })?;
                let messages = self.message_service.list_by_room(&team_id, &room_id)?;
                json!({
                    "room": room,
                    "messages": messages,
                })
            }
            ResourceTarget::Thread { team_id, thread_id } => {
                let thread = self.thread_service.read(&team_id, &thread_id)?;
                let messages = self.thread_service.read_messages(&team_id, &thread_id)?;
                json!({
                    "thread": thread,
                    "messages": messages,
                })
            }
            ResourceTarget::Inbox {
                team_id,
                member_name,
            } => {
                let inbox = self.inbox_service.peek(&team_id, &member_name, None)?;
                let counts = self.inbox_service.count(&team_id, &member_name, None)?;
                json!({
                    "inbox": inbox,
                    "counts": counts,
                })
            }
            ResourceTarget::Session {
                team_id,
                member_name,
            } => {
                let record = self
                    .member_service
                    .get(&team_id, &member_name)?
                    .ok_or_else(|| Error::MemberNotFound {
                        team: team_id.clone(),
                        member: member_name.clone(),
                    })?;
                let session_state = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.session_state.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("not-spawned");
                json!({
                    "team": team_id,
                    "name": record.profile.name,
                    "kind": record.profile.kind,
                    "status": record.profile.status,
                    "sessionState": session_state,
                    "execution": record.execution,
                })
            }
        };

        Ok(ReadResourceResult {
            contents: vec![TextResourceContents {
                uri: uri.to_string(),
                mime_type: "application/json".into(),
                text: serde_json::to_string_pretty(&value)?,
            }],
        })
    }

    pub fn subscribe(&mut self, uri: &str) -> Result<()> {
        parse_uri(uri)?;
        self.subscriptions.insert(uri.to_string());
        Ok(())
    }

    pub fn unsubscribe(&mut self, uri: &str) -> Result<()> {
        parse_uri(uri)?;
        self.subscriptions.remove(uri);
        Ok(())
    }

    pub fn subscribed_updates(&self, updated_uris: &[String]) -> Vec<String> {
        updated_uris
            .iter()
            .filter(|uri| self.subscriptions.contains(uri.as_str()))
            .cloned()
            .collect()
    }

    pub fn is_subscribed(&self, uri: &str) -> bool {
        self.subscriptions.contains(uri)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResourceTarget {
    Team {
        team_id: String,
    },
    Room {
        team_id: String,
        room_id: String,
    },
    Thread {
        team_id: String,
        thread_id: String,
    },
    Inbox {
        team_id: String,
        member_name: String,
    },
    Session {
        team_id: String,
        member_name: String,
    },
}

fn parse_uri(uri: &str) -> Result<ResourceTarget> {
    let Some(rest) = uri.strip_prefix("team://") else {
        return Err(Error::Other(format!("unsupported resource uri '{uri}'")));
    };
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(Error::Other(format!("invalid resource uri '{uri}'")));
    }

    validate_name(segments[0])?;
    match segments.as_slice() {
        [team_id] => Ok(ResourceTarget::Team {
            team_id: (*team_id).to_string(),
        }),
        [team_id, "rooms", room_id] => {
            validate_name(room_id)?;
            Ok(ResourceTarget::Room {
                team_id: (*team_id).to_string(),
                room_id: (*room_id).to_string(),
            })
        }
        [team_id, "threads", thread_id] => {
            validate_name(thread_id)?;
            Ok(ResourceTarget::Thread {
                team_id: (*team_id).to_string(),
                thread_id: (*thread_id).to_string(),
            })
        }
        [team_id, "members", name, "inbox"] => {
            validate_name(name)?;
            Ok(ResourceTarget::Inbox {
                team_id: (*team_id).to_string(),
                member_name: (*name).to_string(),
            })
        }
        [team_id, "members", name, "session"] => {
            validate_name(name)?;
            Ok(ResourceTarget::Session {
                team_id: (*team_id).to_string(),
                member_name: (*name).to_string(),
            })
        }
        _ => Err(Error::Other(format!("unsupported resource uri '{uri}'"))),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{MemberKind, MessageKind, VisibilityRule};
    use crate::team_mode::service::{
        AddMemberRequest, CreateTeamRequest, MemberService, MessageService, RoomService,
        SendMessageRequest, TeamService,
    };

    fn seed_data(base_dir: &std::path::Path) {
        let team_store = TeamStore::new(base_dir);
        let member_store = MemberStore::new(base_dir);
        let room_store = RoomStore::new(base_dir);
        let message_store = MessageStore::new(base_dir);

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        let message_service =
            MessageService::new(message_store, member_store, room_store, team_store);

        let team = team_service
            .create(CreateTeamRequest {
                id: Some("demo".into()),
                name: "demo".into(),
                description: None,
                cwd: None,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: None,
                overwrite: false,
            })
            .unwrap();
        member_service
            .add(AddMemberRequest {
                team_id: team.id.clone(),
                name: "lead".into(),
                kind: MemberKind::Lead,
                role_label: "lead".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();
        member_service
            .add(AddMemberRequest {
                team_id: team.id.clone(),
                name: "bob".into(),
                kind: MemberKind::Member,
                role_label: "worker".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();

        let room = room_service.ensure_main_room(&team.id).unwrap();
        let root = message_service
            .send(SendMessageRequest {
                team_id: team.id.clone(),
                room_id: room.id.clone(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: Some("Review".into()),
                body: "Please review @bob".into(),
                mentions: Vec::new(),
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();
        message_service
            .send(SendMessageRequest {
                team_id: team.id,
                room_id: room.id,
                sender: "bob".into(),
                kind: MessageKind::Reply,
                subject: None,
                body: "On it @lead".into(),
                mentions: Vec::new(),
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                reply_to: Some(root.id),
                thread_id: root.thread_id,
                expires_at: None,
            })
            .unwrap();
    }

    #[test]
    fn list_resources_includes_team_room_thread_and_inbox() {
        let dir = tempdir().unwrap();
        seed_data(dir.path());
        let registry = TeamModeResourceRegistry::new(dir.path());

        let resources = registry.list_resources().unwrap();
        let uris: Vec<_> = resources.into_iter().map(|r| r.uri).collect();

        assert!(uris.iter().any(|uri| uri == "team://demo"));
        assert!(uris.iter().any(|uri| uri == "team://demo/rooms/main"));
        assert!(
            uris.iter()
                .any(|uri| uri == "team://demo/members/lead/inbox")
        );
        assert!(
            uris.iter()
                .any(|uri| uri.starts_with("team://demo/threads/"))
        );
    }

    #[test]
    fn read_resource_returns_expected_payloads() {
        let dir = tempdir().unwrap();
        seed_data(dir.path());
        let registry = TeamModeResourceRegistry::new(dir.path());
        let thread_uri = registry
            .list_resources()
            .unwrap()
            .into_iter()
            .find(|r| r.uri.contains("/threads/"))
            .unwrap()
            .uri;

        let team = registry.read_resource("team://demo").unwrap();
        assert!(team.contents[0].text.contains("\"demo\""));

        let room = registry.read_resource("team://demo/rooms/main").unwrap();
        assert!(room.contents[0].text.contains("\"messages\""));

        let thread = registry.read_resource(&thread_uri).unwrap();
        assert!(thread.contents[0].text.contains("\"thread\""));

        let inbox = registry
            .read_resource("team://demo/members/bob/inbox")
            .unwrap();
        assert!(inbox.contents[0].text.contains("\"inbox\""));
    }

    #[test]
    fn subscribe_and_unsubscribe_track_state() {
        let dir = tempdir().unwrap();
        seed_data(dir.path());
        let mut registry = TeamModeResourceRegistry::new(dir.path());
        let uri = "team://demo/members/bob/inbox";

        registry.subscribe(uri).unwrap();
        assert!(registry.is_subscribed(uri));
        assert_eq!(
            registry.subscribed_updates(&[uri.to_string()]),
            vec![uri.to_string()]
        );

        registry.unsubscribe(uri).unwrap();
        assert!(!registry.is_subscribed(uri));
    }
}
