use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use humansize::{format_size, BINARY};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
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

#[derive(Debug, Default, Clone)]
struct ActionHitboxes {
    apply: Option<Rect>,
    dry_run: Option<Rect>,
    snapshot: Option<Rect>,
}

#[derive(Debug, Default, Clone)]
struct UiLayout {
    list_area: Option<Rect>,
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
    message: Option<String>,
    sudo_reexec_args: Option<Vec<String>>,
    layout: UiLayout,
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
            layout: UiLayout::default(),
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
            app.move_selection(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_selection(-1);
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
            begin_apply(app);
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
            if in_list_area(app, col, row) {
                app.move_selection(1);
            }
        }
        MouseEventKind::ScrollUp => {
            if in_list_area(app, col, row) {
                app.move_selection(-1);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if handle_action_click(app, col, row) {
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
        _ => {}
    }

    Ok(None)
}

fn begin_apply(app: &mut AppState) {
    if app.dry_run {
        app.message = Some("Disable dry-run before applying".to_string());
    } else if app.selected_rules().is_empty() {
        app.message = Some("No rules selected".to_string());
    } else {
        app.confirm_apply = true;
    }
}

fn handle_action_click(app: &mut AppState, col: u16, row: u16) -> bool {
    let actions = &app.layout.actions;
    if let Some(rect) = actions.apply {
        if contains(rect, col, row) {
            begin_apply(app);
            return true;
        }
    }
    if let Some(rect) = actions.dry_run {
        if contains(rect, col, row) {
            app.dry_run = !app.dry_run;
            return true;
        }
    }
    if let Some(rect) = actions.snapshot {
        if contains(rect, col, row) {
            app.toggle_snapshot();
            return true;
        }
    }
    false
}

fn in_list_area(app: &AppState, col: u16, row: u16) -> bool {
    app.layout
        .list_area
        .map(|rect| contains(rect, col, row))
        .unwrap_or(false)
}

fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn build_action_line(area: Rect, app: &AppState) -> (String, ActionHitboxes) {
    let block = Block::default().borders(Borders::ALL).title("Status");
    let inner = block.inner(area);

    let mut line = String::from("Actions: ");
    let mut hitboxes = ActionHitboxes::default();

    let mut cursor = line.len() as u16;
    let apply_label = "[Apply]";
    let dry_label = if app.dry_run { "[Dry-run: ON]" } else { "[Dry-run: OFF]" };

    cursor = push_button(&mut line, inner, cursor, apply_label, &mut hitboxes.apply);
    line.push(' ');
    cursor += 1;
    cursor = push_button(&mut line, inner, cursor, dry_label, &mut hitboxes.dry_run);

    if app.snapshot_support.is_some() {
        line.push(' ');
        cursor += 1;
        let snapshot_label = if app.snapshot_enabled {
            "[Snapshot: ON]"
        } else {
            "[Snapshot: OFF]"
        };
        let _ = push_button(&mut line, inner, cursor, snapshot_label, &mut hitboxes.snapshot);
    }

    (line, hitboxes)
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

    let list_block = Block::default().title("Cleanup Rules").borders(Borders::ALL);
    let list_area = list_block.inner(chunks[0]);
    app.layout.list_area = Some(list_area);
    let list = List::new(items)
        .block(list_block)
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

    let (action_line, actions) = build_action_line(chunks[1], app);
    app.layout.actions = actions;

    let mut help = "Keys: j/k or arrows move | space toggle | r rescan | d dry-run | s sudo".to_string();
    if app.snapshot_support.is_some() {
        help.push_str(" | p snapshot");
    }
    help.push_str(" | a apply | q quit | mouse: click, scroll");
    let status_block = Block::default().borders(Borders::ALL).title("Status");
    let summary_block = Paragraph::new(vec![
        Line::from(summary),
        Line::from(mode),
        Line::from(action_line),
        Line::from(help),
    ])
    .block(status_block);
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
    stdout.execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
