//! the tui state machine, independent of any terminal so it can be tested.
//!
//! state is per tab: each tab is one session with its own transcript, input and counters.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

/// a sliding window of streamed characters, for the live rate.
const RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
/// how long the delivered-message average is flashed for.
const FLASH_DURATION: std::time::Duration = std::time::Duration::from_millis(2400);
/// half-period of the flash.
const FLASH_PHASE: std::time::Duration = std::time::Duration::from_millis(400);
/// characters per token, matching the daemon's compaction estimate.
const CHARS_PER_TOKEN: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// a tool invocation: `text` is the tool name and its arguments.
    Tool,
    /// the output of the tool above it.
    ToolOutput,
    System,
    Error,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub model: String,
    pub turn_running: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// one session's view state.
pub struct Tab {
    pub session: String,
    pub title: String,
    pub transcript: Vec<Entry>,
    input: tui_input::Input,
    pub status: Status,
    pub scroll: u16,
    pub max_scroll: u16,
    pub follow: bool,
    pub tok_per_sec: f32,
    pub flash_highlight: bool,
    samples: std::collections::VecDeque<(std::time::Instant, usize)>,
    turn_chars: usize,
    active_started: Option<std::time::Instant>,
    active_elapsed: std::time::Duration,
    flash: Option<(f32, std::time::Instant)>,
    frozen: Option<f32>,
}

impl Tab {
    #[must_use]
    pub fn new(
        session: impl Into<String>,
        title: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let session = session.into();
        let short: String = session.chars().take(8).collect();
        let title = title.into();
        Self {
            session,
            title: if title.is_empty() { short } else { title },
            transcript: Vec::new(),
            input: tui_input::Input::default(),
            status: Status {
                model: model.into(),
                ..Status::default()
            },
            scroll: 0,
            max_scroll: 0,
            follow: true,
            tok_per_sec: 0.0,
            flash_highlight: false,
            samples: std::collections::VecDeque::new(),
            turn_chars: 0,
            active_started: None,
            active_elapsed: std::time::Duration::ZERO,
            flash: None,
            frozen: None,
        }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.transcript.push(Entry {
            role,
            text: text.into(),
        });
        if self.follow {
            self.scroll = self.max_scroll;
        }
    }

    #[must_use]
    pub fn input_text(&self) -> &str {
        self.input.value()
    }

    pub fn set_input(&mut self, text: &str) {
        self.input = tui_input::Input::new(text.to_owned());
    }

    #[must_use]
    pub fn input_view(&self, width: usize) -> (usize, usize) {
        let scroll = self.input.visual_scroll(width.max(1));
        (scroll, self.input.visual_cursor().saturating_sub(scroll))
    }

    fn edit(&mut self, key: KeyEvent) {
        use tui_input::backend::crossterm::EventHandler;
        self.input.handle_event(&crossterm::event::Event::Key(key));
    }

    fn submit(&mut self) -> Action {
        let text = self.input.value().trim().to_owned();
        if text.is_empty() {
            return Action::None;
        }
        self.input.reset();
        self.follow = true;
        if text.starts_with('/') {
            Action::Command(text)
        } else {
            Action::Submit(text)
        }
    }

    fn note_generation(&mut self, text: &str, now: std::time::Instant) {
        if self.active_started.is_none() {
            self.active_started = Some(now);
        }
        let chars = text.chars().count();
        if chars == 0 {
            return;
        }
        self.turn_chars += chars;
        self.samples.push_back((now, chars));
    }

    fn close_generation(&mut self, now: std::time::Instant) {
        if let Some(started) = self.active_started.take() {
            if let Some(delta) = now.checked_duration_since(started) {
                self.active_elapsed += delta;
            }
        }
    }

    fn finish_message(&mut self, now: std::time::Instant) {
        self.close_generation(now);
        let chars = self.turn_chars;
        let active = self.active_elapsed;
        self.samples.clear();
        self.turn_chars = 0;
        self.active_started = None;
        self.active_elapsed = std::time::Duration::ZERO;

        if chars == 0 || active.is_zero() {
            self.flash = None;
            self.flash_highlight = false;
            self.tok_per_sec = 0.0;
            return;
        }
        let average = (chars as f32 / CHARS_PER_TOKEN) / active.as_secs_f32().max(0.05);
        self.flash = Some((average, now));
        self.tok_per_sec = average;
        self.flash_highlight = true;
    }

    pub fn tick(&mut self) {
        self.tick_at(std::time::Instant::now());
    }

    pub fn tick_at(&mut self, now: std::time::Instant) {
        if let Some((rate, started)) = self.flash {
            let elapsed = now.duration_since(started);
            if elapsed >= FLASH_DURATION {
                self.flash = None;
                self.flash_highlight = false;
                self.frozen = Some(rate);
                self.tok_per_sec = rate;
                return;
            }
            self.tok_per_sec = rate;
            let phase = (elapsed.as_millis() / FLASH_PHASE.as_millis().max(1)) % 2;
            self.flash_highlight = phase == 0;
            return;
        }
        self.flash_highlight = false;
        while let Some((at, _)) = self.samples.front() {
            if now.duration_since(*at) > RATE_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        if self.samples.is_empty() {
            self.tok_per_sec = self.frozen.unwrap_or(0.0);
            return;
        }
        let chars: usize = self.samples.iter().map(|(_, n)| n).sum();
        let span = self
            .samples
            .front()
            .map(|(at, _)| now.duration_since(*at).as_secs_f32())
            .unwrap_or(0.0)
            .max(0.5)
            .min(RATE_WINDOW.as_secs_f32());
        self.tok_per_sec = (chars as f32 / CHARS_PER_TOKEN) / span;
    }

    /// applies a daemon event to this tab.
    pub fn on_event(&mut self, kind: &str, payload: &Value) {
        self.on_event_at(kind, payload, std::time::Instant::now());
    }

    pub fn on_event_at(&mut self, kind: &str, payload: &Value, now: std::time::Instant) {
        match kind {
            "llm.delta" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                self.note_generation(text, now);
                self.append_stream(text);
            }
            "llm.reasoning" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                self.note_generation(text, now);
            }
            "llm.completed" => self.close_generation(now),
            "tool.requested" => {
                self.close_generation(now);
                let tool = payload
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let args = summarise(&payload["args"]);
                self.push(Role::Tool, format!("{tool} {args}").trim_end().to_owned());
            }
            "tool.completed" => {
                let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
                self.push(Role::ToolOutput, indent(output, 12));
            }
            "tool.failed" => {
                let error = payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                self.push(Role::Error, error.to_owned());
            }
            "turn.completed" => {
                self.status.turn_running = false;
                self.finish_message(now);
                self.status.prompt_tokens +=
                    payload["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                self.status.completion_tokens +=
                    payload["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            }
            "turn.interrupted" => {
                self.status.turn_running = false;
                self.finish_message(now);
                let cause = payload.get("cause").and_then(Value::as_str).unwrap_or("");
                self.push(Role::Error, format!("interrupted: {cause}"));
            }
            "turn.started" => {
                if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
                    if self.transcript.last().map(|e| e.role) != Some(Role::User) {
                        self.push(Role::User, prompt.to_owned());
                    }
                }
                self.turn_chars = 0;
                self.active_started = None;
                self.active_elapsed = std::time::Duration::ZERO;
                self.samples.clear();
                self.frozen = None;
                self.tok_per_sec = 0.0;
            }
            "compaction.applied" => self.push(Role::System, "context compacted"),
            _ => {}
        }
    }

    fn append_stream(&mut self, text: &str) {
        match self.transcript.last_mut() {
            Some(entry) if entry.role == Role::Assistant && !entry.text.ends_with('\n') => {
                entry.text.push_str(text);
            }
            _ => {
                if !text.is_empty() {
                    self.push(Role::Assistant, text.to_owned());
                }
            }
        }
        if self.follow {
            self.scroll = self.max_scroll;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
    Command(String),
    NextTab,
    NewTab,
    CloseTab,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub branch: Option<String>,
    pub workspace: Option<String>,
}

impl App {
    #[must_use]
    pub fn new(session: impl Into<String>, model: impl Into<String>) -> Self {
        let tab = Tab::new(session, String::new(), model);
        Self {
            tabs: vec![tab],
            active: 0,
            branch: None,
            workspace: None,
        }
    }

    #[must_use]
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len().saturating_sub(1))]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let index = self.active.min(self.tabs.len().saturating_sub(1));
        &mut self.tabs[index]
    }

    /// the tab showing a given session, if any.
    pub fn tab_for(&mut self, session: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.session == session)
    }

    pub fn add_tab(&mut self, session: impl Into<String>, model: impl Into<String>) -> usize {
        self.tabs.push(Tab::new(session, String::new(), model));
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// closes the active tab. returns the session that was closed, if it closed.
    pub fn close_tab(&mut self) -> Option<String> {
        if self.tabs.len() <= 1 {
            return None;
        }
        let removed = self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        Some(removed.session)
    }

    pub fn next_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    /// handles a key press. returns what the caller should do about it.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl {
            return match key.code {
                KeyCode::Char('c') | KeyCode::Char('d') => Action::Quit,
                KeyCode::Char('N') | KeyCode::Char('n') if shift => Action::NewTab,
                KeyCode::Char('W') | KeyCode::Char('w') if shift => Action::CloseTab,
                _ => {
                    self.tab_mut().edit(key);
                    Action::None
                }
            };
        }

        match key.code {
            KeyCode::BackTab => Action::NextTab,
            KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r') => self.tab_mut().submit(),
            KeyCode::Up => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + 1).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                Action::None
            }
            KeyCode::PageUp => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + 10).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                Action::None
            }
            KeyCode::Esc => Action::Cancel,
            _ => {
                self.tab_mut().edit(key);
                Action::None
            }
        }
    }
}

fn summarise(args: &Value) -> String {
    for key in ["path", "command", "pattern"] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            return value.chars().take(90).collect();
        }
    }
    String::new()
}

fn indent(text: &str, max_lines: usize) -> String {
    let mut lines: Vec<&str> = text.lines().take(max_lines).collect();
    if text.lines().count() > max_lines {
        lines.push("\u{2026}");
    }
    lines.join("\n")
}

/// styles for one transcript entry, used by the renderer.
#[must_use]
pub fn entry_style(role: Role) -> (Style, &'static str, Color) {
    match role {
        Role::User => (
            Style::default().fg(Color::Rgb(0x8a, 0xc8, 0xff)),
            "you",
            Color::Rgb(0x8a, 0xc8, 0xff),
        ),
        Role::Assistant => (Style::default(), "sa", Color::Rgb(0x7f, 0xc7, 0x9a)),
        Role::Tool => (
            Style::default().fg(Color::Rgb(0x7a, 0x84, 0x94)),
            "tool",
            Color::Rgb(0x7a, 0x84, 0x94),
        ),
        Role::ToolOutput => (
            Style::default().fg(Color::Rgb(0x5f, 0x68, 0x74)),
            "output",
            Color::Rgb(0x4a, 0x52, 0x5c),
        ),
        Role::System => (
            Style::default().fg(Color::Rgb(0x6c, 0x76, 0x86)),
            "system",
            Color::Rgb(0x6c, 0x76, 0x86),
        ),
        Role::Error => (
            Style::default()
                .fg(Color::Rgb(0xe8, 0x7c, 0x7c))
                .add_modifier(Modifier::BOLD),
            "error",
            Color::Rgb(0xe8, 0x7c, 0x7c),
        ),
    }
}

/// renders the transcript as a stack of framed windows, one per entry.
#[must_use]
pub fn windows(app: &App, width: u16) -> Vec<Line<'static>> {
    let tab = app.tab();
    let inner = usize::from(width).saturating_sub(4).clamp(16, 160);
    let mut out: Vec<Line<'static>> = Vec::new();

    for entry in &tab.transcript {
        let (style, label, edge) = entry_style(entry.role);
        let head = format!("\u{256d}\u{2500} {label} ");
        let fill = inner.saturating_sub(head.chars().count().saturating_sub(1));
        out.push(Line::from(vec![
            Span::styled(head, Style::default().fg(edge)),
            Span::styled("\u{2500}".repeat(fill), Style::default().fg(edge)),
            Span::styled("\u{256e}", Style::default().fg(edge)),
        ]));
        for raw in entry.text.lines() {
            let mut text = raw.to_owned();
            if text.chars().count() > inner {
                text = text
                    .chars()
                    .take(inner.saturating_sub(1))
                    .collect::<String>();
                text.push('\u{2026}');
            }
            let pad = inner.saturating_sub(text.chars().count());
            out.push(Line::from(vec![
                Span::styled("\u{2502} ", Style::default().fg(edge)),
                Span::styled(text, style),
                Span::styled(
                    format!("{} \u{2502}", " ".repeat(pad)),
                    Style::default().fg(edge),
                ),
            ]));
        }
        out.push(Line::from(vec![
            Span::styled("\u{2570}", Style::default().fg(edge)),
            Span::styled("\u{2500}".repeat(inner + 1), Style::default().fg(edge)),
            Span::styled("\u{256f}", Style::default().fg(edge)),
        ]));
        out.push(Line::from(""));
    }
    out
}
