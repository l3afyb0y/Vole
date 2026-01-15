use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use humansize::{format_size, BINARY};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use serde_json;

use crate::clean::{dry_run_output, scan_rule, write_dry_run_report};
use crate::config::Rule;
use crate::snapshot::SnapshotSupport;

const OUTPUT_SCROLL_STEP: isize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub enabled_rules: Vec<String>,
    pub selected_rule: Option<String>,
    pub dry_run: bool,
    pub snapshot_enabled: bool,
    pub include_sudo: bool,
}

pub fn run(
    rules: Vec<Rule>,
    snapshot_support: Option<SnapshotSupport>,
    is_root: bool,
    start_with_sudo: bool,
    start_with_dry_run: bool,
    sudo_reexec: Option<Vec<String>>,
    initial_state: Option<PersistedState>,
    home: PathBuf,
) -> Result<TuiExit> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(
        rules,
        snapshot_support,
        is_root,
        start_with_sudo,
        start_with_dry_run,
        sudo_reexec,
        home,
    );
    if let Some(state) = initial_state {
        app.apply_state(&state);
    }
    app.rescan_with_message(Some("Scan complete".to_string()));

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

#[derive(Debug, Default, Clone)]
struct ActionHitboxes {
    apply: Option<Rect>,
    dry_run: Option<Rect>,
    sudo: Option<Rect>,
    snapshot: Option<Rect>,
}

#[derive(Debug, Clone)]
struct ActionLine {
    spans: Vec<Span<'static>>,
}

#[derive(Debug, Default, Clone)]
struct UiLayout {
    list_area: Option<Rect>,
    output_block_area: Option<Rect>,
    output_area: Option<Rect>,
    output_scrollbar_area: Option<Rect>,
    actions: ActionHitboxes,
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
    confirm_requires_delete: bool,
    confirm_buffer: String,
    message: Option<String>,
    sudo_reexec_args: Option<Vec<String>>,
    layout: UiLayout,
    home: PathBuf,
    output_lines: Vec<String>,
    output_scroll: usize,
}

impl AppState {
    fn new(
        rules: Vec<Rule>,
        snapshot_support: Option<SnapshotSupport>,
        is_root: bool,
        start_with_sudo: bool,
        start_with_dry_run: bool,
        sudo_reexec_args: Option<Vec<String>>,
        home: PathBuf,
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
            dry_run: if include_sudo {
                true
            } else {
                start_with_dry_run
            },
            include_sudo,
            is_root,
            snapshot_support,
            snapshot_enabled: false,
            confirm_apply: false,
            confirm_requires_delete: false,
            confirm_buffer: String::new(),
            message: None,
            sudo_reexec_args,
            layout: UiLayout::default(),
            home,
            output_lines: Vec::new(),
            output_scroll: 0,
        }
    }

    fn rescan_with_message(&mut self, message: Option<String>) {
        for state in &mut self.rules {
            if state.rule.requires_sudo && (!self.include_sudo || !self.is_root) {
                state.scan = None;
                continue;
            }
            state.scan = Some(scan_rule(&state.rule));
        }
        self.message = message;
    }

    fn toggle_selected(&mut self) {
        if let Some(index) = self.list_state.selected() {
            self.toggle_at(index);
        }
    }

    fn toggle_at(&mut self, index: usize) {
        if let Some(state) = self.rules.get_mut(index) {
            if state.rule.requires_sudo && (!self.include_sudo || !self.is_root) {
                self.message = Some("Requires sudo; restart as root to enable".to_string());
            } else {
                state.enabled = !state.enabled;
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.rules.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
        self.select_index(next);
    }

    fn select_index(&mut self, index: usize) {
        if self.rules.is_empty() {
            self.list_state.select(None);
            return;
        }
        let clamped = index.min(self.rules.len().saturating_sub(1));
        self.list_state.select(Some(clamped));
        self.ensure_visible(clamped);
    }

    fn ensure_visible(&mut self, index: usize) {
        let height = self.list_height();
        if height == 0 {
            return;
        }
        let offset = self.list_state.offset();
        let max_offset = self.rules.len().saturating_sub(height);
        if index < offset {
            *self.list_state.offset_mut() = index;
        } else if index >= offset + height {
            *self.list_state.offset_mut() = (index + 1 - height).min(max_offset);
        }
    }

    fn list_height(&self) -> usize {
        self.layout
            .list_area
            .map(|rect| rect.height as usize)
            .unwrap_or_else(|| self.rules.len().max(1))
    }

    fn output_height(&self) -> usize {
        self.layout
            .output_area
            .map(|rect| rect.height as usize)
            .unwrap_or(0)
    }

    fn clamp_output_scroll(&mut self) {
        let height = self.output_height();
        if height == 0 || self.output_lines.is_empty() {
            self.output_scroll = 0;
            return;
        }
        let max_offset = self.output_lines.len().saturating_sub(height);
        if self.output_scroll > max_offset {
            self.output_scroll = max_offset;
        }
    }

    fn scroll_output(&mut self, delta: isize) {
        let height = self.output_height();
        if height == 0 || self.output_lines.is_empty() {
            self.output_scroll = 0;
            return;
        }
        if self.output_lines.len() <= height {
            self.output_scroll = 0;
            return;
        }
        let max_offset = self.output_lines.len() - height;
        let current = self.output_scroll as isize;
        let next = (current + delta).clamp(0, max_offset as isize) as usize;
        self.output_scroll = next;
    }

    fn scroll_output_page(&mut self, direction: isize) {
        let height = self.output_height().max(1);
        let step = height.saturating_sub(1).max(1) as isize;
        let delta = if direction < 0 { -step } else { step };
        self.scroll_output(delta);
    }

    fn scroll_output_to_top(&mut self) {
        self.output_scroll = 0;
    }

    fn scroll_output_to_bottom(&mut self) {
        let height = self.output_height();
        let max_offset = self.output_lines.len().saturating_sub(height);
        self.output_scroll = max_offset;
    }

    fn jump_output_to_row(&mut self, row: u16) {
        let Some(area) = self.layout.output_area else {
            return;
        };
        if self.output_lines.is_empty() || area.height == 0 {
            return;
        }
        let height = self.output_height();
        if height == 0 || self.output_lines.len() <= height {
            self.output_scroll = 0;
            return;
        }
        let max_offset = self.output_lines.len().saturating_sub(height);
        let max_row = area.y.saturating_add(area.height.saturating_sub(1));
        let clamped_row = row.clamp(area.y, max_row);
        let row_offset = clamped_row.saturating_sub(area.y) as usize;
        if area.height == 1 {
            self.output_scroll = 0;
            return;
        }
        let denom = (area.height - 1) as usize;
        let pos = (row_offset.saturating_mul(max_offset)) / denom;
        self.output_scroll = pos.min(max_offset);
    }

    fn toggle_sudo(&mut self) {
        let was_enabled = self.include_sudo;
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
        if self.include_sudo && !was_enabled {
            self.dry_run = true;
            self.rescan_with_message(Some("Sudo mode enabled: dry-run forced ON".to_string()));
        } else {
            let message = if !self.include_sudo {
                self.snapshot_enabled = false;
                Some("Sudo rules disabled (still running as root)".to_string())
            } else {
                Some("Scan complete".to_string())
            };
            self.rescan_with_message(message);
        }
    }

    fn toggle_snapshot(&mut self) {
        if self.snapshot_support.is_none() {
            return;
        }
        if self.dry_run {
            self.message = Some("Disable dry-run to use snapshots".to_string());
            return;
        }
        if !self.include_sudo {
            self.message = Some("Enable sudo to use snapshots".to_string());
            return;
        }
        self.snapshot_enabled = !self.snapshot_enabled;
    }

    fn toggle_dry_run(&mut self) {
        self.dry_run = !self.dry_run;
        if self.dry_run && self.snapshot_enabled {
            self.snapshot_enabled = false;
            self.message = Some("Dry-run enabled: snapshot disabled".to_string());
        }
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

    fn apply_state(&mut self, state: &PersistedState) {
        self.include_sudo = self.is_root && state.include_sudo;
        self.dry_run = if self.include_sudo {
            true
        } else {
            state.dry_run
        };
        self.snapshot_enabled =
            state.snapshot_enabled && self.snapshot_support.is_some() && self.include_sudo;
        self.apply_enabled_rules(&state.enabled_rules, state.selected_rule.as_deref());
    }

    fn export_state(&self) -> PersistedState {
        PersistedState {
            enabled_rules: self
                .rules
                .iter()
                .filter(|rule| rule.enabled)
                .map(|rule| rule.rule.id.clone())
                .collect(),
            selected_rule: self
                .list_state
                .selected()
                .and_then(|index| self.rules.get(index))
                .map(|state| state.rule.id.clone()),
            dry_run: self.dry_run,
            snapshot_enabled: self.snapshot_enabled,
            include_sudo: self.include_sudo,
        }
    }

    fn export_state_for_sudo(&self) -> PersistedState {
        let mut state = self.export_state();
        state.include_sudo = true;
        for rule in &self.rules {
            if rule.rule.requires_sudo && rule.rule.enabled_by_default {
                if !state
                    .enabled_rules
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&rule.rule.id))
                {
                    state.enabled_rules.push(rule.rule.id.clone());
                }
            }
        }
        state
    }

    fn apply_enabled_rules(&mut self, enabled_rules: &[String], selected_rule: Option<&str>) {
        let enabled = enabled_rules
            .iter()
            .map(|id| id.to_lowercase())
            .collect::<Vec<_>>();
        for (index, rule) in self.rules.iter_mut().enumerate() {
            let should_enable = enabled.iter().any(|id| id == &rule.rule.id.to_lowercase());
            rule.enabled = if rule.rule.requires_sudo && !self.include_sudo {
                false
            } else {
                should_enable
            };
            if let Some(selected_id) = selected_rule {
                if rule.rule.id.eq_ignore_ascii_case(selected_id) {
                    self.list_state.select(Some(index));
                }
            }
        }
        if self.list_state.selected().is_none() && !self.rules.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    fn selected_scans(&self) -> Vec<crate::clean::RuleScan> {
        self.rules
            .iter()
            .filter(|rule| rule.enabled)
            .filter_map(|rule| rule.scan.clone())
            .collect()
    }

    fn set_output_lines(&mut self, lines: Vec<String>) {
        self.output_lines = lines;
        self.output_scroll = self.output_lines.len();
    }

    fn run_dry_run(&mut self) {
        let scans = self.selected_scans();
        let output = dry_run_output(&scans);
        let mut lines = output
            .details
            .lines()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        match write_dry_run_report(&self.home, &output.details) {
            Ok(path) => lines.push(format!("Dry-run report saved to {}", path.display())),
            Err(err) => lines.push(format!("Failed to write dry-run report: {err}")),
        }

        let report = output.report;
        lines.push(format!(
            "Dry-run listed {} files and {} directories",
            report.files_listed, report.dirs_listed
        ));
        lines.push(format!(
            "Would free {}",
            format_size(report.bytes_listed, BINARY)
        ));
        if report.errors > 0 {
            lines.push(format!("Errors encountered: {}", report.errors));
        }

        self.set_output_lines(lines);
        self.message = Some("Dry-run complete (see Output panel)".to_string());
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
) -> Result<TuiExit> {
    loop {
        terminal.draw(|frame| draw_ui(frame, app))?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(exit) = handle_key(app, key)? {
                        return Ok(exit);
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(exit) = handle_mouse(app, mouse)? {
                        return Ok(exit);
                    }
                }
                _ => {}
            }
        }
    }
}

fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<Option<TuiExit>> {
    if app.confirm_apply {
        if app.confirm_requires_delete {
            match key.code {
                KeyCode::Esc => {
                    app.confirm_apply = false;
                    app.confirm_requires_delete = false;
                    app.confirm_buffer.clear();
                }
                KeyCode::Enter => {
                    if app.confirm_buffer.eq_ignore_ascii_case("delete") {
                        let rules = app.selected_rules();
                        let snapshot = if app.snapshot_enabled {
                            app.snapshot_support.clone()
                        } else {
                            None
                        };
                        return Ok(Some(TuiExit::Apply { rules, snapshot }));
                    }
                    app.message = Some("Type DELETE to confirm".to_string());
                    app.confirm_buffer.clear();
                }
                KeyCode::Backspace => {
                    app.confirm_buffer.pop();
                }
                KeyCode::Char(c) => {
                    if c.is_ascii_alphabetic() && app.confirm_buffer.len() < 6 {
                        app.confirm_buffer.push(c.to_ascii_uppercase());
                    }
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Char('y')
                | KeyCode::Char('Y')
                | KeyCode::Char('\n')
                | KeyCode::Char('\r')
                | KeyCode::Enter => {
                    if app.dry_run {
                        app.run_dry_run();
                        app.confirm_apply = false;
                        app.confirm_requires_delete = false;
                        app.confirm_buffer.clear();
                        return Ok(None);
                    }
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
                    app.confirm_requires_delete = false;
                    app.confirm_buffer.clear();
                }
                _ => {}
            }
        }
        return Ok(None);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(Some(TuiExit::Quit)),
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_selection(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_selection(-1);
        }
        KeyCode::Char(' ') => {
            app.toggle_selected();
        }
        KeyCode::Char('r') => {
            app.rescan_with_message(Some("Scan complete".to_string()));
        }
        KeyCode::Char('d') => {
            app.toggle_dry_run();
        }
        KeyCode::Char('s') => {
            if !app.is_root {
                if let Some(args) = build_sudo_reexec(app)? {
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
            begin_apply(app);
        }
        KeyCode::Enter => {
            begin_apply(app);
        }
        KeyCode::PageUp => {
            app.scroll_output_page(-1);
        }
        KeyCode::PageDown => {
            app.scroll_output_page(1);
        }
        KeyCode::Home => {
            app.scroll_output_to_top();
        }
        KeyCode::End => {
            app.scroll_output_to_bottom();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(Some(TuiExit::Quit));
        }
        _ => {}
    }

    Ok(None)
}

fn handle_mouse(app: &mut AppState, mouse: MouseEvent) -> Result<Option<TuiExit>> {
    if app.confirm_apply {
        return Ok(None);
    }

    let row = mouse.row;
    let col = mouse.column;

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if in_output_area(app, col, row) {
                app.scroll_output(OUTPUT_SCROLL_STEP);
            } else if in_list_area(app, col, row) {
                app.move_selection(1);
            }
        }
        MouseEventKind::ScrollUp => {
            if in_output_area(app, col, row) {
                app.scroll_output(-OUTPUT_SCROLL_STEP);
            } else if in_list_area(app, col, row) {
                app.move_selection(-1);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if in_scrollbar_area(app, col, row) || in_output_area(app, col, row) {
                app.jump_output_to_row(row);
                return Ok((true, None));
            }
            let (handled, exit) = handle_action_click(app, col, row)?;
            if handled {
                if let Some(exit) = exit {
                    return Ok(Some(exit));
                }
                return Ok(None);
            }
            if let Some(list_area) = app.layout.list_area {
                if contains(list_area, col, row) {
                    let offset = app.list_state.offset();
                    let index = offset + (row.saturating_sub(list_area.y) as usize);
                    if index < app.rules.len() {
                        app.select_index(index);
                        app.toggle_at(index);
                    }
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if in_scrollbar_area(app, col, row) || in_output_area(app, col, row) {
                app.jump_output_to_row(row);
                return Ok((true, None));
            }
        }
        _ => {}
    }

    Ok(None)
}

fn begin_apply(app: &mut AppState) {
    if app.selected_rules().is_empty() {
        app.message = Some("No rules selected".to_string());
    } else if app.snapshot_enabled && app.dry_run {
        app.message = Some("Disable dry-run to create snapshots".to_string());
    } else if app.snapshot_enabled && !app.include_sudo {
        app.message = Some("Enable sudo to use snapshots".to_string());
    } else {
        app.confirm_apply = true;
        app.confirm_requires_delete = app.include_sudo && !app.dry_run;
        app.confirm_buffer.clear();
    }
}

fn handle_action_click(app: &mut AppState, col: u16, row: u16) -> Result<(bool, Option<TuiExit>)> {
    let actions = &app.layout.actions;
    if let Some(rect) = actions.apply {
        if contains(rect, col, row) {
            begin_apply(app);
            return Ok((true, None));
        }
    }
    if let Some(rect) = actions.dry_run {
        if contains(rect, col, row) {
            app.toggle_dry_run();
            return Ok((true, None));
        }
    }
    if let Some(rect) = actions.sudo {
        if contains(rect, col, row) {
            if !app.is_root {
                if let Some(args) = build_sudo_reexec(app)? {
                    return Ok((true, Some(TuiExit::ReexecSudo { args })));
                }
                app.message = Some("Sudo is unavailable in this environment".to_string());
                return Ok((true, None));
            }
            app.toggle_sudo();
            return Ok((true, None));
        }
    }
    if let Some(rect) = actions.snapshot {
        if contains(rect, col, row) {
            app.toggle_snapshot();
            return Ok((true, None));
        }
    }
    Ok((false, None))
}

fn build_sudo_reexec(app: &AppState) -> Result<Option<Vec<String>>> {
    let Some(mut args) = app.sudo_reexec_args.clone() else {
        return Ok(None);
    };
    let path = save_state_to_temp(app, true)?;
    args.push("--tui-state".to_string());
    args.push(path.to_string_lossy().to_string());
    Ok(Some(args))
}

fn save_state_to_temp(app: &AppState, force_sudo: bool) -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    let pid = std::process::id();
    path.push(format!("vole-tui-state-{}.json", pid));
    let state = if force_sudo {
        app.export_state_for_sudo()
    } else {
        app.export_state()
    };
    let data = serde_json::to_vec(&state)?;
    std::fs::write(&path, data)?;
    Ok(path)
}

pub fn load_state(path: &Path) -> Result<PersistedState> {
    let data = std::fs::read(path)?;
    let state = serde_json::from_slice(&data)?;
    Ok(state)
}

fn in_list_area(app: &AppState, col: u16, row: u16) -> bool {
    app.layout
        .list_area
        .map(|rect| contains(rect, col, row))
        .unwrap_or(false)
}

fn in_output_area(app: &AppState, col: u16, row: u16) -> bool {
    app.layout
        .output_block_area
        .map(|rect| contains(rect, col, row))
        .unwrap_or(false)
}

fn in_scrollbar_area(app: &AppState, col: u16, row: u16) -> bool {
    app.layout
        .output_scrollbar_area
        .map(|rect| contains(rect, col, row))
        .unwrap_or(false)
}

fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn build_action_line(area: Rect, app: &AppState) -> (ActionLine, ActionHitboxes) {
    let block = Block::default().borders(Borders::ALL).title("Status");
    let inner = block.inner(area);

    let base_style = Style::default().fg(Color::Blue);
    let mut line = String::from("Actions: ");
    let mut spans = vec![Span::styled("Actions: ", base_style)];
    let mut hitboxes = ActionHitboxes::default();

    let mut cursor = line.len() as u16;
    let apply_enabled = !app.selected_rules().is_empty();
    let apply_label = if !apply_enabled {
        "[Apply (disabled)]"
    } else if app.dry_run {
        "[Apply (dry-run)]"
    } else {
        "[Apply]"
    };
    let dry_label = if app.dry_run {
        "[Dry-run: ON]"
    } else {
        "[Dry-run: OFF]"
    };

    cursor = push_button(&mut line, inner, cursor, apply_label, &mut hitboxes.apply);
    spans.push(Span::styled(apply_label.to_string(), base_style));
    if !apply_enabled {
        hitboxes.apply = None;
    }
    line.push(' ');
    cursor += 1;
    spans.push(Span::styled(" ", base_style));
    cursor = push_button(&mut line, inner, cursor, dry_label, &mut hitboxes.dry_run);
    spans.push(Span::styled(dry_label.to_string(), base_style));

    line.push(' ');
    cursor += 1;
    spans.push(Span::styled(" ", base_style));
    let sudo_label = if app.include_sudo {
        "[Sudo: ON]"
    } else {
        "[Sudo: OFF]"
    };
    cursor = push_button(&mut line, inner, cursor, sudo_label, &mut hitboxes.sudo);
    spans.extend(sudo_status_spans(app.include_sudo, base_style, true));

    if app.snapshot_support.is_some() {
        line.push(' ');
        cursor += 1;
        spans.push(Span::styled(" ", base_style));
        let snapshot_label = if app.snapshot_enabled {
            "[Snapshot: ON]"
        } else {
            "[Snapshot: OFF]"
        };
        let _ = push_button(
            &mut line,
            inner,
            cursor,
            snapshot_label,
            &mut hitboxes.snapshot,
        );
        spans.push(Span::styled(snapshot_label.to_string(), base_style));
    }

    (ActionLine { spans }, hitboxes)
}

fn sudo_status_spans(sudo_on: bool, base_style: Style, bracketed: bool) -> Vec<Span<'static>> {
    let sudo_style = Style::default().fg(Color::Red);
    let status_style = if sudo_on {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };
    let mut spans = Vec::new();
    if bracketed {
        spans.push(Span::styled("[", base_style));
    }
    spans.push(Span::styled("Sudo:", sudo_style));
    spans.push(Span::styled(" ", base_style));
    spans.push(Span::styled(
        if sudo_on { "ON" } else { "OFF" },
        status_style,
    ));
    if bracketed {
        spans.push(Span::styled("]", base_style));
    }
    spans
}

fn push_button(
    line: &mut String,
    inner: Rect,
    cursor: u16,
    label: &str,
    target: &mut Option<Rect>,
) -> u16 {
    let start = cursor;
    line.push_str(label);
    let end = start.saturating_add(label.len() as u16);

    if inner.height >= 3 {
        let action_y = inner.y + 2;
        let max_x = inner.x.saturating_add(inner.width);
        let start_x = inner.x.saturating_add(start);
        let end_x = inner.x.saturating_add(end);
        if start_x < max_x && end_x <= max_x {
            *target = Some(Rect {
                x: start_x,
                y: action_y,
                width: label.len() as u16,
                height: 1,
            });
        }
    }

    end
}

fn draw_ui(frame: &mut ratatui::Frame<'_>, app: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(6),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.size());

    let items = app
        .rules
        .iter()
        .map(|state| {
            let enabled = if state.enabled { "x" } else { " " };
            let sudo = if state.rule.requires_sudo {
                " (sudo)"
            } else {
                ""
            };
            let size = state
                .scan
                .as_ref()
                .map(|scan| format!("{} / {}", format_size(scan.bytes, BINARY), scan.entries))
                .unwrap_or_else(|| "-".to_string());
            let content = format!("[{}] {}{}  {}", enabled, state.rule.label, sudo, size);
            ListItem::new(Line::from(content))
        })
        .collect::<Vec<_>>();

    let list_block = Block::default()
        .title("Cleanup Rules")
        .borders(Borders::ALL);
    let list_area = list_block.inner(chunks[0]);
    app.layout.list_area = Some(list_area);
    let list = List::new(items).block(list_block).highlight_style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(list, chunks[0], &mut app.list_state);

    let (bytes, entries) = app.total_selected();
    let summary = format!(
        "Selected: {} rules | {} | {} items",
        app.selected_rules().len(),
        format_size(bytes, BINARY),
        entries
    );
    let base_mode = Style::default().fg(Color::LightBlue);
    let mut mode_spans = Vec::new();
    mode_spans.push(Span::styled("Dry-run: ", base_mode));
    mode_spans.push(Span::styled(on_off(app.dry_run), base_mode));
    mode_spans.push(Span::styled(" | ", base_mode));
    mode_spans.extend(sudo_status_spans(app.include_sudo, base_mode, false));
    if let Some(support) = &app.snapshot_support {
        mode_spans.push(Span::styled(" | Snapshot: ", base_mode));
        mode_spans.push(Span::styled(on_off(app.snapshot_enabled), base_mode));
        mode_spans.push(Span::styled(" (", base_mode));
        mode_spans.push(Span::styled(support.label.to_string(), base_mode));
        mode_spans.push(Span::styled(")", base_mode));
    }
    let mode_line = Line::from(mode_spans);

    let (action_line, actions) = build_action_line(chunks[1], app);
    app.layout.actions = actions;

    let mut help_spans = vec![Span::raw(
        "Keys: j/k or arrows move | space toggle | r rescan | ",
    )];
    help_spans.push(Span::raw("PgUp/PgDn output | "));
    help_spans.push(Span::styled("d dry-run", Style::default().fg(Color::Green)));
    help_spans.push(Span::raw(" | "));
    help_spans.push(Span::styled("s sudo", Style::default().fg(Color::Red)));
    if app.snapshot_support.is_some() {
        help_spans.push(Span::raw(" | "));
        help_spans.push(Span::styled(
            "p snapshot",
            Style::default().fg(Color::Yellow),
        ));
    }
    help_spans.push(Span::raw(" | "));
    help_spans.push(Span::styled(
        "a/enter apply",
        Style::default().fg(Color::Red),
    ));
    help_spans.push(Span::raw(" | q quit | "));
    help_spans.push(Span::styled(
        "mouse: click, scroll list/output",
        Style::default().fg(Color::LightCyan),
    ));
    let status_block = Block::default().borders(Borders::ALL).title("Status");
    let summary_block = Paragraph::new(vec![
        Line::styled(summary, Style::default().fg(Color::Cyan)),
        mode_line,
        Line::from(action_line.spans),
        Line::from(help_spans),
    ])
    .block(status_block);
    frame.render_widget(summary_block, chunks[1]);

    let output_title = "Output".to_string();
    let output_block = Block::default().borders(Borders::ALL).title(output_title);
    let output_inner = output_block.inner(chunks[2]);
    app.layout.output_block_area = Some(chunks[2]);
    app.layout.output_area = Some(output_inner);
    let height = output_inner.height as usize;
    app.clamp_output_scroll();

    let show_scrollbar = height > 0 && output_inner.width > 1 && app.output_lines.len() > height;
    let mut output_text_area = output_inner;
    let scrollbar_area = if show_scrollbar {
        output_text_area.width = output_text_area.width.saturating_sub(1);
        Some(Rect {
            x: output_inner.x + output_inner.width - 1,
            y: output_inner.y,
            width: 1,
            height: output_inner.height,
        })
    } else {
        None
    };
    app.layout.output_scrollbar_area = scrollbar_area;

    frame.render_widget(output_block, chunks[2]);
    let lines = if app.output_lines.is_empty() {
        vec![Line::from("No output yet.")]
    } else {
        let max_offset = app.output_lines.len().saturating_sub(height);
        let offset = app.output_scroll.min(max_offset);
        let end = (offset + height).min(app.output_lines.len());
        app.output_lines[offset..end]
            .iter()
            .map(|line| Line::from(line.as_str()))
            .collect::<Vec<_>>()
    };
    let output_widget = Paragraph::new(lines);
    frame.render_widget(output_widget, output_text_area);

    if let Some(area) = scrollbar_area {
        let mut state = ScrollbarState::new(app.output_lines.len())
            .position(app.output_scroll)
            .viewport_content_length(height);
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        frame.render_stateful_widget(scrollbar, area, &mut state);
    }

    let message = if app.confirm_apply && app.confirm_requires_delete {
        if app.confirm_buffer.is_empty() {
            "Sudo mode: type DELETE to confirm".to_string()
        } else {
            format!("Type DELETE to confirm: {}", app.confirm_buffer)
        }
    } else if app.confirm_apply {
        if app.dry_run {
            "Run dry-run preview? (y/n)".to_string()
        } else {
            "Confirm delete? (y/n)".to_string()
        }
    } else {
        app.message.clone().unwrap_or_default()
    };
    let message_block =
        Paragraph::new(message).block(Block::default().borders(Borders::ALL).title("Message"));
    frame.render_widget(message_block, chunks[3]);
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
    stdout.execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}
