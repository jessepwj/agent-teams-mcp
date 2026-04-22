use crate::domain::{Message, MessageKind, Thread, ThreadStatus};
use crate::service::message_service::MessageService;
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct ThreadService {
    store: JsonFileStore,
}

impl ThreadService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn read(&self, team_id: &str, thread_id: &str) -> Result<Thread> {
        let messages = self.read_messages(team_id, thread_id)?;
        let root = messages
            .first()
            .ok_or_else(|| Error::NotFound(format!("thread: {thread_id}")))?;
        let updated_at = messages
            .last()
            .map(|message| message.created_at)
            .unwrap_or(root.created_at);
        Ok(Thread {
            id: thread_id.to_string(),
            team_id: team_id.to_string(),
            room_id: root.room_id.clone(),
            root_message_id: root.id.clone(),
            subject: root.subject.clone(),
            message_ids: messages.iter().map(|message| message.id.clone()).collect(),
            status: ThreadStatus::Open,
            created_at: root.created_at,
            updated_at,
        })
    }

    pub fn read_messages(&self, team_id: &str, thread_id: &str) -> Result<Vec<Message>> {
        let mut messages: Vec<Message> = self
            .store
            .load_messages()?
            .into_iter()
            .filter(|message| message.team_id == team_id && message.thread_id == thread_id)
            .collect();
        messages.sort_by_key(|message| message.created_at);
        if messages.is_empty() {
            return Err(Error::NotFound(format!("thread: {thread_id}")));
        }
        Ok(messages)
    }

    pub fn reply(
        &self,
        team_id: &str,
        thread_id: &str,
        sender_member_id: &str,
        body: impl Into<String>,
    ) -> Result<Message> {
        let root = self.read_messages(team_id, thread_id)?.remove(0);
        MessageService::new(self.store.clone()).append_reply(
            team_id,
            thread_id,
            sender_member_id,
            body.into(),
            root.room_id,
            MessageKind::Reply,
            Default::default(),
            Vec::new(),
        )
    }
}
