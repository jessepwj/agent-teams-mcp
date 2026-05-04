use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticBundleMode {
    Baked,
    Dev { root: PathBuf },
}

impl StaticBundleMode {
    pub fn from_env() -> Self {
        let fallback_root = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("web")
            .join("team-mode");
        Self::from_env_values(
            std::env::var("TEAM_MODE_WEB_DEV_BUNDLE").ok(),
            std::env::var_os("TEAM_MODE_WEB_DEV_BUNDLE_DIR").map(PathBuf::from),
            fallback_root,
        )
    }

    fn from_env_values(
        enabled_value: Option<String>,
        configured_root: Option<PathBuf>,
        fallback_root: PathBuf,
    ) -> Self {
        let enabled = enabled_value
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            })
            .unwrap_or(false);
        if !enabled {
            return Self::Baked;
        }

        let root = configured_root.unwrap_or(fallback_root);
        Self::Dev { root }
    }
}

#[derive(Debug, Clone)]
pub struct TeamModeWebState {
    pub(crate) base_dir: PathBuf,
    session_home: Option<PathBuf>,
    static_bundle: StaticBundleMode,
    pub(crate) team_service: TeamService,
    pub(crate) member_service: MemberService,
    pub(crate) room_service: RoomService,
    pub(crate) message_service: MessageService,
    pub(crate) inbox_service: InboxService,
    pub(crate) runtime_workers: RuntimeWorkerStore,
}

impl TeamModeWebState {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self::with_session_home_and_static_bundle(base_dir, None, StaticBundleMode::Baked)
    }

    pub fn with_session_home(base_dir: impl Into<PathBuf>, session_home: Option<PathBuf>) -> Self {
        Self::with_session_home_and_static_bundle(base_dir, session_home, StaticBundleMode::Baked)
    }

    pub fn with_session_home_and_static_bundle(
        base_dir: impl Into<PathBuf>,
        session_home: Option<PathBuf>,
        static_bundle: StaticBundleMode,
    ) -> Self {
        Self::build(base_dir, session_home, static_bundle, true)
    }

    /// Per-project state factory used by the multi-project web router.
    /// Forces a fresh `MessageService` rooted at the given base_dir
    /// instead of consulting `SHARED_MESSAGE_SERVICE` — that OnceLock is
    /// pinned to whatever project lazy-spawned the daemon, so for any
    /// other project its message_store points at the wrong directory and
    /// `read_main_room` returns 0 messages even when `messages.jsonl` is
    /// non-empty on disk. (BUG-12, observed 2026-05-04 immediately after
    /// the BUG-11 fix landed: lead pane shows the right cwd but the
    /// chat panel renders empty for any project that wasn't the first
    /// to start the service.)
    pub(crate) fn for_project(
        base_dir: impl Into<PathBuf>,
        session_home: Option<PathBuf>,
        static_bundle: StaticBundleMode,
    ) -> Self {
        Self::build(base_dir, session_home, static_bundle, false)
    }

    fn build(
        base_dir: impl Into<PathBuf>,
        session_home: Option<PathBuf>,
        static_bundle: StaticBundleMode,
        use_shared_message_service: bool,
    ) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::with_message_store(message_store.clone());

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        // For the daemon's startup default state, prefer the shared
        // service so writes from the web layer trigger the same
        // inbox-notifier + lead-pending-writer side effects as MCP-tool
        // writes. For per-project state (`use_shared_message_service =
        // false`), always build fresh so reads/writes hit the correct
        // project's `messages.jsonl`.
        let message_service = if use_shared_message_service {
            SHARED_MESSAGE_SERVICE.get().cloned().unwrap_or_else(|| {
                MessageService::new(
                    message_store.clone(),
                    member_store.clone(),
                    room_store.clone(),
                    team_store.clone(),
                )
            })
        } else {
            MessageService::new(
                message_store.clone(),
                member_store.clone(),
                room_store.clone(),
                team_store.clone(),
            )
        };
        let inbox_service = InboxService::new(projection_store.clone(), message_store.clone());
        // RuntimeWorkerStore reads `<base>/runtime/workers.json` — the daemon
        // updates this sidecar whenever a worker dies or its state changes,
        // so the web layer can show live "dead" status without holding the
        // orchestrator lock.
        let runtime_workers = RuntimeWorkerStore::new(base_dir.clone());

        Self {
            base_dir,
            session_home,
            static_bundle,
            team_service,
            member_service,
            room_service,
            message_service,
            inbox_service,
            runtime_workers,
        }
    }

    pub(crate) fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub(crate) fn session_home(&self) -> Option<&Path> {
        self.session_home.as_deref()
    }

    pub(crate) fn static_bundle(&self) -> &StaticBundleMode {
        &self.static_bundle
    }
}

#[cfg(test)]
mod tests {
    use super::StaticBundleMode;
    use std::path::PathBuf;

    #[test]
    fn dev_static_bundle_env_parser_disables_falsey_and_invalid_values() {
        let fallback = PathBuf::from("fallback-web-team-mode");

        for value in [
            None,
            Some("0"),
            Some("false"),
            Some("off"),
            Some("no"),
            Some("maybe"),
        ] {
            let mode = StaticBundleMode::from_env_values(
                value.map(str::to_string),
                Some(PathBuf::from("configured-web-team-mode")),
                fallback.clone(),
            );
            assert_eq!(mode, StaticBundleMode::Baked, "value {value:?}");
        }
    }

    #[test]
    fn dev_static_bundle_env_parser_uses_configured_or_fallback_root_when_enabled() {
        let fallback = PathBuf::from("fallback-web-team-mode");
        let configured = PathBuf::from("configured-web-team-mode");

        let configured_mode = StaticBundleMode::from_env_values(
            Some("true".into()),
            Some(configured.clone()),
            fallback.clone(),
        );
        assert_eq!(configured_mode, StaticBundleMode::Dev { root: configured });

        let fallback_mode =
            StaticBundleMode::from_env_values(Some("1".into()), None, fallback.clone());
        assert_eq!(fallback_mode, StaticBundleMode::Dev { root: fallback });
    }
}
