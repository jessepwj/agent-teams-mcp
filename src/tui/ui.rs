//! Main UI layout and rendering.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use super::app::{App, Tab};
use super::views;

/// Draw the entire application frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Tab bar
            Constraint::Min(0),    // Content area
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    draw_tabs(frame, app, chunks[0]);
    draw_content(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);
}

/// Draw the tab bar at the top.
fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::all()
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let num = format!("{}", i + 1);
            Line::from(vec![
                Span::styled(num, Style::default().fg(Color::Yellow)),
                Span::raw(":"),
                Span::raw(tab.title()),
            ])
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Agent Teams TUI "),
        )
        .select(app.active_tab.index())
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "));

    frame.render_widget(tabs, area);
}

/// Draw the active tab's content.
fn draw_content(frame: &mut Frame, app: &App, area: Rect) {
    match app.active_tab {
        Tab::TeamStatus => views::team_status::draw(frame, app, area),
        Tab::Checkpoints => views::checkpoints::draw(frame, app, area),
        Tab::TokenCosts => views::token_costs::draw(frame, app, area),
    }
}

/// Draw the status bar at the bottom.
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = match app.active_tab {
        Tab::Checkpoints => {
            if matches!(app.checkpoint_view, super::app::CheckpointView::Detail(_)) {
                "Esc:back | q:quit | Tab:switch | r:refresh"
            } else {
                "j/k:navigate | Enter:detail | q:quit | Tab:switch | r:refresh"
            }
        }
        _ => "j/k:navigate | q:quit | 1/2/3:tabs | Tab:switch | r:refresh",
    };

    let team_info = app
        .team_name
        .as_deref()
        .or_else(|| app.teams.first().map(|s| s.as_str()))
        .unwrap_or("(no team)");

    let status = Line::from(vec![
        Span::styled(
            format!(" {team_info} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(help, Style::default().fg(Color::DarkGray)),
    ]);

    let bar = Paragraph::new(status);
    frame.render_widget(bar, area);
}
