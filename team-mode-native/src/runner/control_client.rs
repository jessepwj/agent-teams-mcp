use std::io;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use super::protocol::{HostToRunnerFrame, OutputStream, RunnerFrame};

pub struct RunnerControlClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    token: Option<String>,
}

impl RunnerControlClient {
    pub async fn connect(host: &str, token: Option<String>) -> io::Result<Self> {
        let stream = TcpStream::connect(host).await?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            token,
        })
    }

    pub async fn send(&mut self, frame: &RunnerFrame) -> io::Result<()> {
        let value = runner_frame_to_host_ipc(frame, self.token.clone());
        send_ndjson_frame(&mut self.writer, &value).await
    }

    pub async fn recv(&mut self) -> io::Result<Option<HostToRunnerFrame>> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            let value: Value = serde_json::from_str(line.trim_end()).map_err(invalid_data)?;
            if value.get("type").and_then(Value::as_str) == Some("host/inject_input") {
                let frame = serde_json::from_value(value).map_err(invalid_data)?;
                return Ok(Some(frame));
            }
        }
    }
}

pub async fn send_ndjson_frame<T>(writer: &mut OwnedWriteHalf, frame: &T) -> io::Result<()>
where
    T: Serialize + ?Sized,
{
    let mut line = serde_json::to_vec(frame).map_err(invalid_data)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn runner_frame_to_host_ipc(frame: &RunnerFrame, token: Option<String>) -> Value {
    let (method, params) = match frame {
        RunnerFrame::Hello(frame) => (
            "runner/hello",
            json!({
                "memberId": frame.member_id,
                "runnerId": frame.runner_id,
                "pid": frame.pid,
                "state": "running"
            }),
        ),
        RunnerFrame::Heartbeat(frame) => (
            "runner/heartbeat",
            json!({
                "memberId": frame.member_id,
                "runnerId": frame.runner_id,
                "state": "running",
                "unixMs": frame.unix_ms
            }),
        ),
        RunnerFrame::Output(frame) => (
            "runner/output",
            json!({
                "memberId": frame.member_id,
                "runnerId": frame.runner_id,
                "stream": output_stream_name(&frame.stream),
                "data": frame.data
            }),
        ),
        RunnerFrame::InputInjected(frame) => (
            "runner/input_injected",
            json!({
                "memberId": frame.member_id,
                "runnerId": frame.runner_id,
                "messageId": frame.injection_id,
                "ok": frame.ok
            }),
        ),
        RunnerFrame::ChildExit(frame) => (
            "runner/child_exit",
            json!({
                "memberId": frame.member_id,
                "runnerId": frame.runner_id,
                "exitCode": frame.exit_code,
                "state": "stopped"
            }),
        ),
    };

    json!({
        "type": method,
        "params": params,
        "token": token
    })
}

fn output_stream_name(stream: &OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
        OutputStream::Pty => "pty",
    }
}
