//! Application state and tab management for the TUI.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::checkpoint::query::{CheckpointFilter, CheckpointQuery};
use crate::models::checkpoint::Checkpoint;
use crate::models::team::TeamConfig;
use crate::models::token::{CostSummary, TokenUsage, estimate_cost};
use crate::util::session_discovery;

/// Active tab in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    TeamStatus,
    Checkpoints,
    TokenCosts,
}

impl Tab {
    pub fn title(&self) -> &'static str {
        match self {
            Tab::TeamStatus => "Team Status",
            Tab::Checkpoints => "Checkpoints",
            Tab::TokenCosts => "Token Costs",
        }
    }

    pub fn all() -> &'static [Tab] {
        &[Tab::TeamStatus, Tab::Checkpoints, Tab::TokenCosts]
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::TeamStatus => 0,
            Tab::Checkpoints => 1,
            Tab::TokenCosts => 2,
        }
    }
}

/// Detail view for checkpoints tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointView {
    /// Scrollable list of checkpoints.
    List,
    /// Detail view for a single checkpoint.
    Detail(usize),
}

/// Loaded team data for the team status view.
#[derive(Debug, Clone)]
pub struct TeamData {
    pub config: TeamConfig,
    pub tasks: Vec<crate::models::task::TaskFile>,
}

/// Loaded checkpoint data.
#[derive(Debug, Clone)]
pub struct CheckpointData {
    pub checkpoints: Vec<Checkpoint>,
    pub error: Option<String>,
}

/// Token cost data.
#[derive(Debug, Clone)]
pub struct CostData {
    pub summary: Option<CostSummary>,
    pub error: Option<String>,
}

/// Main application state.
pub struct App {
    /// Currently selected tab.
    pub active_tab: Tab,

    /// Whether the app should quit.
    pub should_quit: bool,

    /// Optional team name to focus on.
    pub team_name: Option<String>,

    /// Repository path.
    pub repo_path: PathBuf,

    /// Discovered teams (if no specific team given).
    pub teams: Vec<String>,

    /// Loaded team data.
    pub team_data: Option<TeamData>,

    /// Checkpoint data.
    pub checkpoint_data: CheckpointData,

    /// Checkpoint sub-view.
    pub checkpoint_view: CheckpointView,

    /// Token cost data.
    pub cost_data: CostData,

    /// Scroll offset for the active list.
    pub scroll_offset: usize,

    /// Selected index in the active list.
    pub selected: usize,

    /// Tick counter (for periodic refresh).
    tick_count: u64,
}

impl App {
    /// Create a new App with optional team focus and repo path.
    pub fn new(team_name: Option<String>, repo_path: PathBuf) -> Self {
        let mut app = Self {
            active_tab: Tab::TeamStatus,
            should_quit: false,
            team_name,
            repo_path,
            teams: Vec::new(),
            team_data: None,
            checkpoint_data: CheckpointData {
                checkpoints: Vec::new(),
                error: None,
            },
            checkpoint_view: CheckpointView::List,
            cost_data: CostData {
                summary: None,
                error: None,
            },
            scroll_offset: 0,
            selected: 0,
            tick_count: 0,
        };
        // Initial data load
        app.refresh_all();
        app
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            // Quit
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }

            // Tab switching
            KeyCode::Char('1') => self.switch_tab(Tab::TeamStatus),
            KeyCode::Char('2') => self.switch_tab(Tab::Checkpoints),
            KeyCode::Char('3') => self.switch_tab(Tab::TokenCosts),
            KeyCode::Tab => self.next_tab(),
            KeyCode::BackTab => self.prev_tab(),

            // Navigation
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Enter => self.select_item(),
            KeyCode::Esc => self.back(),

            // Refresh
            KeyCode::Char('r') => self.refresh_all(),

            _ => {}
        }
    }

    /// Called on every tick (250ms) — periodic data refresh.
    pub fn tick(&mut self) {
        self.tick_count += 1;
        // Refresh data every 4 seconds (16 ticks at 250ms)
        if self.tick_count % 16 == 0 {
            self.refresh_all();
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.active_tab != tab {
            self.active_tab = tab;
            self.selected = 0;
            self.scroll_offset = 0;
            self.checkpoint_view = CheckpointView::List;
        }
    }

    fn next_tab(&mut self) {
        let tabs = Tab::all();
        let idx = self.active_tab.index();
        self.switch_tab(tabs[(idx + 1) % tabs.len()]);
    }

    fn prev_tab(&mut self) {
        let tabs = Tab::all();
        let idx = self.active_tab.index();
        self.switch_tab(tabs[(idx + tabs.len() - 1) % tabs.len()]);
    }

    fn move_down(&mut self) {
        let max = self.list_len();
        if max > 0 && self.selected < max - 1 {
            self.selected += 1;
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn select_item(&mut self) {
        match self.active_tab {
            Tab::Checkpoints => {
                if self.checkpoint_view == CheckpointView::List
                    && self.selected < self.checkpoint_data.checkpoints.len()
                {
                    self.checkpoint_view = CheckpointView::Detail(self.selected);
                }
            }
            _ => {}
        }
    }

    fn back(&mut self) {
        match self.active_tab {
            Tab::Checkpoints => {
                if self.checkpoint_view != CheckpointView::List {
                    self.checkpoint_view = CheckpointView::List;
                }
            }
            _ => {}
        }
    }

    /// Number of items in the current list view.
    fn list_len(&self) -> usize {
        match self.active_tab {
            Tab::TeamStatus => self.team_data.as_ref().map(|d| d.tasks.len()).unwrap_or(0),
            Tab::Checkpoints => self.checkpoint_data.checkpoints.len(),
            Tab::TokenCosts => self
                .cost_data
                .summary
                .as_ref()
                .map(|s| s.per_agent.len())
                .unwrap_or(0),
        }
    }

    /// Refresh all data from disk.
    pub fn refresh_all(&mut self) {
        self.refresh_teams();
        self.refresh_checkpoints();
        self.refresh_costs();
    }

    fn refresh_teams(&mut self) {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };

        // Discover teams
        if let Ok(teams) = tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|_| None::<Vec<String>>) // We're sync, can't use async
            .ok_or(())
            .or_else(|_| -> Result<Vec<String>, ()> {
                // Read team dirs directly from filesystem
                let teams_dir = home.join(".claude").join("teams");
                if !teams_dir.exists() {
                    return Ok(Vec::new());
                }
                let mut teams = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&teams_dir) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            if let Some(name) = entry.file_name().to_str() {
                                teams.push(name.to_string());
                            }
                        }
                    }
                }
                Ok(teams)
            })
        {
            self.teams = teams;
        }

        // Load focused team or first discovered team
        let team_name = self
            .team_name
            .clone()
            .or_else(|| self.teams.first().cloned());

        if let Some(ref name) = team_name {
            // Read team config
            let config_path = home
                .join(".claude")
                .join("teams")
                .join(name)
                .join("config.json");
            let config: Option<TeamConfig> = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|data| serde_json::from_str(&data).ok());

            // Read tasks
            let tasks_dir = home.join(".claude").join("tasks").join(name);
            let tasks = read_tasks_from_dir(&tasks_dir);

            if let Some(config) = config {
                self.team_data = Some(TeamData { config, tasks });
            }
        }
    }

    fn refresh_checkpoints(&mut self) {
        match CheckpointQuery::open(&self.repo_path) {
            Ok(query) => match query.list(&CheckpointFilter::default()) {
                Ok(checkpoints) => {
                    self.checkpoint_data = CheckpointData {
                        checkpoints,
                        error: None,
                    };
                }
                Err(e) => {
                    self.checkpoint_data.error = Some(format!("List error: {e}"));
                }
            },
            Err(e) => {
                self.checkpoint_data = CheckpointData {
                    checkpoints: Vec::new(),
                    error: Some(format!("No checkpoints: {e}")),
                };
            }
        }
    }

    fn refresh_costs(&mut self) {
        let sessions = session_discovery::discover_sessions(&self.repo_path);
        if sessions.is_empty() {
            // Try from checkpoints
            let total_from_checkpoints = self.aggregate_checkpoint_costs();
            let has_data = total_from_checkpoints.is_some();
            self.cost_data = CostData {
                summary: total_from_checkpoints,
                error: if !has_data {
                    Some("No session data found".into())
                } else {
                    None
                },
            };
            return;
        }

        match session_discovery::aggregate_cost(&sessions, None) {
            Ok(summary) => {
                self.cost_data = CostData {
                    summary: Some(summary),
                    error: None,
                };
            }
            Err(e) => {
                self.cost_data = CostData {
                    summary: None,
                    error: Some(format!("Cost aggregation error: {e}")),
                };
            }
        }
    }

    fn aggregate_checkpoint_costs(&self) -> Option<CostSummary> {
        if self.checkpoint_data.checkpoints.is_empty() {
            return None;
        }

        let mut total = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        let mut count = 0;

        for ckpt in &self.checkpoint_data.checkpoints {
            if let Some(ref usage) = ckpt.token_usage {
                total.merge(usage);
                count += 1;
            }
        }

        if count == 0 {
            return None;
        }

        Some(CostSummary {
            total_usage: total.clone(),
            session_count: count,
            per_agent: Vec::new(), // Checkpoints don't have per-agent breakdown
            estimated_cost_usd: estimate_cost(&total),
        })
    }
}

/// Read task files from a directory (sync filesystem read).
fn read_tasks_from_dir(dir: &std::path::Path) -> Vec<crate::models::task::TaskFile> {
    let mut tasks = Vec::new();
    if !dir.exists() {
        return tasks;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return tasks,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(task) = serde_json::from_str::<crate::models::task::TaskFile>(&data) {
                    tasks.push(task);
                }
            }
        }
    }
    // Sort by ID
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    tasks
}
