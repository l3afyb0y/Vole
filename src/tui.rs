use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use humansize::{format_size, BINARY};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Terminal;

use crate::clean::scan_rule;
use crate::config::Rule;
use crate::snapshot::SnapshotSupport;

pub fn run(
    rules: Vec<Rule>,
    snapshot_support: Option<SnapshotSupport>,
    is_root: bool,
    start_with_sudo: bool,
    sudo_reexec: Option<Vec<String>>,
) -> Result<TuiExit> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(rules, snapshot_support, is_root, start_with_sudo, sudo_reexec);
    app.rescan();

    let exit = run_app(&mut terminal, &mut app);

    restore_terminal(&mut terminal)?;

    exit
}

#[derive(Debug)]
pub enum TuiExit {
    Quit,
    Apply {
        rules: Vec<Rule>,
        snapshot: Option<SnapshotSupport>,
    },
    ReexecSudo {
        args: Vec<String>,
    },
}

struct RuleState {
    rule: Rule,
    enabled: bool,
    scan: Option<crate::clean::RuleScan>,
}

struct AppState {
    rules: Vec<RuleState>,
    list_state: ListState,
    dry_run: bool,
    include_sudo: bool,
    is_root: bool,
    snapshot_support: Option<SnapshotSupport>,
    snapshot_enabled: bool,
    confirm_apply: bool,
    message: Option<String>,
    sudo_reexec_args: Option<Vec<String>>,
}

impl AppState {
    fn new(
        rules: Vec<Rule>,
        snapshot_support: Option<SnapshotSupport>,
        is_root: bool,
        start_with_sudo: bool,
        sudo_reexec_args: Option<Vec<String>>,
    ) -> Self {
        let mut list_state = ListState::default();
        if !rules.is_empty() {
            list_state.select(Some(0));
        }
        let include_sudo = start_with_sudo && is_root;
        Self {
            rules: rules
                .into_iter()
                .map(|rule| RuleState {
                    enabled: if rule.requires_sudo {
                        include_sudo && rule.enabled_by_default
                    } else {
                        rule.enabled_by_default
                    },
                    rule,
                    scan: None,
                })
                .collect(),
            list_state,
            dry_run: false,
            include_sudo,
            is_root,
            snapshot_support,
            snapshot_enabled: false,
            confirm_apply: false,
            message: None,
            sudo_reexec_args,
        }
    }

    fn rescan(&mut self) {
        for state in &mut self.rules {
            if state.rule.requires_sudo && (!self.include_sudo || !self.is_root) {
                state.scan = None;
                continue;
            }
            state.scan = Some(scan_rule(&state.rule));
        }
        self.message = Some("Scan complete".to_string());
    }

    fn toggle_selected(&mut self) {
        if let Some(index) = self.list_state.selected() {
            if let Some(state) = self.rules.get_mut(index) {
                if state.rule.requires_sudo && (!self.include_sudo || !self.is_root) {
                    self.message = Some("Requires sudo; restart as root to enable".to_string());
                } else {
                    state.enabled = !state.enabled;
                }
            }
        }
    }

    fn toggle_sudo(&mut self) {
        self.include_sudo = !self.include_sudo;
        for rule in &mut self.rules {
            if rule.rule.requires_sudo {
                if self.include_sudo {
                    rule.enabled = rule.rule.enabled_by_default;
                } else {
                    rule.enabled = false;
                }
            }
        }
        self.rescan();
    }

    fn toggle_snapshot(&mut self) {
        if self.snapshot_support.is_none() {
            return;
        }
        self.snapshot_enabled = !self.snapshot_enabled;
    }

    fn total_selected(&self) -> (u64, usize) {
        let mut bytes = 0;
        let mut entries = 0;
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            if let Some(scan) = &rule.scan {
                bytes += scan.bytes;
                entries += scan.entries;
            }
        }
        (bytes, entries)
    }

    fn selected_rules(&self) -> Vec<Rule> {
        self.rules
            .iter()
            .filter(|rule| rule.enabled)
            .map(|rule| rule.rule.clone())
            .collect()
    }
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut AppState) -> Result<TuiExit> {
    loop {
        terminal.draw(|frame| draw_ui(frame, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if let Some(exit) = handle_key(app, key)? {
                    return Ok(exit);
                }
            }
        }
    }
}

fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<Option<TuiExit>> {
    if app.confirm_apply {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let rules = app.selected_rules();
                let snapshot = if app.snapshot_enabled {
                    app.snapshot_support.clone()
                } else {
                    None
                };
                return Ok(Some(TuiExit::Apply { rules, snapshot }));
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.confirm_apply = false;
            }
            _ => {}
        }
        return Ok(None);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(Some(TuiExit::Quit)),
        KeyCode::Down | KeyCode::Char('j') => {
            let next = match app.list_state.selected() {
                Some(i) => (i + 1).min(app.rules.len().saturating_sub(1)),
                None => 0,
            };
            app.list_state.select(Some(next));
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let prev = match app.list_state.selected() {
                Some(i) => i.saturating_sub(1),
                None => 0,
            };
            app.list_state.select(Some(prev));
        }
        KeyCode::Char(' ') => {
            app.toggle_selected();
        }
        KeyCode::Char('r') => {
            app.rescan();
        }
        KeyCode::Char('d') => {
            app.dry_run = !app.dry_run;
        }
        KeyCode::Char('s') => {
            if !app.is_root {
                if let Some(args) = app.sudo_reexec_args.clone() {
                    return Ok(Some(TuiExit::ReexecSudo { args }));
                }
                app.message = Some("Sudo is unavailable in this environment".to_string());
            } else {
                app.toggle_sudo();
            }
        }
        KeyCode::Char('p') => {
            app.toggle_snapshot();
        }
        KeyCode::Char('a') => {
            if app.dry_run {
                app.message = Some("Disable dry-run before applying".to_string());
            } else if app.selected_rules().is_empty() {
                app.message = Some("No rules selected".to_string());
            } else {
                app.confirm_apply = true;
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(Some(TuiExit::Quit));
        }
        _ => {}
    }

    Ok(None)
}

fn draw_ui(frame: &mut ratatui::Frame<'_>, app: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(frame.size());

    let items = app
        .rules
        .iter()
        .map(|state| {
            let enabled = if state.enabled { "x" } else { " " };
            let sudo = if state.rule.requires_sudo { " (sudo)" } else { "" };
            let size = state
                .scan
                .as_ref()
                .map(|scan| format!("{} / {}", format_size(scan.bytes, BINARY), scan.entries))
                .unwrap_or_else(|| "-".to_string());
            let content = format!("[{}] {}{}  {}", enabled, state.rule.label, sudo, size);
            ListItem::new(Line::from(content))
        })
        .collect::<Vec<_>>();

    let list = List::new(items)
        .block(Block::default().title("Cleanup Rules").borders(Borders::ALL))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    frame.render_stateful_widget(list, chunks[0], &mut app.list_state);

    let (bytes, entries) = app.total_selected();
    let summary = format!(
        "Selected: {} rules | {} | {} items",
        app.selected_rules().len(),
        format_size(bytes, BINARY),
        entries
    );
    let mode = if let Some(support) = &app.snapshot_support {
        let snapshot_status = format!("{} ({})", on_off(app.snapshot_enabled), support.label);
        format!(
            "Dry-run: {} | Sudo: {} | Snapshot: {}",
            on_off(app.dry_run),
            on_off(app.include_sudo),
            snapshot_status
        )
    } else {
        format!(
            "Dry-run: {} | Sudo: {}",
            on_off(app.dry_run),
            on_off(app.include_sudo)
        )
    };

    let mut help = "Keys: j/k or arrows move | space toggle | r rescan | d dry-run | s sudo".to_string();
    if app.snapshot_support.is_some() {
        help.push_str(" | p snapshot");
    }
    help.push_str(" | a apply | q quit");
    let summary_block = Paragraph::new(vec![Line::from(summary), Line::from(mode), Line::from(help)])
        .block(Block::default().borders(Borders::ALL).title("Status"));
    frame.render_widget(summary_block, chunks[1]);

    let message = if app.confirm_apply {
        "Confirm delete? (y/n)"
    } else {
        app.message.as_deref().unwrap_or("")
    };
    let message_block = Paragraph::new(message)
        .block(Block::default().borders(Borders::ALL).title("Message"));
    frame.render_widget(message_block, chunks[2]);
}

fn on_off(value: bool) -> &'static str {
    if value {
        "ON"
    } else {
        "OFF"
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
