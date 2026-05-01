use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::team_mode_web::dto::EventView;
use crate::team_mode_web::read_model;
use crate::team_mode_web::state::TeamModeWebState;

const DEFAULT_EVENT_LIMIT: usize = 100;

#[derive(Debug, Clone)]
pub struct SseConfig {
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub max_stream_duration: Option<Duration>,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            heartbeat_interval: Duration::from_secs(15),
            max_stream_duration: None,
        }
    }
}

pub fn write_headers(stream: &mut TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\n\
Connection: keep-alive\r\n\
\r\n",
    )?;
    stream.flush()
}

pub fn stream_events(
    state: Arc<TeamModeWebState>,
    mut stream: TcpStream,
    team_id: String,
    initial_cursor: Option<String>,
    config: SseConfig,
) -> io::Result<()> {
    let mut cursor = match initial_cursor {
        Some(cursor) => cursor,
        None => {
            read_model::read_events(&state, &team_id, None, Some(DEFAULT_EVENT_LIMIT))
                .map_err(|err| io::Error::other(err.message().to_string()))?
                .page
                .next_cursor
        }
    };

    write_headers(&mut stream)?;
    let started = Instant::now();
    let mut last_heartbeat = Instant::now();

    loop {
        if config
            .max_stream_duration
            .map(|max| started.elapsed() >= max)
            .unwrap_or(false)
        {
            return Ok(());
        }

        let response =
            read_model::read_events(&state, &team_id, Some(&cursor), Some(DEFAULT_EVENT_LIMIT))
                .map_err(|err| io::Error::other(err.message().to_string()))?;
        for event in &response.events {
            write_event(&mut stream, event)?;
        }
        if !response.events.is_empty() {
            cursor = response.page.next_cursor;
        }

        if last_heartbeat.elapsed() >= config.heartbeat_interval {
            write_heartbeat(&mut stream)?;
            last_heartbeat = Instant::now();
        }

        std::thread::sleep(config.poll_interval);
    }
}

fn write_event(stream: &mut TcpStream, event: &EventView) -> io::Result<()> {
    let data = serde_json::to_string(event).map_err(io::Error::other)?;
    write!(
        stream,
        "id: {}\nevent: {}\ndata: {}\n\n",
        event.cursor, event.event_type, data
    )?;
    stream.flush()
}

fn write_heartbeat(stream: &mut TcpStream) -> io::Result<()> {
    stream.write_all(b"event: heartbeat\ndata: {}\n\n")?;
    stream.flush()
}
