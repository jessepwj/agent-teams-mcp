use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn unique_name(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{millis}-{id}")
}

#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> io::Result<Self> {
        tempdir()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn tempdir() -> io::Result<TempDir> {
    let path = std::env::temp_dir().join(unique_name("agent-teams-tempdir"));
    fs::create_dir_all(&path)?;
    Ok(TempDir { path })
}

#[derive(Debug)]
pub struct NamedTempFile {
    path: PathBuf,
    file: File,
}

impl NamedTempFile {
    pub fn new_in(dir: impl AsRef<Path>) -> io::Result<Self> {
        let path = dir.as_ref().join(unique_name("agent-teams-tmp"));
        let file = File::create(&path)?;
        Ok(Self { path, file })
    }

    pub fn persist(self, new_path: impl AsRef<Path>) -> Result<(), PersistError> {
        fs::rename(&self.path, new_path.as_ref()).map_err(PersistError::from)
    }
}

impl Write for NamedTempFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Debug)]
pub struct PersistError {
    inner: io::Error,
}

impl From<io::Error> for PersistError {
    fn from(inner: io::Error) -> Self {
        Self { inner }
    }
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for PersistError {}
