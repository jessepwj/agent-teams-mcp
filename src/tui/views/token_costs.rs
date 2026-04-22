//! Token costs view: aggregated usage, per-agent breakdown, and cost estimates.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Cell, Paragraph, Row, Table};

use crate::models::token::estimate_cost;
use crate::tui::app::App;

/// Draw the token costs tab.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Summary header
            Constraint::Min(0),    // Per-agent breakdown + chart
        ])
        .split(area);

    draw_summary(frame, app, chunks[0]);
    draw_breakdown(frame, app, chunks[1]);
}

/// Draw the cost summary header.
fn draw_summary(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Token Cost Summary ");

    if let Some(ref err) = app.cost_data.error {
        let msg = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let summary = match &app.cost_data.summary {
        Some(s) => s,
        None => {
            let msg = Paragraph::new("No cost data available")
                .style(Style::default().fg(Color::DarkGray))
                .block(block);
            frame.render_widget(msg, area);
            return;
        }
    };

    let usage = &summary.total_usage;
    let lines = vec![
        Line::from(vec![
            Span::styled("Sessions: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(summary.session_count.to_string()),
            Span::raw("    "),
            Span::styled("Agents: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(summary.per_agent.len().to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Input Tokens: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format_tokens(usage.input_tokens)),
            Span::raw("    "),
            Span::styled(
                "Output Tokens: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format_tokens(usage.output_tokens)),
        ]),
        Line::from(vec![
            Span::styled(
                "Cache Read: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format_tokens(usage.cache_read_tokens.unwrap_or(0))),
            Span::raw("    "),
            Span::styled(
                "Cache Write: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format_tokens(usage.cache_write_tokens.unwrap_or(0))),
        ]),
        Line::from(vec![
            Span::styled(
                "Total Tokens: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format_tokens(usage.total()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("    "),
            Span::styled(
                "Estimated Cost: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("${:.4}", summary.estimated_cost_usd),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Draw per-agent breakdown.
fn draw_breakdown(frame: &mut Frame, app: &App, area: Rect) {
    let summary = match &app.cost_data.summary {
        Some(s) if !s.per_agent.is_empty() => s,
        _ => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Per-Agent Breakdown ");
            let msg = Paragraph::new("No per-agent breakdown available")
                .style(Style::default().fg(Color::DarkGray))
                .block(block);
            frame.render_widget(msg, area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    // Table
    let table_block = Block::default()
        .borders(Borders::ALL)
        .title(" Per-Agent Breakdown ");

    let header = Row::new(vec![
        Cell::from("Agent"),
        Cell::from("Input"),
        Cell::from("Output"),
        Cell::from("Total"),
        Cell::from("Cost"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = summary
        .per_agent
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            let cost = estimate_cost(&agent.usage);
            let row_style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(agent.agent_name.clone()),
                Cell::from(format_tokens(agent.usage.input_tokens)),
                Cell::from(format_tokens(agent.usage.output_tokens)),
                Cell::from(format_tokens(agent.usage.total())),
                Cell::from(format!("${cost:.4}")),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(table_block);

    frame.render_widget(table, chunks[0]);

    // Bar chart
    let chart_block = Block::default()
        .borders(Borders::ALL)
        .title(" Token Usage by Agent ");

    if summary.per_agent.is_empty() {
        let msg = Paragraph::new("No data").block(chart_block);
        frame.render_widget(msg, chunks[1]);
        return;
    }

    let max_tokens = summary
        .per_agent
        .iter()
        .map(|a| a.usage.total())
        .max()
        .unwrap_or(1);

    let bars: Vec<Bar> = summary
        .per_agent
        .iter()
        .map(|agent| {
            let label = if agent.agent_name.len() > 10 {
                agent.agent_name[..10].to_string()
            } else {
                agent.agent_name.clone()
            };
            Bar::default()
                .value(agent.usage.total())
                .label(Line::from(label))
                .style(Style::default().fg(Color::Cyan))
        })
        .collect();

    let bar_chart = BarChart::default()
        .block(chart_block)
        .data(BarGroup::default().bars(&bars))
        .bar_width(8)
        .bar_gap(2)
        .max(max_tokens);

    frame.render_widget(bar_chart, chunks[1]);
}

/// Format a token count with K/M suffix.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
