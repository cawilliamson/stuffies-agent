//! the tui state machine, independent of any terminal so it can be tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

const RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
const FLASH_DURATION: std::time::Duration = std::time::Duration::from_millis(2400);
const FLASH_PHASE: std::time::Duration = std::time::Duration::from_millis(400);
const CHARS_PER_TOKEN: f32 = 4.0;

pub const INK: Color = Color::Rgb(0xc8, 0xd0, 0xdc);
pub const DIM: Color = Color::Rgb(0x6c, 0x76, 0x86);
pub const FAINT: Color = Color::Rgb(0x44, 0x4c, 0x58);
pub const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);
pub const OK: Color = Color::Rgb(0x7f, 0xc7, 0x9a);
pub const BAD: Color = Color::Rgb(0xe8, 0x7c, 0x7c);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    ToolOutput,
    /// the model's reasoning trace.
    Thinking,
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
    /// bumped on every visible change so the renderer can skip clean frames.
    pub revision: u64,
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
            revision: 1,
        }
    }

    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.transcript.push(Entry {
            role,
            text: text.into(),
        });
        if self.follow {
            self.scroll = self.max_scroll;
        }
        self.touch();
    }

    #[must_use]
    pub fn input_text(&self) -> &str {
        self.input.value()
    }

    pub fn set_input(&mut self, text: &str) {
        self.input = tui_input::Input::new(text.to_owned());
        self.touch();
    }

    #[must_use]
    pub fn input_view(&self, width: usize) -> (usize, usize) {
        let scroll = self.input.visual_scroll(width.max(1));
        (scroll, self.input.visual_cursor().saturating_sub(scroll))
    }

    fn edit(&mut self, key: KeyEvent) {
        use tui_input::backend::crossterm::EventHandler;
        self.input.handle_event(&crossterm::event::Event::Key(key));
        self.touch();
    }

    fn submit(&mut self) -> Action {
        let text = self.input.value().trim().to_owned();
        if text.is_empty() {
            return Action::None;
        }
        self.input.reset();
        self.follow = true;
        self.touch();
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
            self.touch();
            return;
        }
        let average = (chars as f32 / CHARS_PER_TOKEN) / active.as_secs_f32().max(0.05);
        self.flash = Some((average, now));
        self.tok_per_sec = average;
        self.flash_highlight = true;
        self.touch();
    }

    pub fn tick(&mut self) {
        self.tick_at(std::time::Instant::now());
    }

    pub fn tick_at(&mut self, now: std::time::Instant) {
        let before = (self.tok_per_sec * 10.0) as i64;
        let lit_before = self.flash_highlight;

        if let Some((rate, started)) = self.flash {
            let elapsed = now.duration_since(started);
            if elapsed >= FLASH_DURATION {
                self.flash = None;
                self.flash_highlight = false;
                self.frozen = Some(rate);
                self.tok_per_sec = rate;
            } else {
                self.tok_per_sec = rate;
                let phase = (elapsed.as_millis() / FLASH_PHASE.as_millis().max(1)) % 2;
                self.flash_highlight = phase == 0;
            }
        } else {
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
            } else {
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
        }

        // only dirty the frame when the displayed figure actually changes.
        if (self.tok_per_sec * 10.0) as i64 != before || self.flash_highlight != lit_before {
            self.touch();
        }
    }

    pub fn on_event(&mut self, kind: &str, payload: &Value) {
        self.on_event_at(kind, payload, std::time::Instant::now());
    }

    pub fn on_event_at(&mut self, kind: &str, payload: &Value, now: std::time::Instant) {
        match kind {
            "llm.delta" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                self.note_generation(text, now);
                self.append_stream_as(Role::Assistant, text);
            }
            "llm.reasoning" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                self.note_generation(text, now);
                self.append_stream_as(Role::Thinking, text);
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
                self.touch();
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
                self.touch();
            }
            "compaction.applied" => self.push(Role::System, "context compacted"),
            _ => {}
        }
    }

    fn append_stream_as(&mut self, role: Role, text: &str) {
        match self.transcript.last_mut() {
            Some(entry) if entry.role == role && !entry.text.ends_with('\n') => {
                entry.text.push_str(text);
            }
            _ => {
                if !text.is_empty() {
                    self.transcript.push(Entry {
                        role,
                        text: text.to_owned(),
                    });
                }
            }
        }
        if self.follow {
            self.scroll = self.max_scroll;
        }
        self.touch();
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
    pub revision: u64,
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
            revision: 1,
        }
    }

    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        let active = self.active.min(self.tabs.len().saturating_sub(1));
        if let Some(tab) = self.tabs.get_mut(active) {
            tab.touch();
        }
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
            .wrapping_mul(31)
            .wrapping_add(self.tab().revision)
    }

    #[must_use]
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len().saturating_sub(1))]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let index = self.active.min(self.tabs.len().saturating_sub(1));
        &mut self.tabs[index]
    }

    pub fn tab_for(&mut self, session: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.session == session)
    }

    pub fn add_tab(&mut self, session: impl Into<String>, model: impl Into<String>) -> usize {
        self.tabs.push(Tab::new(session, String::new(), model));
        self.active = self.tabs.len() - 1;
        self.touch();
        self.active
    }

    pub fn close_tab(&mut self) -> Option<String> {
        if self.tabs.len() <= 1 {
            return None;
        }
        let removed = self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        self.touch();
        Some(removed.session)
    }

    pub fn next_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active = (self.active + 1) % self.tabs.len();
            self.touch();
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl {
            return match key.code {
                KeyCode::Char('c') | KeyCode::Char('d') => Action::Quit,
                // plain ctrl+n / ctrl+w, with the shifted forms accepted as aliases.
                KeyCode::Char('n') | KeyCode::Char('N') => Action::NewTab,
                KeyCode::Char('w') | KeyCode::Char('W') => Action::CloseTab,
                _ => {
                    let _ = shift;
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
                tab.touch();
                Action::None
            }
            KeyCode::Down => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + 1).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                tab.touch();
                Action::None
            }
            KeyCode::PageUp => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(10);
                tab.touch();
                Action::None
            }
            KeyCode::PageDown => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + 10).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                tab.touch();
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

/// the label and edge colour for a role.
#[must_use]
pub fn role_style(role: Role, model: &str) -> (String, Color, Color) {
    let label = match role {
        Role::User => "you".to_owned(),
        // the assistant speaks as whichever model produced it.
        Role::Assistant => {
            if model.is_empty() {
                "assistant".to_owned()
            } else {
                model.to_owned()
            }
        }
        Role::Thinking => "thinking".to_owned(),
        Role::Tool => "tool".to_owned(),
        Role::ToolOutput => "output".to_owned(),
        Role::System => "system".to_owned(),
        Role::Error => "error".to_owned(),
    };
    let (edge, body) = match role {
        Role::User => (ACCENT, Color::Rgb(0xe8, 0xf0, 0xff)),
        Role::Assistant => (OK, Color::Rgb(0xd8, 0xe8, 0xdc)),
        Role::Thinking => (Color::Rgb(0xc7, 0x9b, 0xff), Color::Rgb(0xb0, 0x9c, 0xc8)),
        Role::Tool => (Color::Rgb(0xe0, 0xb0, 0x60), Color::Rgb(0xd8, 0xc8, 0xa8)),
        Role::ToolOutput => (FAINT, DIM),
        Role::System => (DIM, DIM),
        Role::Error => (BAD, Color::Rgb(0xf0, 0xc0, 0xc0)),
    };
    (label, edge, body)
}

/// pushes one body section: each row is exactly `width` cells.
fn body(out: &mut Vec<Line<'static>>, text: &str, width: usize, edge: Color, ink: Color) {
    let room = width.saturating_sub(4);
    for raw in text.lines() {
        let mut line = raw.to_owned();
        if line.chars().count() > room {
            line = line
                .chars()
                .take(room.saturating_sub(1))
                .collect::<String>();
            line.push('\u{2026}');
        }
        let pad = room.saturating_sub(line.chars().count());
        out.push(Line::from(vec![
            Span::styled("\u{2502} ", Style::default().fg(edge)),
            Span::styled(line, Style::default().fg(ink)),
            Span::styled(
                format!("{} \u{2502}", " ".repeat(pad)),
                Style::default().fg(edge),
            ),
        ]));
    }
}

fn top(out: &mut Vec<Line<'static>>, label: &str, width: usize, edge: Color) {
    let head = format!("\u{256d}\u{2500} {label} ");
    let fill = width.saturating_sub(head.chars().count() + 1);
    out.push(Line::from(vec![
        Span::styled(head, Style::default().fg(edge)),
        Span::styled("\u{2500}".repeat(fill), Style::default().fg(edge)),
        Span::styled("\u{256e}", Style::default().fg(edge)),
    ]));
}

fn divider(out: &mut Vec<Line<'static>>, width: usize, edge: Color) {
    out.push(Line::from(vec![
        Span::styled("\u{251c}", Style::default().fg(edge)),
        Span::styled(
            "\u{2500}".repeat(width.saturating_sub(2)),
            Style::default().fg(edge),
        ),
        Span::styled("\u{2524}", Style::default().fg(edge)),
    ]));
}

fn bottom(out: &mut Vec<Line<'static>>, width: usize, edge: Color) {
    out.push(Line::from(vec![
        Span::styled("\u{2570}", Style::default().fg(edge)),
        Span::styled(
            "\u{2500}".repeat(width.saturating_sub(2)),
            Style::default().fg(edge),
        ),
        Span::styled("\u{256f}", Style::default().fg(edge)),
    ]));
}

/// renders one transcript entry as a titled frame.
fn framed(role: Role, text: &str, width: usize, model: &str) -> Vec<Line<'static>> {
    let (label, edge, ink) = role_style(role, model);
    let mut out = Vec::new();
    top(&mut out, &label, width, edge);
    body(&mut out, text, width, edge, ink);
    bottom(&mut out, width, edge);
    out
}

/// renders a turn's reasoning and its reply as one frame split by a rule, rather than two
/// separate boxes.
fn framed_split(model: &str, thinking: &str, message: &str, width: usize) -> Vec<Line<'static>> {
    let (label, edge, _) = role_style(Role::Assistant, model);
    let (_, think_edge, think_ink) = role_style(Role::Thinking, model);
    let mut out = Vec::new();
    top(&mut out, &label, width, edge);
    body(&mut out, thinking, width, edge, think_ink);
    divider(&mut out, width, think_edge);
    body(&mut out, message, width, edge, Color::Rgb(0xd8, 0xe8, 0xdc));
    bottom(&mut out, width, edge);
    out
}

/// the transcript of a tab as renderable lines.
///
/// reasoning and the reply it produced share one frame: the reasoning sits above a rule and
/// the reply below it.
#[must_use]
pub fn transcript_lines(tab: &Tab, width: u16) -> Vec<Line<'static>> {
    let inner = usize::from(width).clamp(16, 160);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut index = 0;

    while index < tab.transcript.len() {
        let entry = &tab.transcript[index];

        if entry.role == Role::Thinking {
            // gather the reasoning run, then see whether a reply follows it.
            let mut thinking = entry.text.clone();
            let mut next = index + 1;
            while next < tab.transcript.len() && tab.transcript[next].role == Role::Thinking {
                thinking.push('\n');
                thinking.push_str(&tab.transcript[next].text);
                next += 1;
            }
            if next < tab.transcript.len() && tab.transcript[next].role == Role::Assistant {
                let message = tab.transcript[next].text.clone();
                out.extend(framed_split(&tab.status.model, &thinking, &message, inner));
                index = next + 1;
            } else {
                out.extend(framed(Role::Thinking, &thinking, inner, &tab.status.model));
                index = next;
            }
            continue;
        }

        out.extend(framed(entry.role, &entry.text, inner, &tab.status.model));
        index += 1;
    }

    out
}
