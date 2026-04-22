use serde::{Deserialize, Serialize};

/// Room aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    pub kind: RoomKind,
    pub status: RoomStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    Main,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Active,
    Archived,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_round_trip() {
        let room = Room {
            id: "main".into(),
            team_id: Some("team-1".into()),
            kind: RoomKind::Main,
            status: RoomStatus::Active,
        };

        let json = serde_json::to_string(&room).unwrap();
        let parsed: Room = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, RoomKind::Main);
    }
}
