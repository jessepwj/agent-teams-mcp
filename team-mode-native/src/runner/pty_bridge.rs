use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc;

use super::input_injector::{InjectionStrategy, inject_into_writer};

#[derive(Debug, Clone)]
pub struct PtyCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum PtyEvent {
    Output(String),
    Exit {
        exit_code: Option<i32>,
        success: bool,
    },
}

#[derive(Clone)]
pub struct PtyInputHandle {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl PtyInputHandle {
    pub fn inject(&self, text: &str, strategy: InjectionStrategy) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "pty input writer poisoned"))?;
        inject_into_writer(writer.as_mut(), text, strategy)
    }

    pub fn write_raw(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "pty input writer poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }
}

pub struct PtyBridge {
    pub input: PtyInputHandle,
    pub events: mpsc::UnboundedReceiver<PtyEvent>,
}

pub fn spawn_pty_bridge(mut spec: PtyCommandSpec) -> anyhow::Result<PtyBridge> {
    // On Windows, npm-installed scripts (claude, gemini) are not Win32 executables.
    // Wrap them with `cmd.exe /c` so CreateProcessW can launch them via the shell.
    #[cfg(windows)]
    {
        let prog_lower = spec.program.to_lowercase();
        let is_exe = prog_lower.ends_with(".exe") || prog_lower == "cmd" || prog_lower == "cmd.exe";
        if !is_exe {
            let mut new_args = vec!["/c".to_string(), spec.program.clone()];
            new_args.extend(spec.args.drain(..));
            spec.args = new_args;
            spec.program = "cmd.exe".to_string();
        }
        // Detach from the parent console (e.g. wt.exe's ConPTY) before creating our own
        // ConPTY via portable-pty. Without this, nested ConPTY fails with STATUS_NOT_IMPLEMENTED.
        // After FreeConsole the existing stdout pipe handle remains valid, so PTY output
        // written via print!() in run_pty still appears in the wt.exe tab.
        unsafe extern "system" { fn FreeConsole() -> i32; }
        unsafe { FreeConsole(); }
    }

    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: 30,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(&spec.program);
    for arg in &spec.args {
        command.arg(arg);
    }
    if let Some(cwd) = &spec.cwd {
        command.cwd(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let input = PtyInputHandle {
        writer: Arc::new(Mutex::new(writer)),
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let output_tx = tx.clone();
    thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = output_tx.send(PtyEvent::Output(text));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    thread::spawn(move || {
        let status = child.wait();
        let (exit_code, success) = match status {
            Ok(status) => (Some(status.exit_code() as i32), status.success()),
            Err(_) => (None, false),
        };
        let _ = tx.send(PtyEvent::Exit { exit_code, success });
    });

    Ok(PtyBridge { input, events: rx })
}

/// Spawns the child process with stdin piped (for injection) and stdout/stderr inherited
/// from the runner process. Use this when the runner itself runs in a real terminal (e.g.,
/// a wt.exe tab), so the child's I/O goes directly to that terminal without nested ConPTY.
pub fn spawn_direct_bridge(mut spec: PtyCommandSpec) -> anyhow::Result<PtyBridge> {
    use std::process::{Command, Stdio};

    // On Windows, npm-installed scripts are not Win32 executables. Instead of wrapping with
    // cmd.exe (which creates a grandchild that doesn't properly inherit the ConPTY from wt.exe),
    // resolve the .cmd file to the actual node.exe + script invocation so node.exe is a direct
    // child of the runner — it then inherits the wt.exe ConPTY correctly.
    #[cfg(windows)]
    {
        let prog_lower = spec.program.to_lowercase();
        let is_exe = prog_lower.ends_with(".exe") || prog_lower == "cmd" || prog_lower == "cmd.exe";
        if !is_exe {
            if let Some((node_exe, script)) = resolve_npm_cmd_to_node(&spec.program) {
                let mut new_args = vec![script];
                new_args.extend(spec.args.drain(..));
                spec.args = new_args;
                spec.program = node_exe;
            } else {
                // Fallback: cmd.exe /c (may not work in nested ConPTY)
                let mut new_args = vec!["/c".to_string(), spec.program.clone()];
                new_args.extend(spec.args.drain(..));
                spec.args = new_args;
                spec.program = "cmd.exe".to_string();
            }
        }
    }

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    // Inherit stdin so the child sees a real console TTY → interactive mode instead of print mode.
    // Injection via the PtyInputHandle is a no-op in this mode; use keyboard or MCP inbox instead.
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("direct spawn failed: {e}"))?;
    // No stdin pipe — inject_input is unsupported in native-terminal mode.
    let noop_writer: Box<dyn Write + Send> = Box::new(io::sink());
    let input = PtyInputHandle {
        writer: Arc::new(Mutex::new(noop_writer)),
    };

    let (tx, rx) = mpsc::unbounded_channel();
    thread::spawn(move || {
        let status = child.wait();
        let (exit_code, success) = match status {
            Ok(s) => (s.code(), s.success()),
            Err(_) => (None, false),
        };
        let _ = tx.send(PtyEvent::Exit { exit_code, success });
    });

    Ok(PtyBridge { input, events: rx })
}

pub fn command_spec_from_parts(
    command: &[String],
    cwd: Option<&Path>,
) -> anyhow::Result<PtyCommandSpec> {
    let Some((program, args)) = command.split_first() else {
        anyhow::bail!("command is empty");
    };
    Ok(PtyCommandSpec {
        program: program.clone(),
        args: args.to_vec(),
        cwd: cwd.map(Path::to_path_buf),
        env: Vec::new(),
    })
}

/// On Windows, resolve an npm-installed script name (e.g. "claude") to the actual
/// (node_exe_path, script_js_path) pair by parsing the associated .cmd file.
/// Returns None if resolution fails; caller falls back to cmd.exe /c.
#[cfg(windows)]
fn resolve_npm_cmd_to_node(program: &str) -> Option<(String, String)> {
    use std::path::Path;
    use std::process::Command;

    // Find the .cmd file for the program in PATH
    let cmd_name = format!("{}.cmd", program);
    let output = Command::new("where").arg(&cmd_name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let cmd_path_str = String::from_utf8(output.stdout).ok()?;
    let cmd_path_s = cmd_path_str.trim().lines().next()?.trim().to_string();
    let cmd_path = Path::new(&cmd_path_s);
    let dp0 = cmd_path.parent()?; // directory of the .cmd file

    // npm .cmd files end with a line like:
    //   endLocal & ... & "%_prog%"  "%dp0%\node_modules\...\cli.js" %*
    // We find the last line with a .js path in quotes and expand %dp0%.
    let content = std::fs::read_to_string(cmd_path).ok()?;
    let exec_line = content.lines().rev().find(|l| l.contains(".js\""))?;

    // Extract all double-quoted tokens, expanding %dp0%
    let dp0_str = dp0.to_string_lossy();
    let expanded = exec_line.replace("%dp0%", &dp0_str).replace("%DP0%", &dp0_str);
    let tokens: Vec<String> = expanded
        .split('"')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Find the .js script token (absolute path after dp0 expansion)
    let script_token = tokens.iter().find(|t| t.ends_with(".js"))?;
    let script_path = if Path::new(script_token).is_absolute() {
        script_token.clone()
    } else {
        dp0.join(script_token).to_string_lossy().into_owned()
    };

    // Find node.exe: check if node.exe exists next to the .cmd file, else search PATH
    let local_node = dp0.join("node.exe");
    let node_exe = if local_node.exists() {
        local_node.to_string_lossy().into_owned()
    } else {
        let where_out = Command::new("where").arg("node.exe").output().ok()?;
        if !where_out.status.success() {
            return None;
        }
        String::from_utf8(where_out.stdout)
            .ok()?
            .trim()
            .lines()
            .next()?
            .trim()
            .to_string()
    };

    // Verify both paths exist
    if !Path::new(&node_exe).exists() || !Path::new(&script_path).exists() {
        return None;
    }

    Some((node_exe, script_path))
}
