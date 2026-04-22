//! Terminal UI for browsing team status, checkpoints, and token costs.
//!
//! Built on ratatui + crossterm. Gated behind the `tui` cargo feature.
//!
//! # Usage
//!
//! ```ignore
//! agent_teams::tui::run(Some("my-team"), None)?;
//! ```

pub mod app;
pub mod event;
pub mod ui;
pub mod views;

use std::io;
use std::path::PathBuf;

use crossterm::{
    event::DisableMouseCapture,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use event::{Event, EventHandler};

/// Run the TUI application.
///
/// - `team`: Optional team name to focus on.
/// - `repo_path`: Optional repo path; defaults to current directory.
pub fn run(team: Option<String>, repo_path: Option<PathBuf>) -> crate::Result<()> {
    let repo_path = match repo_path {
        Some(p) => p,
        None => std::env::current_dir().map_err(|e| crate::Error::Other(e.to_string()))?,
    };

    // Setup terminal
    enable_raw_mode()
        .map_err(|e| crate::Error::Other(format!("Failed to enable raw mode: {e}")))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| crate::Error::Other(format!("Failed to enter alternate screen: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| crate::Error::Other(format!("Failed to create terminal: {e}")))?;

    // Create app state and event handler
    let mut app = App::new(team, repo_path);
    let events = EventHandler::new(250);

    // Main loop
    let result = run_loop(&mut terminal, &mut app, &events);

    // Restore terminal
    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    events: &EventHandler,
) -> crate::Result<()> {
    loop {
        // Draw
        terminal
            .draw(|frame| ui::draw(frame, app))
            .map_err(|e| crate::Error::Other(format!("Draw error: {e}")))?;

        // Handle events
        match events.next() {
            Ok(Event::Key(key)) => {
                app.handle_key(key);
                if app.should_quit {
                    return Ok(());
                }
            }
            Ok(Event::Tick) => {
                app.tick();
            }
            Ok(Event::Resize(_, _)) => {
                // Terminal handles resize automatically
            }
            Err(e) => {
                return Err(crate::Error::Other(format!("Event error: {e}")));
            }
        }
    }
}
