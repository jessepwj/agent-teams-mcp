use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::domain::{MemberRecord, Message, Room, Team};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct JsonFileStore {
    root: PathBuf,
}

impl JsonFileStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|source| Error::io(&root, source))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load_teams(&self) -> Result<Vec<Team>> {
        self.read_json_vec("teams.json")
    }

    pub fn save_teams(&self, teams: &[Team]) -> Result<()> {
        self.write_json("teams.json", teams)
    }

    pub fn load_rooms(&self) -> Result<Vec<Room>> {
        self.read_json_vec("rooms.json")
    }

    pub fn save_rooms(&self, rooms: &[Room]) -> Result<()> {
        self.write_json("rooms.json", rooms)
    }

    pub fn load_members(&self) -> Result<Vec<MemberRecord>> {
        self.read_json_vec("members.json")
    }

    pub fn save_members(&self, members: &[MemberRecord]) -> Result<()> {
        self.write_json("members.json", members)
    }

    pub fn append_message(&self, message: &Message) -> Result<()> {
        let path = self.path("messages.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| Error::io(&path, source))?;
        serde_json::to_writer(&mut file, message).map_err(|source| Error::json(&path, source))?;
        file.write_all(b"\n")
            .map_err(|source| Error::io(&path, source))?;
        Ok(())
    }

    pub fn load_messages(&self) -> Result<Vec<Message>> {
        let path = self.path("messages.jsonl");
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&path).map_err(|source| Error::io(&path, source))?;
        let reader = BufReader::new(file);
        let mut messages = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|source| Error::io(&path, source))?;
            if line.trim().is_empty() {
                continue;
            }
            messages
                .push(serde_json::from_str(&line).map_err(|source| Error::json(&path, source))?);
        }
        Ok(messages)
    }

    pub fn save_messages(&self, messages: &[Message]) -> Result<()> {
        let path = self.path("messages.jsonl");
        let mut file = File::create(&path).map_err(|source| Error::io(&path, source))?;
        for message in messages {
            serde_json::to_writer(&mut file, message)
                .map_err(|source| Error::json(&path, source))?;
            file.write_all(b"\n")
                .map_err(|source| Error::io(&path, source))?;
        }
        Ok(())
    }

    fn read_json_vec<T: DeserializeOwned>(&self, name: &str) -> Result<Vec<T>> {
        let path = self.path(name);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path).map_err(|source| Error::io(&path, source))?;
        serde_json::from_str(&data).map_err(|source| Error::json(&path, source))
    }

    fn write_json<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.path(name);
        let data =
            serde_json::to_string_pretty(value).map_err(|source| Error::json(&path, source))?;
        fs::write(&path, data).map_err(|source| Error::io(&path, source))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}
