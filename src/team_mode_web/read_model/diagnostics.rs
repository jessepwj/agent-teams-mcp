use super::*;
use crate::team_mode::data_dir;

pub fn read_diagnostics(
    state: &TeamModeWebState,
    team_id: &str,
) -> Result<DiagnosticsResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let project_root = diagnostics_project_root(&team);
    let base_dir = state.base_dir().to_path_buf();
    let sources = diagnostics_sources(&project_root, &base_dir, &team.id);

    let lead_session = read_lead_session_diagnostics(
        &project_root,
        &base_dir,
        team.owner_cc_pid,
        state.session_home(),
    );

    Ok(DiagnosticsResponse {
        team_id: team.id.clone(),
        team_name: Some(team.name.clone()),
        cwd: team.cwd.clone(),
        generated_at: Utc::now(),
        limitations: vec![
            "These diagnostics are file/session-level observations, not per-member stdout/stderr.".into(),
            "Lead pending queue sources include the canonical per-team file plus legacy project-root/base-dir files for migration forensics.".into(),
        ],
        sources,
        lead_session,
    })
}

fn diagnostics_project_root(team: &Team) -> PathBuf {
    team.cwd
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn diagnostics_sources(
    project_root: &Path,
    base_dir: &Path,
    team_id: &str,
) -> Vec<DiagnosticsSourceView> {
    let mut seen_paths = BTreeSet::new();
    let mut sources = Vec::new();
    let mcp_log_path = preferred_diagnostics_path(project_root, base_dir, "mcp.log");
    let wake_log_path =
        preferred_diagnostics_path(project_root, base_dir, ".lead-pending-wake.log");
    let candidates = [
        (
            "lead_pending_jsonl_team",
            "Lead Pending Queue (team)",
            "file",
            data_dir::lead_pending_file_for_team(base_dir, team_id),
        ),
        (
            "lead_pending_jsonl_project_root",
            "Lead Pending Queue (project root)",
            "file",
            project_root.join("lead_pending.jsonl"),
        ),
        (
            "lead_pending_jsonl_base_dir",
            "Lead Pending Queue (base dir)",
            "file",
            base_dir.join("lead_pending.jsonl"),
        ),
        ("mcp_log", "MCP Log", "file", mcp_log_path),
        (
            "lead_pending_wake_log",
            "Lead Pending Wake Log",
            "file",
            wake_log_path,
        ),
    ];

    for (id, label, kind, path) in candidates {
        let key = path.display().to_string();
        if !seen_paths.insert(key.clone()) {
            continue;
        }
        sources.push(read_diagnostics_source(&path, id, label, kind));
    }

    sources
}

fn preferred_diagnostics_path(project_root: &Path, base_dir: &Path, file_name: &str) -> PathBuf {
    let project_candidate = project_root.join(file_name);
    let base_candidate = base_dir.join(file_name);

    match (
        fs::metadata(&project_candidate),
        fs::metadata(&base_candidate),
    ) {
        (Ok(project_meta), Ok(base_meta)) => {
            let project_modified = project_meta.modified().ok();
            let base_modified = base_meta.modified().ok();
            if project_modified >= base_modified {
                project_candidate
            } else {
                base_candidate
            }
        }
        (Ok(_), Err(_)) => project_candidate,
        (Err(_), Ok(_)) => base_candidate,
        (Err(_), Err(_)) => project_candidate,
    }
}

fn read_diagnostics_source(
    path: &Path,
    id: &str,
    label: &str,
    kind: &str,
) -> DiagnosticsSourceView {
    let metadata = fs::metadata(path).ok();
    let exists = metadata.is_some();
    let size_bytes = metadata.as_ref().map(|meta| meta.len());
    let updated_at = metadata
        .as_ref()
        .and_then(|meta| meta.modified().ok().map(DateTime::<Utc>::from));
    let preview = if exists {
        read_preview(path)
    } else {
        "not found".into()
    };

    DiagnosticsSourceView {
        id: id.into(),
        label: label.into(),
        kind: kind.into(),
        path: path.display().to_string(),
        exists,
        size_bytes,
        updated_at,
        preview,
    }
}

fn read_preview(path: &Path) -> String {
    match fs::File::open(path) {
        Ok(mut file) => {
            let mut buf = vec![0_u8; 4096];
            match file.read(&mut buf) {
                Ok(0) => "empty".into(),
                Ok(n) => {
                    let content = String::from_utf8_lossy(&buf[..n]).to_string();
                    let snippet = content.lines().take(8).collect::<Vec<_>>().join("\n");
                    crate::models::token::truncate_string(&snippet, 800)
                }
                Err(err) => format!("unavailable: {err}"),
            }
        }
        Err(err) => format!("unavailable: {err}"),
    }
}

fn read_lead_session_diagnostics(
    project_root: &Path,
    base_dir: &Path,
    owner_cc_pid: Option<u32>,
    session_home: Option<&Path>,
) -> LeadSessionDiagnosticsView {
    let mut sessions = discover_sessions(session_home, project_root);
    if sessions.is_empty() && base_dir != project_root {
        sessions = discover_sessions(session_home, base_dir);
    }
    let mapped_lead_session_id =
        owner_cc_pid.and_then(|pid| lookup_lead_session_id(project_root, base_dir, pid));
    if let Some(session_id) = mapped_lead_session_id.as_deref() {
        if !sessions
            .iter()
            .any(|session| session.session_id == session_id)
        {
            if let Some(session) =
                find_mapped_session_file(project_root, base_dir, session_id, session_home)
            {
                sessions.insert(0, session);
            }
        }
    }
    let latest = select_lead_session(&sessions, mapped_lead_session_id.as_deref());
    let discovered = latest.is_some();

    let mut view = LeadSessionDiagnosticsView {
        discovered,
        session_count: sessions.len(),
        latest_session_id: latest.map(|session| session.session_id.clone()),
        latest_modified_at: latest.and_then(|session| session.modified),
        recent_tool_calls: Vec::new(),
        token_usage: None,
        limitations: vec![
            "Lead session diagnostics sample Claude session files only; they do not expose per-member stdout/stderr.".into(),
            "Recent tool calls are truncated and derived from the latest discovered Claude session.".into(),
        ],
        source_path: latest.map(|session| session.path.display().to_string()),
    };

    if let Some(session) = latest {
        if mapped_lead_session_id.as_deref() == Some(session.session_id.as_str()) {
            view.limitations.push(
                "Lead session matched by the Stop hook's {owner_cc_pid -> claude_session_id} map."
                    .into(),
            );
        }
        match session_discovery::parse_session_file(&session.path) {
            Ok((tool_calls, token_usage)) => {
                view.recent_tool_calls = tool_calls
                    .into_iter()
                    .take(10)
                    .map(|call| LeadSessionToolCallView {
                        tool_name: call.tool_name,
                        input_summary: call.input_summary,
                        timestamp: call.timestamp,
                    })
                    .collect();
                view.token_usage = token_usage.map(|usage| LeadSessionTokenUsageView {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                    total_tokens: usage.total(),
                });
            }
            Err(err) => {
                view.limitations
                    .push(format!("Latest session could not be parsed: {err}"));
            }
        }
    }

    view
}

fn select_lead_session<'a>(
    sessions: &'a [session_discovery::SessionFile],
    mapped_lead_session_id: Option<&str>,
) -> Option<&'a session_discovery::SessionFile> {
    mapped_lead_session_id
        .and_then(|session_id| {
            sessions
                .iter()
                .find(|session| session.session_id == session_id)
        })
        .or_else(|| sessions.first())
}

fn lookup_lead_session_id(project_root: &Path, base_dir: &Path, pid: u32) -> Option<String> {
    let mut seen = BTreeSet::new();
    let candidates = [
        Some(project_root.join(".lead-sessions.json")),
        base_dir
            .parent()
            .map(|path| path.join(".lead-sessions.json")),
        Some(base_dir.join(".lead-sessions.json")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if !seen.insert(candidate.display().to_string()) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&candidate) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        if let Some(session_id) = parsed
            .get(pid.to_string())
            .and_then(|entry| entry.get("session_id"))
            .and_then(|value| value.as_str())
        {
            return Some(session_id.to_string());
        }
    }

    None
}

fn find_mapped_session_file(
    project_root: &Path,
    base_dir: &Path,
    session_id: &str,
    session_home: Option<&Path>,
) -> Option<session_discovery::SessionFile> {
    let mut seen = BTreeSet::new();
    for home in candidate_home_dirs(session_home) {
        for project in candidate_project_paths(project_root, base_dir) {
            let path = home
                .join(".claude")
                .join("projects")
                .join(session_discovery::encode_project_path(&project))
                .join(format!("{session_id}.jsonl"));
            if !seen.insert(path.display().to_string()) {
                continue;
            }
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            return Some(session_discovery::SessionFile {
                path,
                session_id: session_id.to_string(),
                modified: metadata.modified().ok().map(DateTime::<Utc>::from),
                size: metadata.len(),
            });
        }
    }

    None
}

fn candidate_home_dirs(session_home: Option<&Path>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut homes = Vec::new();
    for candidate in candidate_home_dir_values(session_home) {
        if seen.insert(candidate.display().to_string()) {
            homes.push(candidate);
        }
    }
    homes
}

fn candidate_home_dir_values(session_home: Option<&Path>) -> Vec<PathBuf> {
    if let Some(home) = session_home {
        return vec![home.to_path_buf()];
    }
    [
        dirs::home_dir(),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("USERPROFILE").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn discover_sessions(
    session_home: Option<&Path>,
    repo_path: &Path,
) -> Vec<session_discovery::SessionFile> {
    match session_home {
        Some(home) => session_discovery::discover_sessions_with_home(home, repo_path),
        None => session_discovery::discover_sessions(repo_path),
    }
}

fn candidate_project_paths(project_root: &Path, base_dir: &Path) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    for candidate in [
        project_root
            .canonicalize()
            .ok()
            .map(strip_windows_ext_prefix),
        Some(project_root.to_path_buf()),
        base_dir.parent().map(Path::to_path_buf),
        Some(base_dir.to_path_buf()),
    ]
    .into_iter()
    .flatten()
    {
        if seen.insert(candidate.display().to_string()) {
            paths.push(candidate);
        }
    }
    paths
}

fn strip_windows_ext_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}
