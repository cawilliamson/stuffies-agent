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
    Answer { allow: bool, scope: &'static str },
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub session: String,
    pub model: String,
    pub turn_running: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub struct App {
    pub transcript: Vec<Entry>,
    pub input: String,
    pub scroll: u16,
    pub follow: bool,
    pub status: Status,
    pub pending_approval: Option<String>,
}

impl App {
    #[must_use]
    pub fn new(session: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            scroll: 0,
            follow: true,
            status: Status {
                session: session.into(),
                model: model.into(),
                ..Status::default()
            },
            pending_approval: None,
        }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.transcript.push(Entry {
            role,
            text: text.into(),
        });
        if self.follow {
            self.scroll = self.max_scroll();
        }
    }

    fn max_scroll(&self) -> u16 {
        u16::try_from(self.transcript.len().saturating_sub(1)).unwrap_or(u16::MAX)
    }

    /// handles a key press. returns what the caller should do about it.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => {
                    if let Some(id) = self.pending_approval.take() {
                        self.push(Role::System, "denied");
                        let _ = id;
                        return Action::Answer {
                            allow: false,
                            scope: "once",
                        };
                    }
                    Action::Cancel
                }
                KeyCode::Char('d') => Action::Quit,
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
                if let Some(_id) = self.pending_approval.take() {
                    let (allow, scope) = match text.as_str() {
                        "y" | "yes" => (true, "once"),
                        "t" => (true, "turn"),
                        "s" => (true, "session"),
                        "c" => (true, "class"),
                        _ => (false, "once"),
                    };
                    self.push(Role::System, if allow { "allowed" } else { "denied" });
                    return Action::Answer { allow, scope };
                }
                self.follow = true;
                Action::Submit(text)
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
                self.scroll = (self.scroll + 1).min(self.max_scroll());
                self.follow = self.scroll >= self.max_scroll();
                Action::None
            }
            KeyCode::PageUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + 10).min(self.max_scroll());
                self.follow = self.scroll >= self.max_scroll();
                Action::None
            }
            KeyCode::Esc => Action::Cancel,
            _ => Action::None,
        }
    }

    /// applies a daemon event to the transcript.
    pub fn on_event(&mut self, kind: &str, payload: &Value) {
        match kind {
            "llm.delta" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
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
                self.status.prompt_tokens +=
                    payload["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                self.status.completion_tokens +=
                    payload["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            }
            "turn.interrupted" => {
                self.status.turn_running = false;
                let cause = payload.get("cause").and_then(Value::as_str).unwrap_or("");
                self.push(Role::Error, format!("[interrupted: {cause}]"));
            }
            "turn.started" => {
                if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
                    self.push(Role::User, prompt.to_owned());
                }
            }
            "compaction.applied" => self.push(Role::System, "[context compacted]"),
            _ => {}
        }
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
            self.scroll = self.max_scroll();
        }
    }

    /// the transcript as renderable lines.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for entry in &self.transcript {
            let style = style_for(entry.role);
            for (index, raw) in entry.text.lines().enumerate() {
                let prefix = if index == 0 {
                    match entry.role {
                        Role::User => "› ",
                        Role::Assistant => "  ",
                        Role::Tool => "  ",
                        Role::System => "· ",
                        Role::Error => "! ",
                    }
                } else {
                    "  "
                };
                out.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(raw.to_owned(), style),
                ]));
            }
        }
        out
    }
}

fn style_for(role: Role) -> Style {
    match role {
        Role::User => Style::default().fg(Color::Cyan),
        Role::Assistant => Style::default(),
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
