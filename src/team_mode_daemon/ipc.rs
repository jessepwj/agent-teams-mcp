use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::error::{Error, Result};

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonInfo {
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub base_dir: PathBuf,
    pub project_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub id: u64,
    pub token: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn runtime_dir(base_dir: &Path) -> PathBuf {
    base_dir.join("runtime")
}

pub fn info_path(base_dir: &Path) -> PathBuf {
    runtime_dir(base_dir).join("daemon.json")
}

pub fn read_info(base_dir: &Path) -> Result<DaemonInfo> {
    let text = std::fs::read_to_string(info_path(base_dir))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn write_info(base_dir: &Path, info: &DaemonInfo) -> Result<()> {
    let dir = runtime_dir(base_dir);
    std::fs::create_dir_all(&dir)?;
    let path = info_path(base_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(info)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    let mut len_bytes = [0_u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(Error::Other(format!(
            "daemon frame too large: {len} bytes (max {MAX_FRAME_BYTES})"
        )));
    }
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn write_frame<T: Serialize>(stream: &mut TcpStream, payload: &T) -> Result<()> {
    let body = serde_json::to_vec(payload)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(Error::Other(format!(
            "daemon frame too large: {} bytes (max {MAX_FRAME_BYTES})",
            body.len()
        )));
    }
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}
