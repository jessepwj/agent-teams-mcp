//! Checkpoint views: list, detail, and diff.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::models::checkpoint::Checkpoint;
use crate::models::token::estimate_cost;
use crate::tui::app::{App, CheckpointView};

/// Draw the checkpoints tab.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    match &app.checkpoint_view {
        CheckpointView::List => draw_list(frame, app, area),
        CheckpointView::Detail(idx) => draw_detail(frame, app, area, *idx),
    }
}

/// Draw the checkpoint list view.
fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Checkpoints (Enter: detail) ");

    if let Some(ref err) = app.checkpoint_data.error {
        let msg = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let checkpoints = &app.checkpoint_data.checkpoints;
    if checkpoints.is_empty() {
        let msg =
            Paragraph::new("No checkpoints found. Create one with: agent-teams checkpoint create")
                .style(Style::default().fg(Color::DarkGray))
                .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Commit"),
        Cell::from("Branch"),
        Cell::from("Agent"),
        Cell::from("Time"),
        Cell::from("Files"),
        Cell::from("Tasks"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = checkpoints
        .iter()
        .enumerate()
        .map(|(i, ckpt)| {
            let sha_short = if ckpt.commit_sha.len() > 7 {
                &ckpt.commit_sha[..7]
            } else {
                &ckpt.commit_sha
            };
            let time = ckpt.created_at.format("%Y-%m-%d %H:%M").to_string();
            let files = ckpt.files.len().to_string();
            let tasks = ckpt.tasks.len().to_string();

            let row_style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(sha_short.to_string()),
                Cell::from(ckpt.branch.clone()),
                Cell::from(ckpt.session.agent_name.clone()),
                Cell::from(time),
                Cell::from(files),
                Cell::from(tasks),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(16),
            Constraint::Length(14),
            Constraint::Length(18),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(block);

    frame.render_widget(table, area);
}

/// Draw the checkpoint detail view.
fn draw_detail(frame: &mut Frame, app: &App, area: Rect, idx: usize) {
    let ckpt = match app.checkpoint_data.checkpoints.get(idx) {
        Some(c) => c,
        None => {
            let msg = Paragraph::new("Checkpoint not found")
                .block(Block::default().borders(Borders::ALL).title(" Detail "));
            frame.render_widget(msg, area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10), // Header info
            Constraint::Length(8),  // Token usage
            Constraint::Min(0),     // Files + tasks
        ])
        .split(area);

    draw_detail_header(frame, ckpt, chunks[0]);
    draw_detail_tokens(frame, ckpt, chunks[1]);
    draw_detail_files_tasks(frame, ckpt, chunks[2]);
}

/// Draw checkpoint detail header.
fn draw_detail_header(frame: &mut Frame, ckpt: &Checkpoint, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Checkpoint Detail (Esc: back) ");

    let sha_short = if ckpt.commit_sha.len() > 12 {
        &ckpt.commit_sha[..12]
    } else {
        &ckpt.commit_sha
    };

    let team_info = ckpt
        .team
        .as_ref()
        .map(|t| format!("{} ({} members)", t.team_name, t.members.len()))
        .unwrap_or_else(|| "(solo)".into());

    let prompt = ckpt
        .session
        .prompt_summary
        .as_deref()
        .unwrap_or("(no prompt)");

    let model = ckpt.session.model.as_deref().unwrap_or("unknown");

    let lines = vec![
        Line::from(vec![
            Span::styled("ID: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&ckpt.id),
        ]),
        Line::from(vec![
            Span::styled("Commit: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(sha_short, Style::default().fg(Color::Yellow)),
            Span::raw(format!("  branch: {}", ckpt.branch)),
        ]),
        Line::from(vec![
            Span::styled("Agent: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&ckpt.session.agent_name),
            Span::raw(format!("  model: {model}")),
        ]),
        Line::from(vec![
            Span::styled("Team: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(team_info),
        ]),
        Line::from(vec![
            Span::styled("Time: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(ckpt.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
        ]),
        Line::from(vec![
            Span::styled("Prompt: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(truncate(prompt, 80), Style::default().fg(Color::DarkGray)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

/// Draw token usage section of detail view.
fn draw_detail_tokens(frame: &mut Frame, ckpt: &Checkpoint, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Token Usage ");

    let lines = if let Some(ref usage) = ckpt.token_usage {
        let cost = estimate_cost(usage);
        vec![
            Line::from(vec![
                Span::styled("Input: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format_tokens(usage.input_tokens)),
                Span::raw("    "),
                Span::styled("Output: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format_tokens(usage.output_tokens)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Cache Read: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format_tokens(usage.cache_read_tokens.unwrap_or(0))),
                Span::raw("  "),
                Span::styled(
                    "Cache Write: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format_tokens(usage.cache_write_tokens.unwrap_or(0))),
            ]),
            Line::from(vec![
                Span::styled("Total: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format_tokens(usage.total())),
                Span::raw("    "),
                Span::styled("Est. Cost: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("${cost:.4}"), Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Tool Calls: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(ckpt.tool_calls.len().to_string()),
            ]),
        ]
    } else {
        vec![Line::from(Span::styled(
            "No token usage data (use --extended when creating checkpoints)",
            Style::default().fg(Color::DarkGray),
        ))]
    };

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Draw files and tasks in the detail view.
fn draw_detail_files_tasks(frame: &mut Frame, ckpt: &Checkpoint, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Files list
    let files_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Files ({}) ", ckpt.files.len()));

    let file_items: Vec<ListItem> = ckpt
        .files
        .iter()
        .map(|f| {
            let role_icon = match f.role {
                crate::models::checkpoint::FileRole::Created => "+",
                crate::models::checkpoint::FileRole::Modified => "~",
                crate::models::checkpoint::FileRole::Deleted => "-",
                crate::models::checkpoint::FileRole::Referenced => ".",
            };
            let style = match f.role {
                crate::models::checkpoint::FileRole::Created => Style::default().fg(Color::Green),
                crate::models::checkpoint::FileRole::Modified => Style::default().fg(Color::Yellow),
                crate::models::checkpoint::FileRole::Deleted => Style::default().fg(Color::Red),
                crate::models::checkpoint::FileRole::Referenced => {
                    Style::default().fg(Color::DarkGray)
                }
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{role_icon} "), style),
                Span::raw(&f.path),
            ]))
        })
        .collect();

    let files_list = List::new(file_items).block(files_block);
    frame.render_widget(files_list, chunks[0]);

    // Tasks list
    let tasks_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Tasks ({}) ", ckpt.tasks.len()));

    let task_items: Vec<ListItem> = ckpt
        .tasks
        .iter()
        .map(|t| {
            let status_color = match t.status.as_str() {
                "pending" => Color::Yellow,
                "in_progress" => Color::Blue,
                "completed" => Color::Green,
                _ => Color::White,
            };
            let owner = t.owner.as_deref().unwrap_or("-");
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", t.status),
                    Style::default().fg(status_color),
                ),
                Span::raw(&t.subject),
                Span::styled(format!(" ({owner})"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let tasks_list = List::new(task_items).block(tasks_block);
    frame.render_widget(tasks_list, chunks[1]);
}

/// Format a token count with comma separators.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Truncate a string to max length.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
