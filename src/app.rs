//! The tangents application: transparent PTY wrapper (Phase 1), live session-tree
//! sidebar (Phase 2), forking (Phase 3), branch switching (Phase 4), and polish
//! (Phase 5: status bar/breadcrumb, rename, depth colour, help, config).
//!
//! Input model: the terminal pane is transparent — keystrokes are re-encoded and
//! forwarded to claude — *except* the tmux-style prefix key (default Ctrl+G).
//! Prefix then a command key drives tangents. While the tree has focus,
//! navigation keys are consumed by tangents, not forwarded.
//!
//! Fork/switch both tear down the current claude PTY and spawn a new one with
//! different `--resume`/`--fork-session` args (there is no `/branch` command).
//! Each PTY carries a generation tag so output/exit from a killed child is
//! ignored rather than quitting tangents.

use std::io::{self, Stdout, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tui_term::widget::PseudoTerminal;

use crate::cli::{self, Config};
use crate::config::TangentsConfig;
use crate::event::{Event, EventTx, Generation};
use crate::keys::{encode_key, encode_paste};
use crate::pty::PtySession;
use crate::session::{self, SessionInfo};
use crate::state::TangentsState;
use crate::tree::{self, TreePanel};
use crate::watcher;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Terminal,
    Tree,
}

/// A transient text-entry mode (currently just branch rename).
enum InputMode {
    Normal,
    Rename { target: String, buffer: String },
}

/// Restores the real terminal on drop, even on panic.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
            Show
        );
        let _ = io::stdout().flush();
    }
}

/// Split the screen into (optional tree pane, terminal pane, status row).
fn compute_layout(area: Rect, tree_visible: bool, tree_width: u16) -> (Option<Rect>, Rect, Rect) {
    let vchunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let (content, status) = (vchunks[0], vchunks[1]);
    if !tree_visible || content.width < 40 {
        return (None, content, status);
    }
    let tree_w = tree_width
        .clamp(24, 50)
        .min(content.width.saturating_sub(20));
    let hchunks =
        Layout::horizontal([Constraint::Length(tree_w), Constraint::Min(1)]).split(content);
    (Some(hchunks[0]), hchunks[1], status)
}

/// Most-recently-modified session id (fallback when we don't know the current).
fn newest_session(project_dir: &Path) -> Option<String> {
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(project_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        if let (Some(mtime), Some(id)) = (mtime, id)
            && best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                best = Some((mtime, id));
            }
    }
    best.map(|(_, id)| id)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let h = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .split(area)[0];
    Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .split(h)[0]
}

pub struct App {
    parser: vt100::Parser,
    pty: PtySession,
    pty_generation: Generation,
    rows: u16,
    cols: u16,
    should_quit: bool,

    cfg: Config,
    tx: EventTx,
    prefix: char,
    tree_width: u16,
    base_args: Vec<String>,
    current_session_id: Option<String>,

    state: TangentsState,
    sessions: Vec<SessionInfo>,
    tree: TreePanel,
    focus: Focus,
    pending_prefix: bool,
    input_mode: InputMode,
    show_help: bool,
}

impl App {
    fn new(cfg: Config, tx: EventTx) -> Result<Self> {
        let ui_cfg = TangentsConfig::load(&cfg.tangents_dir);
        let visible = !(cfg.no_tree || ui_cfg.no_tree);

        let (full_cols, full_rows) = crossterm::terminal::size()?;
        let area = Rect::new(0, 0, full_cols, full_rows);
        let (_tree_rect, term_rect, _status) = compute_layout(area, visible, ui_cfg.tree_width);
        let (rows, cols) = (term_rect.height.max(1), term_rect.width.max(1));

        let (launch_args, current_session_id) = cli::prepare_initial(&cfg.claude_args);
        let base_args = cli::strip_session_flags(&cfg.claude_args);

        let parser = vt100::Parser::new(rows, cols, 0);
        let pty = PtySession::spawn(
            0,
            &cfg.claude_bin.to_string_lossy(),
            &launch_args,
            &cfg.cwd,
            rows,
            cols,
            tx.clone(),
        )?;

        let state = TangentsState::load(&cfg.tangents_dir);
        let sessions = session::scan(&cfg.project_dir);
        let mut tree = TreePanel::new(visible);
        tree.rebuild(&sessions, &state);

        Ok(Self {
            parser,
            pty,
            pty_generation: 0,
            rows,
            cols,
            should_quit: false,
            cfg,
            tx,
            prefix: ui_cfg.prefix.to_ascii_lowercase(),
            tree_width: ui_cfg.tree_width,
            base_args,
            current_session_id,
            state,
            sessions,
            tree,
            focus: Focus::Terminal,
            pending_prefix: false,
            input_mode: InputMode::Normal,
            show_help: false,
        })
    }

    fn rescan(&mut self) {
        self.sessions = session::scan(&self.cfg.project_dir);
        self.tree.rebuild(&self.sessions, &self.state);
    }

    fn handle(&mut self, ev: Event) -> bool {
        match ev {
            Event::PtyOutput(generation, bytes) => {
                if generation == self.pty_generation {
                    self.parser.process(&bytes);
                    return true;
                }
                false
            }
            Event::PtyExit(generation) => {
                if generation == self.pty_generation {
                    self.should_quit = true;
                }
                false
            }
            Event::Input(ct) => self.handle_input(ct),
            Event::SessionsChanged => {
                self.rescan();
                true
            }
        }
    }

    fn handle_input(&mut self, ct: CtEvent) -> bool {
        match ct {
            CtEvent::Key(k) => {
                if matches!(k.kind, KeyEventKind::Release) {
                    return false;
                }
                if self.show_help {
                    self.show_help = false;
                    return true;
                }
                if matches!(self.input_mode, InputMode::Rename { .. }) {
                    return self.handle_rename_key(k);
                }
                if self.pending_prefix {
                    self.pending_prefix = false;
                    return self.handle_prefix_command(k);
                }
                if self.focus == Focus::Tree {
                    return self.handle_tree_key(k);
                }
                if self.is_prefix(&k) {
                    self.pending_prefix = true;
                    return false;
                }
                if let Some(bytes) = encode_key(&k) {
                    let _ = self.pty.write(&bytes);
                }
                false
            }
            CtEvent::Paste(s) => {
                if self.focus == Focus::Terminal && matches!(self.input_mode, InputMode::Normal) {
                    let _ = self.pty.write(&encode_paste(&s));
                }
                false
            }
            CtEvent::Resize(_, _) => true,
            _ => false,
        }
    }

    fn handle_prefix_command(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.tree.toggle_visible();
                if !self.tree.visible {
                    self.focus = Focus::Terminal;
                }
                true
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                self.fork_current();
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.start_rename();
                true
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                true
            }
            KeyCode::Enter => {
                if let Some(id) = self.tree.selected_id() {
                    self.switch_to(&id);
                }
                true
            }
            KeyCode::Char(c)
                if c.eq_ignore_ascii_case(&self.prefix)
                    && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // Prefix twice: send a literal prefix keystroke to claude.
                if let Some(bytes) = encode_key(&k) {
                    let _ = self.pty.write(&bytes);
                }
                false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.enter_tree();
                self.tree.key_up();
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.enter_tree();
                self.tree.key_down();
                true
            }
            KeyCode::Char('w') => {
                self.focus = match self.focus {
                    Focus::Terminal => {
                        self.tree.visible = true;
                        Focus::Tree
                    }
                    Focus::Tree => Focus::Terminal,
                };
                true
            }
            _ => true,
        }
    }

    fn handle_tree_key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.tree.key_up(),
            KeyCode::Down | KeyCode::Char('j') => self.tree.key_down(),
            KeyCode::Enter => {
                if let Some(id) = self.tree.selected_id() {
                    self.switch_to(&id);
                }
                true
            }
            KeyCode::Char(' ') => self.tree.toggle_selected(),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.start_rename();
                true
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let sel = self.tree.state.selected().to_vec();
                self.tree.state.open(sel)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let sel = self.tree.state.selected().to_vec();
                self.tree.state.close(&sel)
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.focus = Focus::Terminal;
                true
            }
            _ if self.is_prefix(&k) => {
                self.focus = Focus::Terminal;
                true
            }
            _ => false,
        }
    }

    fn handle_rename_key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Enter => {
                if let InputMode::Rename { target, buffer } = &self.input_mode {
                    let name = buffer.trim();
                    let value = if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    };
                    let target = target.clone();
                    self.state.set_name(&target, value);
                    let _ = self.state.save();
                }
                self.input_mode = InputMode::Normal;
                self.rescan();
                true
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                true
            }
            KeyCode::Backspace => {
                if let InputMode::Rename { buffer, .. } = &mut self.input_mode {
                    buffer.pop();
                }
                true
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                if let InputMode::Rename { buffer, .. } = &mut self.input_mode {
                    buffer.push(c);
                }
                true
            }
            _ => false,
        }
    }

    fn start_rename(&mut self) {
        let target = self
            .tree
            .selected_id()
            .or_else(|| self.resolve_current_id());
        if let Some(target) = target {
            let buffer = self
                .state
                .meta(&target)
                .and_then(|m| m.name.clone())
                .unwrap_or_default();
            self.input_mode = InputMode::Rename { target, buffer };
        }
    }

    fn enter_tree(&mut self) {
        self.tree.visible = true;
        self.focus = Focus::Tree;
    }

    fn is_prefix(&self, k: &KeyEvent) -> bool {
        k.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(k.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&self.prefix))
    }

    fn resolve_current_id(&self) -> Option<String> {
        self.current_session_id
            .clone()
            .or_else(|| newest_session(&self.cfg.project_dir))
    }

    fn build_fork_args(&self, parent: &str, new_id: &str) -> Vec<String> {
        let mut a = self.base_args.clone();
        a.push("--resume".into());
        a.push(parent.into());
        a.push("--fork-session".into());
        a.push("--session-id".into());
        a.push(new_id.into());
        a
    }

    fn build_resume_args(&self, target: &str) -> Vec<String> {
        let mut a = self.base_args.clone();
        a.push("--resume".into());
        a.push(target.into());
        a
    }

    fn fork_current(&mut self) {
        let parent = match self.resolve_current_id() {
            Some(p) => p,
            None => return,
        };
        let new_id = uuid::Uuid::new_v4().to_string();
        let args = self.build_fork_args(&parent, &new_id);
        self.state.record_fork(&new_id, &parent);
        self.state.set_active(Some(new_id.clone()));
        let _ = self.state.save();
        self.respawn(args, Some(new_id));
        self.focus = Focus::Terminal;
    }

    fn switch_to(&mut self, target: &str) {
        self.focus = Focus::Terminal;
        if self.current_session_id.as_deref() == Some(target) {
            return;
        }
        let args = self.build_resume_args(target);
        self.state.set_active(Some(target.to_string()));
        let _ = self.state.save();
        self.respawn(args, Some(target.to_string()));
    }

    fn respawn(&mut self, args: Vec<String>, new_id: Option<String>) {
        self.pty.kill();
        self.pty_generation += 1;
        self.parser = vt100::Parser::new(self.rows, self.cols, 0);
        match PtySession::spawn(
            self.pty_generation,
            &self.cfg.claude_bin.to_string_lossy(),
            &args,
            &self.cfg.cwd,
            self.rows,
            self.cols,
            self.tx.clone(),
        ) {
            Ok(p) => {
                self.pty = p;
                self.current_session_id = new_id;
            }
            Err(_) => self.should_quit = true,
        }
    }

    fn resize_pty(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        self.parser.screen_mut().set_size(self.rows, self.cols);
        let _ = self.pty.resize(self.rows, self.cols);
    }

    fn status_line(&self) -> Line<'static> {
        match &self.input_mode {
            InputMode::Rename { buffer, .. } => Line::from(vec![
                Span::styled(
                    " rename ▸ ",
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                ),
                Span::styled(
                    format!("{buffer}▏"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  (Enter save · Esc cancel)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            InputMode::Normal => {
                let mut spans = vec![Span::raw(" ")];
                if let Some(cur) = self.resolve_current_id() {
                    let crumbs = tree::breadcrumb(&self.sessions, &self.state, &cur);
                    let last = crumbs.len().saturating_sub(1);
                    for (i, c) in crumbs.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(" › ", Style::default().fg(Color::DarkGray)));
                        }
                        let style = if i == last {
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Gray)
                        };
                        spans.push(Span::styled(c.clone(), style));
                    }
                }
                let hint = format!(
                    "  ^{p}: t tree · b branch · ⏎ switch · r rename · ? help",
                    p = self.prefix.to_ascii_uppercase()
                );
                spans.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));
                Line::from(spans)
            }
        }
    }

    fn draw(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let (tree_rect, term_rect, status_rect) =
            compute_layout(area, self.tree.visible, self.tree_width);
        self.resize_pty(term_rect.height, term_rect.width);

        let active = self.state.active.clone();
        let focus = self.focus;
        let status = self.status_line();
        let show_help = self.show_help;
        let renaming = matches!(self.input_mode, InputMode::Rename { .. });
        let screen = self.parser.screen();
        let tree = &mut self.tree;

        terminal.draw(|f| {
            f.render_widget(PseudoTerminal::new(screen), term_rect);
            if let Some(tr) = tree_rect {
                tree.render(f, tr, focus == Focus::Tree, active.as_deref());
            }
            f.render_widget(
                Paragraph::new(status).style(Style::default().bg(Color::Rgb(24, 24, 32))),
                status_rect,
            );
            if focus == Focus::Terminal && !renaming && !show_help && !screen.hide_cursor() {
                let (row, col) = screen.cursor_position();
                let x = term_rect.x + col;
                let y = term_rect.y + row;
                if x < term_rect.right() && y < term_rect.bottom() {
                    f.set_cursor_position(Position { x, y });
                }
            }
            if show_help {
                render_help(f, area);
            }
        })?;
        Ok(())
    }
}

fn render_help(f: &mut ratatui::Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "tangents — keybindings",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Prefix = Ctrl+G, then:"),
        Line::from("   t   toggle tree panel"),
        Line::from("   b   branch (fork current session)"),
        Line::from("   ⏎   switch to selected branch"),
        Line::from("   r   rename selected/current branch"),
        Line::from("   j/k ↑/↓  move focus into tree & navigate"),
        Line::from("   w   toggle focus (terminal ⇄ tree)"),
        Line::from("   ?   this help"),
        Line::from(""),
        Line::from("  In the tree:  ⏎ switch · space expand · h/l collapse/open · Esc back"),
        Line::from("  Everything else goes straight to claude."),
        Line::from(""),
        Line::from(Span::styled(
            "  press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let popup = centered_rect(area, 62, lines.len() as u16 + 2);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" help ");
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Run tangents to completion.
pub fn run(cfg: Config) -> Result<()> {
    let (tx, rx) = mpsc::channel::<Event>();

    let mut guard = TerminalGuard::enter()?;

    {
        let tx = tx.clone();
        thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                if tx.send(Event::Input(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let _watcher = watcher::watch(&cfg.project_dir, tx.clone()).ok();

    let mut app = App::new(cfg, tx)?;
    app.draw(&mut guard.terminal)?;

    while let Ok(ev) = rx.recv() {
        let mut dirty = app.handle(ev);
        while let Ok(ev) = rx.try_recv() {
            dirty |= app.handle(ev);
            if app.should_quit {
                break;
            }
        }
        if app.should_quit {
            break;
        }
        if dirty {
            app.draw(&mut guard.terminal)?;
        }
    }

    app.pty.kill();
    Ok(())
}
