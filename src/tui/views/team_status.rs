//! Team status view: team config, members, and task list.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::models::task::TaskStatus;
use crate::tui::app::App;

/// Draw the team status tab.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Team info header
            Constraint::Min(0),    // Task list
        ])
        .split(area);

    draw_team_header(frame, app, chunks[0]);
    draw_task_list(frame, app, chunks[1]);
}

/// Draw team info header.
fn draw_team_header(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Team Info ");

    let content = if let Some(ref data) = app.team_data {
        let config = &data.config;
        let member_count = config.members.len();
        let desc = config.description.as_deref().unwrap_or("(no description)");

        let total = data.tasks.len();
        let pending = data
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let in_progress = data
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let completed = data
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();

        let members_str: String = config
            .members
            .iter()
            .map(|m| format!("{} ({})", m.name(), m.agent_type()))
            .collect::<Vec<_>>()
            .join(", ");

        vec![
            Line::from(vec![
                Span::styled("Team: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(&config.team_name),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Members: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{member_count} — ")),
                Span::raw(members_str),
            ]),
            Line::from(vec![
                Span::styled("Tasks: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{total} total")),
                Span::raw("  "),
                Span::styled(
                    format!("{pending} pending"),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{in_progress} active"),
                    Style::default().fg(Color::Blue),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{completed} done"),
                    Style::default().fg(Color::Green),
                ),
            ]),
        ]
    } else if app.teams.is_empty() {
        vec![Line::from(Span::styled(
            "No teams found. Create a team with: agent-teams team create <name>",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        vec![Line::from(Span::styled(
            format!("Available teams: {}", app.teams.join(", ")),
            Style::default().fg(Color::DarkGray),
        ))]
    };

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

/// Draw the task list table.
fn draw_task_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Tasks ");

    let tasks = match app.team_data {
        Some(ref data) => &data.tasks,
        None => {
            let empty = Paragraph::new("No team loaded").block(block);
            frame.render_widget(empty, area);
            return;
        }
    };

    if tasks.is_empty() {
        let empty = Paragraph::new("No tasks").block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Status"),
        Cell::from("Owner"),
        Cell::from("Subject"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let status_style = match task.status {
                TaskStatus::Pending => Style::default().fg(Color::Yellow),
                TaskStatus::InProgress => Style::default().fg(Color::Blue),
                TaskStatus::Completed => Style::default().fg(Color::Green),
                TaskStatus::Deleted => Style::default().fg(Color::DarkGray),
            };
            let status_icon = match task.status {
                TaskStatus::Pending => "  pending",
                TaskStatus::InProgress => "  active",
                TaskStatus::Completed => "  done",
                TaskStatus::Deleted => "  deleted",
            };
            let owner = task.owner.as_deref().unwrap_or("-");
            let row_style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(task.id.clone()),
                Cell::from(status_icon).style(status_style),
                Cell::from(owner.to_string()),
                Cell::from(task.subject.clone()),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(block);

    frame.render_widget(table, area);
}
