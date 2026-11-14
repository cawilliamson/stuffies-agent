//! the tui state machine, independent of any terminal so it can be tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
    Error,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
    Command(String),
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub session: String,
    pub model: String,
    pub turn_running: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// a sliding window of streamed characters, for the live rate.
const RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
/// how long the delivered-message average is flashed for: three on-phases of
/// `FLASH_PHASE` with an equal gap between them.
const FLASH_DURATION: std::time::Duration = std::time::Duration::from_millis(2400);
/// half-period of the flash. must stay comfortably longer than the redraw ticker in the
/// run loop, or sampling aliases and the on-phases are never drawn.
const FLASH_PHASE: std::time::Duration = std::time::Duration::from_millis(400);
/// characters per token, matching the daemon's compaction estimate.
const CHARS_PER_TOKEN: f32 = 4.0;

pub struct App {
    pub transcript: Vec<Entry>,
    pub input: String,
    /// absolute offset into the rendered transcript lines, not into entries.
    pub scroll: u16,
    /// the largest useful offset, recomputed on every draw from the real line count and
    /// the viewport height.
    pub max_scroll: u16,
    pub follow: bool,
    pub status: Status,
    /// true while replaying stored history. `turn.started` is rendered then, but not during
    /// a live turn: the prompt was already echoed when it was submitted.
    pub replaying: bool,
    /// tokens per second. always a number: the live rate while streaming, the flashing
    /// average of the last delivered message just after one lands, and zero otherwise.
    pub tok_per_sec: f32,
    samples: std::collections::VecDeque<(std::time::Instant, usize)>,
    /// characters and start time of the message currently being delivered.
    turn_chars: usize,
    turn_started: Option<std::time::Instant>,
    /// the average of the last delivered message, while it is being flashed.
    flash: Option<(f32, std::time::Instant)>,
}

impl App {
    #[must_use]
    pub fn new(session: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            scroll: 0,
            max_scroll: 0,
            follow: true,
            status: Status {
                session: session.into(),
                model: model.into(),
                ..Status::default()
            },
            replaying: false,
            tok_per_sec: 0.0,
            samples: std::collections::VecDeque::new(),
            turn_chars: 0,
            turn_started: None,
            flash: None,
        }
    }

    /// advances the live rate. called on a timer while a turn is streaming.
    pub fn tick(&mut self) {
        self.tick_at(std::time::Instant::now());
    }

    /// `tick` with an injected clock, so the flash is testable without sleeping.
    pub fn tick_at(&mut self, now: std::time::Instant) {
        // a delivered message flashes its average before settling back to zero.
        if let Some((rate, started)) = self.flash {
            let elapsed = now.duration_since(started);
            if elapsed >= FLASH_DURATION {
                self.flash = None;
                self.tok_per_sec = 0.0;
                return;
            }
            let phase = (elapsed.as_millis() / FLASH_PHASE.as_millis().max(1)) % 2;
            self.tok_per_sec = if phase == 0 { rate } else { 0.0 };
            return;
        }

        while let Some((at, _)) = self.samples.front() {
            if now.duration_since(*at) > RATE_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        if self.samples.is_empty() {
            self.tok_per_sec = 0.0;
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

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.transcript.push(Entry {
            role,
            text: text.into(),
        });
        // the draw pass pins the viewport to the newest content when following.
        if self.follow {
            self.scroll = self.max_scroll;
        }
    }

    /// handles a key press. returns what the caller should do about it.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                // ctrl-c always leaves.
                KeyCode::Char('c') | KeyCode::Char('d') => Action::Quit,
                _ => Action::None,
            };
        }

        match key.code {
            KeyCode::Enter => {
                let text = self.input.trim().to_owned();
                if text.is_empty() {
                    return Action::None;
                }
                self.input.clear();
                self.follow = true;
                if text.starts_with('/') {
                    Action::Command(text)
                } else {
                    Action::Submit(text)
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                Action::None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                Action::None
            }
            KeyCode::Up => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.scroll = (self.scroll + 1).min(self.max_scroll);
                self.follow = self.scroll >= self.max_scroll;
                Action::None
            }
            KeyCode::PageUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + 10).min(self.max_scroll);
                self.follow = self.scroll >= self.max_scroll;
                Action::None
            }
            // escape interrupts the turn and never leaves the client.
            KeyCode::Esc => Action::Cancel,
            _ => Action::None,
        }
    }

    /// applies a daemon event to the transcript.
    pub fn on_event(&mut self, kind: &str, payload: &Value) {
        match kind {
            "llm.delta" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                // replayed history is not a live rate.
                if !self.replaying {
                    let chars = text.chars().count();
                    if chars > 0 {
                        self.turn_chars += chars;
                        self.samples.push_back((std::time::Instant::now(), chars));
                    }
                }
                self.append_stream(text);
            }
            "tool.requested" => {
                let tool = payload
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let args = summarise(&payload["args"]);
                self.push(Role::Tool, format!("→ {tool} {args}"));
            }
            "tool.completed" => {
                let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
                self.push(Role::Tool, indent(output, 8));
            }
            "tool.failed" => {
                let error = payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                self.push(Role::Error, format!("✗ {error}"));
            }
            "turn.completed" => {
                self.status.turn_running = false;
                self.finish_message();
                self.status.prompt_tokens +=
                    payload["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                self.status.completion_tokens +=
                    payload["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            }
            "turn.interrupted" => {
                self.status.turn_running = false;
                self.finish_message();
                let cause = payload.get("cause").and_then(Value::as_str).unwrap_or("");
                self.push(Role::Error, format!("[interrupted: {cause}]"));
            }
            "turn.started" => {
                if !self.replaying {
                    self.turn_chars = 0;
                    self.turn_started = Some(std::time::Instant::now());
                }
                if self.replaying {
                    if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
                        self.push(Role::User, prompt.to_owned());
                    }
                }
            }
            "compaction.applied" => self.push(Role::System, "[context compacted]"),
            _ => {}
        }
    }

    /// computes the average rate of the message that just finished and starts flashing it.
    fn finish_message(&mut self) {
        let chars = self.turn_chars;
        let elapsed = self
            .turn_started
            .map(|at| std::time::Instant::now().duration_since(at).as_secs_f32())
            .unwrap_or(0.0);
        self.samples.clear();
        self.turn_chars = 0;
        self.turn_started = None;

        if chars == 0 {
            self.flash = None;
            self.tok_per_sec = 0.0;
            return;
        }
        let average = (chars as f32 / CHARS_PER_TOKEN) / elapsed.max(0.05);
        self.flash = Some((average, std::time::Instant::now()));
        self.tok_per_sec = average;
    }

    /// appends streamed assistant text to the current assistant entry, starting one if needed.
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

    /// the transcript as renderable lines.
    ///
    /// every entry carries an explicit role gutter so the operator can tell their own turns
    /// from the agent's and from tool output without relying on colour.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for (index, entry) in self.transcript.iter().enumerate() {
            if index > 0 {
                out.push(Line::from(""));
            }
            let style = style_for(entry.role);
            let gutter = gutter_for(entry.role);
            for (line_index, raw) in entry.text.lines().enumerate() {
                let head = if line_index == 0 { gutter } else { "    │ " };
                out.push(Line::from(vec![
                    Span::styled(head, style),
                    Span::styled(raw.to_owned(), style),
                ]));
            }
        }
        out
    }
}

fn gutter_for(role: Role) -> &'static str {
    match role {
        Role::User => "you │ ",
        Role::Assistant => "sa  │ ",
        Role::Tool => "tool│ ",
        Role::System => "sys │ ",
        Role::Error => "err │ ",
    }
}

fn style_for(role: Role) -> Style {
    match role {
        Role::User => Style::default().fg(Color::Cyan),
        Role::Assistant => Style::default().fg(Color::Green),
        Role::Tool => Style::default().fg(Color::DarkGray),
        Role::System => Style::default().fg(Color::Yellow),
        Role::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
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
        lines.push("…");
    }
    lines.join("\n")
}
