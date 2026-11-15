//! the tui state machine, independent of any terminal so it can be tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

const RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
const FLASH_DURATION: std::time::Duration = std::time::Duration::from_millis(2400);
const FLASH_PHASE: std::time::Duration = std::time::Duration::from_millis(400);
const CHARS_PER_TOKEN: f32 = 4.0;
/// lines moved per arrow key.
const SCROLL_STEP: u16 = 3;
/// lines moved per page key.
const PAGE_STEP: u16 = 20;

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
    /// for tool entries: which tool ran.
    pub tool: Option<String>,
    /// for tool output: the process exit code, when the tool ran a process.
    pub exit_code: Option<i32>,
}

/// a rendered transcript, reused until its content or width changes. rebuilding it on every
/// scroll keypress is what made scrolling crawl.
pub struct Cache {
    /// matches [`Tab::content_rev`]: the render is reused while these agree.
    pub content_rev: u64,
    pub width: u16,
    pub lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub model: String,
    pub effort: String,
    /// estimated tokens in the model's view, and the window it has.
    pub context_tokens: u64,
    pub context_limit: u64,
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
    /// bumped only when transcript entries change, so scrolling can reuse the render.
    pub content_rev: u64,
    pub cache: Option<Cache>,
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
            title: if title.is_empty() {
                // the tab always carries its session id, so tabs are identifiable.
                format!("({short})")
            } else {
                title
            },
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
            content_rev: 1,
            cache: None,
        }
    }

    pub const fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.transcript.push(Entry {
            role,
            text: text.into(),
            tool: None,
            exit_code: None,
        });
        if self.follow {
            self.scroll = self.max_scroll;
        }
        self.content_rev = self.content_rev.wrapping_add(1);
        self.cache = None;
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

    fn finish_message(&mut self, now: std::time::Instant, reported_tokens: Option<u64>) {
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
        // prefer the provider's own token count over the character estimate.
        let tokens = reported_tokens.map_or(chars as f32 / CHARS_PER_TOKEN, |t| t as f32);
        let average = tokens / active.as_secs_f32().max(0.25);
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
                // while tools run there is no generation: hold the figure rather than
                // decaying to zero mid-turn.
                if self.status.turn_running && self.tok_per_sec > 0.0 {
                    return;
                }
                self.tok_per_sec = self.frozen.unwrap_or(0.0);
            } else {
                let chars: usize = self.samples.iter().map(|(_, n)| n).sum();
                let span = self
                    .samples
                    .front()
                    .map_or(0.0, |(at, _)| now.duration_since(*at).as_secs_f32())
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
                self.transcript.push(Entry {
                    role: Role::Tool,
                    text: args,
                    tool: Some(tool.to_owned()),
                    exit_code: None,
                });
                self.touch();
            }
            "tool.completed" => {
                let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
                let exit_code = payload
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .and_then(|c| i32::try_from(c).ok());
                self.transcript.push(Entry {
                    role: Role::ToolOutput,
                    text: output.to_owned(),
                    tool: None,
                    exit_code,
                });
                self.touch();
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
                let reported = payload["usage"]["completion_tokens"].as_u64();
                self.finish_message(now, reported);
                self.status.prompt_tokens +=
                    payload["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                self.status.completion_tokens +=
                    payload["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                if let Some(context) = payload.get("context") {
                    self.status.context_tokens = context["tokens"]
                        .as_u64()
                        .unwrap_or(self.status.context_tokens);
                    self.status.context_limit = context["limit"]
                        .as_u64()
                        .unwrap_or(self.status.context_limit);
                }
                self.touch();
            }
            "turn.interrupted" => {
                self.status.turn_running = false;
                self.finish_message(now, None);
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
                        tool: None,
                        exit_code: None,
                    });
                }
            }
        }
        if self.follow {
            self.scroll = self.max_scroll;
        }
        self.content_rev = self.content_rev.wrapping_add(1);
        self.cache = None;
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
    /// jump straight to a tab by index.
    SwitchTab(usize),
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub branch: Option<String>,
    pub workspace: Option<String>,
    pub revision: u64,
    /// index into the compiled-in theme registry.
    pub theme: usize,
    /// animation frame for the activity spinner.
    pub spinner: usize,
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
            theme: crate::theme::default_index(),
            spinner: 0,
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

    /// applies a generated session title to the tab that owns it. the id is always kept,
    /// so a tab stays identifiable after it is renamed.
    pub fn apply_rename(&mut self, session: &str, title: &str) {
        let short: String = session.chars().take(8).collect();
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session == session) {
            tab.title = format!("{title} ({short})");
            tab.push(Role::System, format!("title: {title}"));
        }
        self.touch();
    }

    /// replaces the active tab with a fresh one, keeping the strip non-empty.
    pub fn replace_active(&mut self, session: impl Into<String>, model: impl Into<String>) {
        let index = self.active.min(self.tabs.len().saturating_sub(1));
        self.tabs[index] = Tab::new(session, String::new(), model);
        self.touch();
    }

    /// jumps to a tab by zero-based index, ignoring out-of-range presses.
    pub fn switch_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.touch();
        }
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
                KeyCode::Char('c' | 'd') => Action::Quit,
                // plain ctrl+n / ctrl+w, with the shifted forms accepted as aliases.
                KeyCode::Char('t' | 'T') => Action::NewTab,
                KeyCode::Char('w' | 'W') => Action::CloseTab,
                KeyCode::Char(digit @ '1'..='9') => {
                    Action::SwitchTab(digit as usize - '1' as usize)
                }
                _ => {
                    let _ = shift;
                    self.tab_mut().edit(key);
                    Action::None
                }
            };
        }

        match key.code {
            KeyCode::BackTab => Action::NextTab,
            KeyCode::Enter | KeyCode::Char('\n' | '\r') => self.tab_mut().submit(),
            KeyCode::Up => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(SCROLL_STEP);
                tab.touch();
                Action::None
            }
            KeyCode::Down => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + SCROLL_STEP).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                tab.touch();
                Action::None
            }
            KeyCode::PageUp => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(PAGE_STEP);
                tab.touch();
                Action::None
            }
            KeyCode::PageDown => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + PAGE_STEP).min(tab.max_scroll);
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
        Role::Thinking => (DIM, Color::Rgb(0x8a, 0x93, 0xa0)),
        Role::Tool => (Color::Rgb(0xe0, 0xb0, 0x60), Color::Rgb(0xd8, 0xc8, 0xa8)),
        Role::ToolOutput => (FAINT, DIM),
        Role::System => (DIM, DIM),
        Role::Error => (BAD, Color::Rgb(0xf0, 0xc0, 0xc0)),
    };
    (label, edge, body)
}

/// the colour inline code is drawn in.
pub const CODE_INK: Color = Color::Rgb(0xe0, 0xc0, 0x80);

/// drops the inline markdown markers so text can be measured.
#[must_use]
pub fn plain(text: &str) -> String {
    text.replace("**", "").replace(['*', '`'], "")
}

/// splits `**bold**`, `*italic*` and `` `code` `` runs into styled spans, dropping the
/// markers. unmatched markers simply style the remainder.
fn styled(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut rest = text;
    let mut bold = false;
    let mut italic = false;
    let mut code = false;

    let style_of = |bold: bool, italic: bool, code: bool| {
        let mut style = base;
        if bold {
            style = style.add_modifier(ratatui::style::Modifier::BOLD);
        }
        if italic {
            style = style.add_modifier(ratatui::style::Modifier::ITALIC);
        }
        if code {
            style = style.fg(CODE_INK);
        }
        style
    };

    while let Some(ch) = rest.chars().next() {
        // longest marker first, so `**` is never read as two italics.
        if let Some(after) = rest.strip_prefix("**") {
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    style_of(bold, italic, code),
                ));
            }
            bold = !bold;
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix('*') {
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    style_of(bold, italic, code),
                ));
            }
            italic = !italic;
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix('`') {
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    style_of(bold, italic, code),
                ));
            }
            code = !code;
            rest = after;
            continue;
        }
        buf.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style_of(bold, italic, code)));
    }
    spans
}

/// cuts styled spans to a visible width.
fn fit_spans(spans: Vec<Span<'static>>, room: usize) -> (Vec<Span<'static>>, usize) {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        if used >= room {
            break;
        }
        let mut text = String::new();
        for ch in span.content.chars() {
            if used >= room {
                break;
            }
            text.push(ch);
            used += 1;
        }
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
        }
    }
    (out, used)
}

/// the colour a patch or diff line should carry.
fn diff_ink(line: &str, base: Color) -> Color {
    match line.chars().next() {
        Some('+') => Color::Rgb(0x7f, 0xc7, 0x9a),
        Some('-') => Color::Rgb(0xe8, 0x7c, 0x7c),
        Some('@') => Color::Rgb(0x8a, 0xc8, 0xff),
        _ => base,
    }
}

/// pushes a body section whose text may carry ANSI colouring, as command output does.
fn body_ansi(out: &mut Vec<Line<'static>>, text: &str, width: usize, edge: Color, ink: Color) {
    use ansi_to_tui::IntoText;

    let parsed = text.as_bytes().into_text().unwrap_or_else(|_| {
        ratatui::text::Text::from(
            text.lines()
                .map(|l| Line::from(l.to_owned()))
                .collect::<Vec<_>>(),
        )
    });

    let room = width.saturating_sub(4);
    for line in parsed.lines {
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let coloured = line.spans.iter().any(|s| s.style.fg.is_some());
        let mut spans = line.spans;
        if !coloured {
            let tint = diff_ink(&plain, ink);
            spans = spans
                .into_iter()
                .map(|s| Span::styled(s.content, Style::default().fg(tint)))
                .collect();
        }
        let (spans, used) = fit_spans(spans, room);
        let pad = room.saturating_sub(used);

        let mut row = vec![Span::styled("\u{2502} ", Style::default().fg(edge))];
        row.extend(spans);
        row.push(Span::styled(
            format!("{} \u{2502}", " ".repeat(pad)),
            Style::default().fg(edge),
        ));
        out.push(Line::from(row));
    }
}

/// pushes one body section: each row is exactly `width` cells.
fn body(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    edge: Color,
    ink: Color,
    markdown: bool,
) {
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
        // markers are not drawn, so measure the visible text.
        let shown = if markdown {
            plain(&line).chars().count()
        } else {
            line.chars().count()
        };
        let pad = room.saturating_sub(shown);

        let base = Style::default().fg(ink);
        let mut spans = vec![Span::styled("\u{2502} ", Style::default().fg(edge))];
        if markdown {
            spans.extend(styled(&line, base));
        } else {
            spans.push(Span::styled(line, base));
        }
        spans.push(Span::styled(
            format!("{} \u{2502}", " ".repeat(pad)),
            Style::default().fg(edge),
        ));
        out.push(Line::from(spans));
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

/// the frame title for an entry: the tool's own name, or `output (exit N)`.
fn entry_label(entry: &Entry, model: &str) -> String {
    match entry.role {
        Role::Tool => entry.tool.clone().unwrap_or_else(|| "tool".to_owned()),
        Role::ToolOutput => entry.exit_code.map_or_else(
            || "output".to_owned(),
            |code| format!("output (exit {code})"),
        ),
        other => role_style(other, model).0,
    }
}

/// the body text for an entry, with a shell prompt in front of a command.
fn entry_body(entry: &Entry) -> String {
    if entry.role == Role::Tool && entry.tool.as_deref() == Some("bash") {
        format!("$ {}", entry.text)
    } else {
        entry.text.clone()
    }
}

/// renders one transcript entry as a titled frame.
fn framed(entry: &Entry, width: usize, model: &str) -> Vec<Line<'static>> {
    let (_, edge, ink) = role_style(entry.role, model);
    let mut out = Vec::new();
    top(&mut out, &entry_label(entry, model), width, edge);
    let markdown = !matches!(entry.role, Role::Tool | Role::ToolOutput);
    body(&mut out, &entry_body(entry), width, edge, ink, markdown);
    bottom(&mut out, width, edge);
    out
}

/// renders a tool call and its output as one frame split by a rule, rather than two boxes.
fn framed_tool(
    tool: &str,
    call: &str,
    output: &str,
    exit_code: Option<i32>,
    width: usize,
) -> Vec<Line<'static>> {
    let (_, edge, ink) = role_style(Role::Tool, "");
    let (_, _, out_ink) = role_style(Role::ToolOutput, "");
    let mut out = Vec::new();
    // the exit code belongs in the title, so the rule stays clean.
    let title = exit_code.map_or_else(|| tool.to_owned(), |code| format!("{tool} (exit {code})"));
    top(&mut out, &title, width, edge);
    let invocation = if tool == "bash" {
        format!("$ {call}")
    } else {
        call.to_owned()
    };
    // a command and its output are literal: no markdown, but ANSI colouring is honoured.
    body(&mut out, &invocation, width, edge, ink, false);
    divider(&mut out, width, edge);
    body_ansi(&mut out, output, width, edge, out_ink);
    bottom(&mut out, width, edge);
    out
}

/// renders a turn's reasoning and its reply as one frame split by a rule, rather than two
/// separate boxes.
fn framed_split(model: &str, thinking: &str, message: &str, width: usize) -> Vec<Line<'static>> {
    let (label, edge, _) = role_style(Role::Assistant, model);
    let (_, _, think_ink) = role_style(Role::Thinking, model);
    let mut out = Vec::new();
    top(&mut out, &label, width, edge);
    body(&mut out, thinking, width, edge, think_ink, true);
    // the rule is part of this frame, so it uses the frame's colour.
    divider(&mut out, width, edge);
    body(
        &mut out,
        message,
        width,
        edge,
        Color::Rgb(0xd8, 0xe8, 0xdc),
        true,
    );
    bottom(&mut out, width, edge);
    out
}

/// the transcript of a tab as renderable lines.
///
/// reasoning and the reply it produced share one frame: the reasoning sits above a rule and
/// the reply below it.
#[must_use]
pub fn transcript_lines(tab: &Tab, width: u16) -> Vec<Line<'static>> {
    // fill whatever width the pane has: no cap, or wide terminals leave a dead margin.
    let inner = usize::from(width).max(16);
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
                out.extend(framed(
                    &Entry {
                        role: Role::Thinking,
                        text: thinking,
                        tool: None,
                        exit_code: None,
                    },
                    inner,
                    &tab.status.model,
                ));
                index = next;
            }
            continue;
        }

        if entry.role == Role::Tool {
            let tool = entry.tool.clone().unwrap_or_else(|| "tool".to_owned());
            if index + 1 < tab.transcript.len()
                && tab.transcript[index + 1].role == Role::ToolOutput
            {
                let result = &tab.transcript[index + 1];
                out.extend(framed_tool(
                    &tool,
                    &entry.text,
                    &result.text,
                    result.exit_code,
                    inner,
                ));
                index += 2;
                continue;
            }
        }

        out.extend(framed(entry, inner, &tab.status.model));
        index += 1;
    }

    out
}
