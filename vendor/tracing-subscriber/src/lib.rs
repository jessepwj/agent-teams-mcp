#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_core::{Event, Level, Metadata, Subscriber, subscriber::Interest};
use tracing_core::span::{Attributes, Id, Record};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

// Optional append-mode log file, activated via TEAM_MODE_LOG_FILE env var.
// Mirrors every stderr log line to the file for post-mortem diagnosis when
// the MCP is spawned by a host that captures stderr silently.
static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

fn log_file_handle() -> Option<&'static Mutex<File>> {
    LOG_FILE
        .get_or_init(|| {
            std::env::var("TEAM_MODE_LOG_FILE").ok().and_then(|path| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .ok()
                    .map(Mutex::new)
            })
        })
        .as_ref()
}

// ---- EnvFilter ----

#[derive(Debug, Clone)]
pub struct EnvFilter {
    min_level: Level,
    debug_targets: Vec<String>,
}

impl EnvFilter {
    pub fn new(spec: impl Into<String>) -> Self {
        parse_filter_spec(&spec.into())
    }

    pub fn try_from_env(var: &str) -> Result<Self, std::env::VarError> {
        let val = std::env::var(var)?;
        Ok(parse_filter_spec(&val))
    }

    pub fn from_default_env() -> Self {
        match std::env::var("RUST_LOG") {
            Ok(val) => parse_filter_spec(&val),
            Err(_) => EnvFilter { min_level: Level::INFO, debug_targets: Vec::new() },
        }
    }
}

fn parse_filter_spec(spec: &str) -> EnvFilter {
    // Parse simple specs like "info,agent_teams=debug" or "debug"
    let mut global_level = Level::INFO;
    let mut debug_targets: Vec<String> = Vec::new();

    for part in spec.split(',') {
        let part = part.trim();
        if let Some((target, level_str)) = part.split_once('=') {
            let level = parse_level(level_str);
            if level <= Level::DEBUG {
                debug_targets.push(target.to_string());
            }
        } else {
            global_level = parse_level(part);
        }
    }

    EnvFilter { min_level: global_level, debug_targets }
}

fn parse_level(s: &str) -> Level {
    match s.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info"  => Level::INFO,
        "warn"  => Level::WARN,
        "error" => Level::ERROR,
        _       => Level::INFO,
    }
}

impl Into<String> for EnvFilter {
    fn into(self) -> String {
        self.min_level.to_string()
    }
}

// ---- IntoEnvFilter trait ----

pub trait IntoEnvFilter {
    fn into_env_filter(self) -> EnvFilter;
}

impl IntoEnvFilter for EnvFilter {
    fn into_env_filter(self) -> EnvFilter {
        self
    }
}

impl IntoEnvFilter for &str {
    fn into_env_filter(self) -> EnvFilter {
        parse_filter_spec(self)
    }
}

impl IntoEnvFilter for String {
    fn into_env_filter(self) -> EnvFilter {
        parse_filter_spec(&self)
    }
}

// ---- FmtSubscriber ----

pub struct FmtSubscriber {
    filter: EnvFilter,
    write_to_stderr: bool,
    with_target: bool,
    with_thread_ids: bool,
}

impl Default for FmtSubscriber {
    fn default() -> Self {
        Self {
            filter: EnvFilter { min_level: Level::INFO, debug_targets: Vec::new() },
            write_to_stderr: true,
            with_target: true,
            with_thread_ids: false,
        }
    }
}

pub fn fmt() -> FmtSubscriber {
    FmtSubscriber::default()
}

impl FmtSubscriber {
    pub fn with_env_filter<F: IntoEnvFilter>(mut self, filter: F) -> Self {
        self.filter = filter.into_env_filter();
        self
    }

    pub fn with_writer<W>(mut self, _w: W) -> Self {
        // We always write to stderr; this method exists for API compatibility.
        self.write_to_stderr = true;
        self
    }

    pub fn with_target(mut self, v: bool) -> Self {
        self.with_target = v;
        self
    }

    pub fn with_thread_ids(mut self, v: bool) -> Self {
        self.with_thread_ids = v;
        self
    }

    pub fn init(self) {
        if INITIALIZED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = tracing_core::dispatcher::set_global_default(
            tracing_core::Dispatch::new(StderrSubscriber { filter: self.filter, with_target: self.with_target }),
        );
    }
}

// ---- StderrSubscriber ----

struct StderrSubscriber {
    filter: EnvFilter,
    with_target: bool,
}

impl StderrSubscriber {
    fn is_enabled(&self, meta: &Metadata<'_>) -> bool {
        let level = *meta.level();
        let target = meta.target();

        // Check if any debug_targets match this target
        for dt in &self.filter.debug_targets {
            if target.starts_with(dt.as_str()) {
                return level <= Level::DEBUG;
            }
        }

        level <= self.filter.min_level
    }
}

impl Subscriber for StderrSubscriber {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        self.is_enabled(meta)
    }

    fn new_span(&self, _attrs: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        if !self.is_enabled(meta) {
            return;
        }

        let now = {
            // Simple timestamp from SystemTime
            use std::time::{SystemTime, UNIX_EPOCH};
            let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            let secs = d.as_secs();
            let ms = d.subsec_millis();
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            format!("{h:02}:{m:02}:{s:02}.{ms:03}")
        };

        let level = meta.level();
        let target = if self.with_target { meta.target() } else { "" };

        let mut visitor = FieldVisitor { fields: String::new() };
        event.record(&mut visitor);

        let line = if self.with_target && !target.is_empty() {
            format!("{now} {level:5} {target}: {}\n", visitor.fields)
        } else {
            format!("{now} {level:5} {}\n", visitor.fields)
        };

        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Some(lock) = log_file_handle() {
            if let Ok(mut f) = lock.lock() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
    }

    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}

    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if self.is_enabled(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<tracing_core::LevelFilter> {
        Some(tracing_core::LevelFilter::DEBUG)
    }
}

// ---- Field visitor ----

struct FieldVisitor {
    fields: String,
}

impl tracing_core::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing_core::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{value:?}"));
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &tracing_core::Field, value: &str) {
        if field.name() == "message" {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(value);
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{}={value}", field.name()));
        }
    }

    fn record_i64(&mut self, field: &tracing_core::Field, value: i64) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        self.fields.push_str(&format!("{}={value}", field.name()));
    }

    fn record_u64(&mut self, field: &tracing_core::Field, value: u64) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        self.fields.push_str(&format!("{}={value}", field.name()));
    }

    fn record_bool(&mut self, field: &tracing_core::Field, value: bool) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        self.fields.push_str(&format!("{}={value}", field.name()));
    }
}
