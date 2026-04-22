use std::sync::Arc;
use tokio::sync::Notify;

/// Broadcasts a wake-up signal whenever a new message is delivered to the inbox.
/// Clones are cheap (Arc) and all clones share the same underlying Notify.
#[derive(Clone, Debug, Default)]
pub struct InboxNotifier(Arc<Notify>);

impl InboxNotifier {
    pub fn new() -> Self {
        Self(Arc::new(Notify::new()))
    }

    /// Signal all waiting AgentLoops that new messages may be available.
    pub fn notify(&self) {
        self.0.notify_waiters();
    }

    /// Suspend until `notify()` is called.
    pub async fn notified(&self) {
        self.0.notified().await;
    }
}
