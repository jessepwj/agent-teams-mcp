use super::*;

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
    let sources = diagnostics_sources(&project_root, &base_dir);

    let lead_session = read_lead_session_diagnostics(&project_root, &base_dir);

    Ok(DiagnosticsResponse {
        team_id: team.id.clone(),
        team_name: Some(team.name.clone()),
        cwd: team.cwd.clone(),
        generated_at: Utc::now(),
        limitations: vec![
            "These diagnostics are file/session-level observations, not per-member stdout/stderr.".into(),
            "Lead pending queue sources are probed in the project root and the web data base_dir; the real source may live in either place.".into(),
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

fn diagnostics_sources(project_root: &Path, base_dir: &Path) -> Vec<DiagnosticsSourceView> {
    let mut seen_paths = BTreeSet::new();
    let mut sources = Vec::new();
    let mcp_log_path = preferred_diagnostics_path(project_root, base_dir, "mcp.log");
    let wake_log_path =
        preferred_diagnostics_path(project_root, base_dir, ".lead-pending-wake.log");
    let candidates = [
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
) -> LeadSessionDiagnosticsView {
    let mut sessions = session_discovery::discover_sessions(project_root);
    if sessions.is_empty() && base_dir != project_root {
        sessions = session_discovery::discover_sessions(base_dir);
    }
    let discovered = !sessions.is_empty();
    let latest = sessions.first();

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
