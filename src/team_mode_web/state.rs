use std::sync::OnceLock;

use crate::team_mode::runtime_workers::RuntimeWorkerStore;
use crate::team_mode::service::{
    InboxService, MemberService, MessageService, RoomService, TeamService,
};
use crate::team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};

/// Process-wide override for the `MessageService` used by the web layer.
///
/// The daemon constructs ONE `MessageService` wired with both an
/// `InboxNotifier` (so workers wake up immediately when a new message
/// arrives) and a `LeadPendingWriter` (so the lead's Stop hook gets
/// the message in its `system-reminder`). Without those side effects,
/// a message lands in the message store but the worker only sees it
/// after its next 30-second poll, and the lead never sees it via hook.
///
/// `TeamModeWebState::new` optimistically reads this `OnceLock`. If it's
/// populated (it always is in production — set by the daemon's toolset
/// on startup), the web layer borrows the same fully-wired service. If
/// it's empty (unit tests instantiating the state directly), the web
/// builds a notifier-less fallback so storage operations still work.
pub static SHARED_MESSAGE_SERVICE: OnceLock<MessageService> = OnceLock::new();

/// Daemon hook to publish its fully-wired `MessageService` for the web
/// layer to reuse. Called once during toolset construction. Subsequent
/// calls are no-ops (`OnceLock` semantics) — that's fine, the daemon
/// only builds one toolset per process.
pub fn install_shared_message_service(svc: MessageService) {
    let _ = SHARED_MESSAGE_SERVICE.set(svc);
}

#[derive(Debug, Clone)]
pub struct TeamModeWebState {
    pub(crate) base_dir: std::path::PathBuf,
    pub(crate) team_service: TeamService,
    pub(crate) member_service: MemberService,
    pub(crate) room_service: RoomService,
    pub(crate) message_service: MessageService,
    pub(crate) inbox_service: InboxService,
    pub(crate) runtime_workers: RuntimeWorkerStore,
}

impl TeamModeWebState {
    pub fn new(base_dir: impl Into<std::path::PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::with_message_store(message_store.clone());

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        // Prefer the daemon-installed shared service so writes coming from
        // the web layer trigger the same inbox-notifier + lead-pending-writer
        // side effects as MCP-tool writes. Fall back to a service without
        // those side effects only when the override isn't installed (e.g.
        // unit tests that construct `TeamModeWebState` standalone).
        let message_service = SHARED_MESSAGE_SERVICE.get().cloned().unwrap_or_else(|| {
            MessageService::new(
                message_store.clone(),
                member_store.clone(),
                room_store.clone(),
                team_store.clone(),
            )
        });
        let inbox_service = InboxService::new(projection_store.clone(), message_store.clone());
        // RuntimeWorkerStore reads `<base>/runtime/workers.json` — the daemon
        // updates this sidecar whenever a worker dies or its state changes,
        // so the web layer can show live "dead" status without holding the
        // orchestrator lock.
        let runtime_workers = RuntimeWorkerStore::new(base_dir.clone());

        Self {
            base_dir,
            team_service,
            member_service,
            room_service,
            message_service,
            inbox_service,
            runtime_workers,
        }
    }

    pub(crate) fn base_dir(&self) -> &std::path::Path {
        &self.base_dir
    }
}
