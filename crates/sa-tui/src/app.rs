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

/// how many editor states the undo stack keeps.
const UNDO_LIMIT: usize = 128;

/// a run of typing inside this window undoes as one step, rather than per character.
const UNDO_COALESCE: std::time::Duration = std::time::Duration::from_millis(500);

/// how long the scrollbar stays up after the last scroll. it is a hint, not furniture.
const SCROLLBAR_HOLD: std::time::Duration = std::time::Duration::from_millis(1200);
/// lines moved per page key.
const PAGE_STEP: u16 = 20;

pub const INK: Color = Color::Rgb(0xc8, 0xd0, 0xdc);
pub const DIM: Color = Color::Rgb(0x6c, 0x76, 0x86);
pub const FAINT: Color = Color::Rgb(0x44, 0x4c, 0x58);
pub const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);
pub const OK: Color = Color::Rgb(0x7f, 0xc7, 0x9a);
pub const BAD: Color = Color::Rgb(0xe8, 0x7c, 0x7c);

/// shown in the transcript while a turn runs. one is picked per turn, so it reads as the
/// model getting on with it rather than a static label.
pub const WORKING_WORDS: &[&str] = &[
    "adjudicating",
    "appraising",
    "arbitrating",
    "assessing",
    "brainstorming",
    "brewing",
    "brooding",
    "calculating",
    "calibrating",
    "cerebrating",
    "chiselling",
    "chugging",
    "churning",
    "cogitating",
    "concocting",
    "conjuring",
    "considering",
    "contemplating",
    "contriving",
    "cross-referencing",
    "crunching",
    "deciphering",
    "deducing",
    "deliberating",
    "dissecting",
    "distilling",
    "divining",
    "dredging",
    "envisioning",
    "evaluating",
    "excavating",
    "extrapolating",
    "fermenting",
    "ferreting",
    "fettling",
    "fiddling",
    "foraging",
    "fossicking",
    "gauging",
    "gestating",
    "grinding",
    "hammering",
    "imagining",
    "improvising",
    "incubating",
    "introspecting",
    "macgyvering",
    "manifesting",
    "mapping",
    "marinating",
    "measuring",
    "meditating",
    "mulling",
    "musing",
    "noodling",
    "percolating",
    "philosophising",
    "picturing",
    "plotting",
    "pondering",
    "pontificating",
    "probing",
    "prophesying",
    "puzzling",
    "ratiocinating",
    "reflecting",
    "riffing",
    "ruminating",
    "scavenging",
    "scheming",
    "scrutinising",
    "scrying",
    "sculpting",
    "shaping",
    "sifting",
    "simmering",
    "sleuthing",
    "sniffing",
    "soliloquising",
    "speculating",
    "spelunking",
    "stewing",
    "summoning",
    "synthesising",
    "theorising",
    "tinkering",
    "tracing",
    "tracking",
    "triangulating",
    "trundling",
    "tuning",
    "unravelling",
    "weighing",
    "whirring",
    "whittling",
    "winnowing",
    "wondering",
    "wrangling",
    "wrestling",
];

/// picks this turn's word. seeded from the clock: no dependency, and the exact choice is
/// not worth being reproducible.
#[must_use]
pub fn working_word() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() as usize);
    WORKING_WORDS
        .get(nanos % WORKING_WORDS.len())
        .copied()
        .unwrap_or("working")
        .to_owned()
}

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
    /// pictures sent with the prompt, drawn inside the message frame.
    pub images: Vec<Thumb>,
}

impl Entry {
    fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool: None,
            exit_code: None,
            images: Vec::new(),
        }
    }
}

/// what an endpoint reports about one model it serves.
#[derive(Debug, Clone, Default)]
pub struct ServedModel {
    pub context_limit: Option<u64>,
    pub effort_levels: Vec<String>,
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
    /// the directory this tab works in. tools and context follow it.
    pub cwd: String,
    /// the git branch of `cwd`, for the status bar. absent outside a repository.
    pub branch: Option<String>,
    pub transcript: Vec<Entry>,
    input: tui_input::Input,
    pub status: Status,
    /// the word in the in-progress header. cleared when the turn ends, which is what
    /// makes the header disappear.
    pub working: Option<String>,
    /// images queued for the next prompt.
    pub attachments: Vec<Attachment>,
    /// editor states for undo, newest last.
    undo: Vec<(String, usize)>,
    last_edit: Option<std::time::Instant>,
    pub scroll: u16,
    pub max_scroll: u16,
    /// when the operator last scrolled. drives the scrollbar's appearance.
    pub scroll_touched: Option<std::time::Instant>,
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
            cwd: String::new(),
            branch: None,
            transcript: Vec::new(),
            input: tui_input::Input::default(),
            status: Status {
                model: model.into(),
                ..Status::default()
            },
            working: None,
            attachments: Vec::new(),
            undo: Vec::new(),
            last_edit: None,
            scroll_touched: None,
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
        self.transcript.push(Entry::new(role, text));
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

    /// whether closing this tab would throw away work: a conversation, tool output, or a
    /// prompt typed but not sent. notices the app wrote itself do not count.
    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.input.value().trim().is_empty()
            || self
                .transcript
                .iter()
                .any(|entry| entry.role != Role::System)
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

    /// applies an editor command directly, for the keys whose meaning we override.
    fn edit_request(&mut self, request: tui_input::InputRequest) -> Action {
        self.snapshot(false);
        self.input.handle(request);
        self.touch();
        Action::None
    }

    /// records the current editor state, coalescing a run of plain typing so undo steps
    /// back a phrase rather than a character.
    fn snapshot(&mut self, coalesce: bool) {
        let now = std::time::Instant::now();
        let recent = self
            .last_edit
            .is_some_and(|at| now.duration_since(at) < UNDO_COALESCE);
        if !(coalesce && recent && !self.undo.is_empty()) {
            self.undo
                .push((self.input.value().to_owned(), self.input.cursor()));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.last_edit = Some(now);
    }

    /// steps back one editor state. returns false when there is nothing left to undo.
    fn undo(&mut self) -> bool {
        let current = (self.input.value().to_owned(), self.input.cursor());
        while let Some((value, cursor)) = self.undo.pop() {
            // movement keys record states that are identical; skip rather than no-op.
            if (value.as_str(), cursor) == (current.0.as_str(), current.1) {
                continue;
            }
            self.input = std::mem::take(&mut self.input)
                .with_value(value)
                .with_cursor(cursor);
            self.last_edit = None;
            self.touch();
            return true;
        }
        false
    }

    fn edit(&mut self, key: KeyEvent) {
        use tui_input::backend::crossterm::EventHandler;
        let plain = matches!(key.code, KeyCode::Char(_)) && key.modifiers.is_empty();
        self.snapshot(plain);
        self.input.handle_event(&crossterm::event::Event::Key(key));
        self.touch();
    }

    /// completes the path token at the cursor. returns true when the text changed, so
    /// tab keeps its other meanings when there is nothing to complete.
    fn complete_path(&mut self) -> bool {
        let text = self.input.value().to_owned();
        let cursor = self.input.cursor();
        let byte = text
            .char_indices()
            .nth(cursor)
            .map_or(text.len(), |(index, _)| index);
        let before = &text[..byte];
        let start = before
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map_or(0, |(index, character)| index + character.len_utf8());
        let token = &text[start..byte];
        let Some(completed) = complete_token(token, false) else {
            return false;
        };
        let replacement = completed;
        let new_text = format!("{}{}{}", &text[..start], replacement, &text[byte..]);
        let new_cursor = text[..start].chars().count() + replacement.chars().count();
        self.input = std::mem::take(&mut self.input)
            .with_value(new_text)
            .with_cursor(new_cursor);
        self.touch();
        true
    }

    /// completes the directory named in `/cwd`. returns true when the text changed.
    fn complete_cwd(&mut self) -> bool {
        let text = self.input.value().to_owned();
        let Some(arg) = cwd_arg(&text) else {
            return false;
        };
        let Some(completed) = complete_dir_token(arg) else {
            return false;
        };
        let new_text = format!("/cwd {completed}");
        let new_cursor = new_text.chars().count();
        self.input = std::mem::take(&mut self.input)
            .with_value(new_text)
            .with_cursor(new_cursor);
        self.touch();
        true
    }

    /// the completion offered for `/cwd`, drawn dimmed after the cursor. only at the end of
    /// the line, where the operator is actually typing.
    #[must_use]
    pub fn suggestion(&self) -> Option<String> {
        let text = self.input.value();
        if self.input.cursor() != text.chars().count() {
            return None;
        }
        let arg = cwd_arg(text)?;
        let completed = complete_dir_token(arg)?;
        let rest = completed.strip_prefix(arg)?.to_owned();
        (!rest.is_empty()).then_some(rest)
    }

    /// whether the composer holds the `/cwd` command, whose argument is always a directory.
    #[must_use]
    pub fn is_cwd_command(&self) -> bool {
        cwd_arg(self.input.value()).is_some()
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

    /// records a scroll. the bar appears on the first one and the tick clears it.
    fn scrolled(&mut self) {
        self.scroll_touched = Some(std::time::Instant::now());
        self.touch();
    }

    pub fn tick_at(&mut self, now: std::time::Instant) {
        // the bar is transient: drop it once the operator has stopped scrolling. this runs
        // before the early return below, or a busy tab would keep the bar up forever.
        if let Some(at) = self.scroll_touched {
            if now.duration_since(at) >= SCROLLBAR_HOLD {
                self.scroll_touched = None;
                self.touch();
            }
        }

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

    /// takes the tab's directory, model, effort and title from the daemon's session row.
    /// every tab is filled from its row, so none can stand in the strip without a directory.
    pub fn apply_session_row(&mut self, row: &Value) {
        let cwd = row["cwd"].as_str().unwrap_or_default();
        if cwd != self.cwd {
            self.cwd = cwd.to_owned();
            self.branch = git_branch(&self.cwd);
        }
        self.status.effort = row["effort"].as_str().unwrap_or_default().to_owned();
        if let Some(model) = row["model"].as_str().filter(|model| !model.is_empty()) {
            self.status.model = model.to_owned();
        }
        if let Some(title) = row["title"].as_str() {
            self.title = tab_label(&self.session, title);
        }
        self.touch();
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
                let mut entry = Entry::new(Role::Tool, args);
                entry.tool = Some(tool.to_owned());
                self.transcript.push(entry);
                self.touch();
            }
            "tool.completed" => {
                let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
                let exit_code = payload
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .and_then(|c| i32::try_from(c).ok());
                let mut entry = Entry::new(Role::ToolOutput, output);
                entry.exit_code = exit_code;
                self.transcript.push(entry);
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
                self.working = None;
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
                    // a null limit means the endpoint never said: clear it rather than
                    // keeping a figure from another model.
                    self.status.context_limit = context["limit"].as_u64().unwrap_or(0);
                }
                self.touch();
            }
            "turn.steered" => {
                if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
                    self.push(Role::User, prompt.to_owned());
                }
            }
            "turn.interrupted" => {
                self.status.turn_running = false;
                self.working = None;
                self.finish_message(now, None);
                let cause = payload.get("cause").and_then(Value::as_str).unwrap_or("");
                self.push(Role::Error, format!("interrupted: {cause}"));
            }
            "turn.started" => {
                let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or("");
                let images = thumbnails(&payload["images"]);
                if self.transcript.last().map(|e| e.role) == Some(Role::User) {
                    // a steer or a resubmit already put the prompt up; the pictures belong
                    // on that same message rather than a second one.
                    if !images.is_empty() {
                        if let Some(entry) = self.transcript.last_mut() {
                            entry.images = images;
                        }
                        self.content_rev = self.content_rev.wrapping_add(1);
                        self.cache = None;
                    }
                } else if !prompt.is_empty() || !images.is_empty() {
                    self.push(Role::User, prompt.to_owned());
                    if let Some(entry) = self.transcript.last_mut() {
                        entry.images = images;
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
            "session.cwd" => {
                if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                    self.cwd = cwd.to_owned();
                    self.touch();
                }
            }
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
                    self.transcript.push(Entry::new(role, text));
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

/// an image attached to the next prompt. `protocol` is the terminal's own encoding, so
/// the preview is drawn without decoding twice.
pub struct Attachment {
    /// the path as typed, so the list can follow the composer.
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub data: String,
    pub protocol: Option<ratatui_image::protocol::StatefulProtocol>,
}

/// a picture already in the transcript, rendered as half-block cells so it scrolls with
/// the conversation and needs no terminal image protocol.
#[derive(Debug, Clone)]
pub struct Thumb {
    pub name: String,
    pub lines: Vec<Line<'static>>,
}

/// the widest and tallest a transcript picture may be, in cells.
const THUMB_COLS: u32 = 40;
const THUMB_ROWS: u32 = 12;

/// pictures from a `turn.started` payload. undecodable entries are simply left out.
fn thumbnails(images: &Value) -> Vec<Thumb> {
    let Some(items) = images.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let data = item.get("data").and_then(Value::as_str)?;
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok()?;
            let lines = thumb_lines(&bytes)?;
            Some(Thumb {
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("image")
                    .to_owned(),
                lines,
            })
        })
        .collect()
}

/// renders a picture as coloured half-block cells. one cell is one pixel across and two
/// down, which keeps the aspect ratio square.
fn thumb_lines(bytes: &[u8]) -> Option<Vec<Line<'static>>> {
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return None;
    }
    let mut cols = THUMB_COLS.min(width);
    let mut rows = (cols * height / width).max(2);
    let max_rows = THUMB_ROWS * 2;
    if rows > max_rows {
        rows = max_rows;
        cols = (rows * width / height).max(1);
    }
    // whole cell rows only: each row paints two pixel rows.
    rows -= rows % 2;

    let scaled = image
        .resize_exact(cols, rows, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let mut lines = Vec::new();
    for row in 0..rows / 2 {
        let spans: Vec<Span<'static>> = (0..cols)
            .map(|col| {
                let top = scaled.get_pixel(col, row * 2).0;
                let bottom = scaled.get_pixel(col, row * 2 + 1).0;
                Span::styled(
                    "\u{2580}",
                    Style::default().fg(blend(top)).bg(blend(bottom)),
                )
            })
            .collect();
        lines.push(Line::from(spans));
    }
    Some(lines)
}

/// composites a pixel over the transcript background, so transparency does not show as
/// black holes in the middle of a picture.
fn blend(pixel: [u8; 4]) -> Color {
    const BG: [u16; 3] = [0x10, 0x14, 0x1a];
    let alpha = u16::from(pixel[3]);
    let mix = |channel: u8, under: u16| {
        u8::try_from((u16::from(channel) * alpha + under * (255 - alpha)) / 255).unwrap_or(0)
    };
    Color::Rgb(
        mix(pixel[0], BG[0]),
        mix(pixel[1], BG[1]),
        mix(pixel[2], BG[2]),
    )
}

/// the tab label for a session: the generated title, always keeping the id visible so a
/// tab stays identifiable after it is renamed.
#[must_use]
pub fn tab_label(session: &str, title: &str) -> String {
    let short: String = session.chars().take(8).collect();
    format!("{title} ({short})")
}

/// the git branch of a directory, for the status bar. absent outside a repository.
#[must_use]
pub fn git_branch(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch)
    }
}

/// one selectable row in a picker.
#[derive(Debug, Clone)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
}

/// a list the operator chooses from, drawn over the interface.
#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
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
    /// the operator chose a session to resume.
    ResumePicked(String),
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub revision: u64,
    /// index into the compiled-in theme registry.
    pub theme: usize,
    /// animation frame for the activity spinner.
    pub spinner: usize,
    /// the open picker, if any. keys route to it while it is up.
    pub picker: Option<Picker>,
    /// the question awaiting a yes/no answer, if any. keys route to it while it is up.
    pub confirm: Option<String>,
    /// the terminal's image protocol, resolved once at startup. none means the terminal
    /// cannot show pictures, so attachments still send but do not preview.
    pub image_picker: Option<ratatui_image::picker::Picker>,
    /// what the endpoint says about each model it serves. a model absent here has no
    /// known window or effort list, and is not given guessed ones.
    pub served: std::collections::HashMap<String, ServedModel>,
}

impl App {
    #[must_use]
    pub fn new(session: impl Into<String>, model: impl Into<String>) -> Self {
        let tab = Tab::new(session, String::new(), model);
        Self {
            tabs: vec![tab],
            active: 0,
            revision: 1,
            theme: crate::theme::default_index(),
            spinner: 0,
            picker: None,
            confirm: None,
            image_picker: None,
            served: std::collections::HashMap::new(),
        }
    }

    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        let active = self.active.min(self.tabs.len().saturating_sub(1));
        if let Some(tab) = self.tabs.get_mut(active) {
            tab.touch();
        }
    }

    /// the context window for the active tab's model. zero means unknown.
    #[must_use]
    pub fn context_limit(&self, tab: &Tab) -> u64 {
        self.served
            .get(&tab.status.model)
            .and_then(|model| model.context_limit)
            .unwrap_or(tab.status.context_limit)
    }

    /// the reasoning effort values the active tab's model accepts, as the endpoint lists
    /// them. empty means it advertises none.
    #[must_use]
    pub fn effort_levels(&self, tab: &Tab) -> Vec<String> {
        self.served
            .get(&tab.status.model)
            .map(|model| model.effort_levels.clone())
            .unwrap_or_default()
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
        let label = tab_label(session, title);
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session == session) {
            tab.title = label;
            tab.push(Role::System, format!("title: {title}"));
        }
        self.touch();
    }

    /// drops every tab whose session is no longer in `keep`, leaving the same tab active
    /// where it survives. returns whether the active tab survived.
    pub fn retain_tabs(&mut self, keep: &[&str]) -> bool {
        let active = self.tab().session.clone();
        self.tabs.retain(|tab| keep.contains(&tab.session.as_str()));
        let survived = self.tabs.iter().any(|tab| tab.session == active);
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.session == active)
            .unwrap_or(0);
        self.touch();
        survived
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

    /// opens a picker over the interface, replacing any that is already up.
    pub fn open_picker(&mut self, title: impl Into<String>, items: Vec<PickerItem>) {
        self.picker = Some(Picker {
            title: title.into(),
            items,
            selected: 0,
        });
        self.touch();
    }

    /// keeps attachments in step with the image paths in the composer, so the operator
    /// just types a filename rather than running a command.
    fn refresh_attachments(&mut self) {
        let wanted = attachment_paths(self.tab().input_text());
        let Self {
            tabs,
            active,
            image_picker,
            ..
        } = self;
        let index = (*active).min(tabs.len().saturating_sub(1));
        let Some(tab) = tabs.get_mut(index) else {
            return;
        };
        tab.attachments.retain(|held| wanted.contains(&held.path));
        for path in wanted {
            if tab.attachments.iter().any(|held| held.path == path) {
                continue;
            }
            if let Some(attachment) = load_attachment(&path, image_picker.as_ref()) {
                tab.attachments.push(attachment);
            }
        }
    }

    /// keys belong to the confirm while it is up, so nothing else can happen under it.
    fn on_confirm_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                self.confirm = None;
                self.touch();
                Action::CloseTab
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.confirm = None;
                self.touch();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// closing a tab that holds work asks first; an untouched tab just goes.
    fn request_close(&mut self) -> Action {
        if !self.tab().has_content() {
            return Action::CloseTab;
        }
        self.confirm = Some(format!("close {}?", self.tab().title));
        self.touch();
        Action::None
    }

    /// keys belong to the picker while it is open, so the composer never sees them.
    fn on_picker_key(&mut self, key: KeyEvent) -> Action {
        let count = self.picker.as_ref().map_or(0, |p| p.items.len());
        match key.code {
            KeyCode::Up => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                self.touch();
                Action::None
            }
            KeyCode::Down => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
                }
                self.touch();
                Action::None
            }
            KeyCode::Enter => {
                let picked = self
                    .picker
                    .as_ref()
                    .and_then(|p| p.items.get(p.selected))
                    .map(|item| item.id.clone());
                self.picker = None;
                self.touch();
                picked.map_or(Action::None, Action::ResumePicked)
            }
            KeyCode::Esc => {
                self.picker = None;
                self.touch();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if self.confirm.is_some() {
            return self.on_confirm_key(key);
        }
        if self.picker.is_some() {
            return self.on_picker_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl {
            return match key.code {
                KeyCode::Char('c' | 'd') => Action::Quit,
                // plain ctrl+n / ctrl+w, with the shifted forms accepted as aliases.
                KeyCode::Char('t' | 'T') => Action::NewTab,
                KeyCode::Char('-' | 'z') => {
                    self.tab_mut().undo();
                    Action::None
                }
                // pi kills to the line start here; the library's default kills the line.
                KeyCode::Char('u') => self
                    .tab_mut()
                    .edit_request(tui_input::InputRequest::DeleteFromStart),
                KeyCode::Char('w' | 'W') => self.request_close(),
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
            KeyCode::Tab => {
                // `/cwd` names a directory, so it completes to directories only rather than
                // falling back to file completion.
                let tab = self.tab_mut();
                if tab.is_cwd_command() {
                    tab.complete_cwd();
                } else if tab.complete_path() {
                    self.refresh_attachments();
                }
                Action::None
            }
            KeyCode::BackTab => Action::NextTab,
            KeyCode::Enter | KeyCode::Char('\n' | '\r') => self.tab_mut().submit(),
            KeyCode::Up => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(SCROLL_STEP);
                tab.scrolled();
                Action::None
            }
            KeyCode::Down => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + SCROLL_STEP).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                tab.scrolled();
                Action::None
            }
            KeyCode::PageUp => {
                let tab = self.tab_mut();
                tab.follow = false;
                tab.scroll = tab.scroll.saturating_sub(PAGE_STEP);
                tab.scrolled();
                Action::None
            }
            KeyCode::PageDown => {
                let tab = self.tab_mut();
                tab.scroll = (tab.scroll + PAGE_STEP).min(tab.max_scroll);
                tab.follow = tab.scroll >= tab.max_scroll;
                tab.scrolled();
                Action::None
            }
            KeyCode::Esc => Action::Cancel,
            _ => {
                self.tab_mut().edit(key);
                self.refresh_attachments();
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

/// pushes a picture section: the file name, then the pixels, both inside the frame.
fn body_image(out: &mut Vec<Line<'static>>, thumb: &Thumb, width: usize, edge: Color) {
    let room = width.saturating_sub(4);
    let caption: String = thumb.name.chars().take(room).collect();
    let pad = room.saturating_sub(caption.chars().count());
    out.push(Line::from(vec![
        Span::styled("\u{2502} ", Style::default().fg(edge)),
        Span::styled(caption, Style::default().fg(DIM)),
        Span::styled(
            format!("{} \u{2502}", " ".repeat(pad)),
            Style::default().fg(edge),
        ),
    ]));
    for line in &thumb.lines {
        let (spans, used) = fit_spans(line.spans.clone(), room);
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
    for thumb in &entry.images {
        body_image(&mut out, thumb, width, edge);
    }
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
                    &Entry::new(Role::Thinking, thinking),
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

/// image paths mentioned in a prompt. quoted, punctuated and home-relative forms all
/// appear in real use, so the token is cleaned before it is tested.
fn attachment_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| {
            token.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | ';' | '(' | ')'))
        })
        .filter(|token| {
            let lower = token.to_ascii_lowercase();
            [".png", ".jpg", ".jpeg", ".gif", ".webp"]
                .iter()
                .any(|ext| lower.ends_with(ext))
        })
        .map(expand_home)
        .filter(|path| std::path::Path::new(path).is_file())
        .collect()
}

/// expands a leading `~/` so a typed path can be opened.
#[must_use]
pub fn expand_home(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home).join(rest).display().to_string();
        }
    }
    token.to_owned()
}

fn media_type_for(path: &str) -> &'static str {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("gif") => "image/gif",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

fn load_attachment(
    path: &str,
    picker: Option<&ratatui_image::picker::Picker>,
) -> Option<Attachment> {
    let bytes = std::fs::read(path).ok()?;
    let protocol = picker.and_then(|picker| {
        image::load_from_memory(&bytes)
            .ok()
            .map(|decoded| picker.new_resize_protocol(decoded))
    });
    Some(Attachment {
        path: path.to_owned(),
        name: std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_owned(),
        media_type: media_type_for(path).to_owned(),
        data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes),
        protocol,
    })
}

/// the argument of `/cwd`, when the composer holds that command.
fn cwd_arg(text: &str) -> Option<&str> {
    text.strip_prefix("/cwd ")
}

/// completes `token` against the filesystem, keeping the form the operator typed so
/// `~/Dow` becomes `~/Downloads/` rather than an absolute path. `dirs_only` is for the
/// arguments that can only name a directory.
fn complete_token(token: &str, dirs_only: bool) -> Option<String> {
    let (shown_dir, prefix) = token
        .rfind('/')
        .map_or(("", token), |index| (&token[..=index], &token[index + 1..]));
    let lookup = if shown_dir.is_empty() {
        std::path::PathBuf::from(".")
    } else {
        std::path::PathBuf::from(expand_home(shown_dir))
    };

    let mut matches: Vec<String> = std::fs::read_dir(&lookup)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) {
                return None;
            }
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if dirs_only && !is_dir {
                return None;
            }
            Some(if is_dir { format!("{name}/") } else { name })
        })
        .collect();
    matches.sort();

    match matches.len() {
        0 => None,
        1 => Some(format!("{shown_dir}{}", matches[0])),
        _ => {
            // more than one: extend to the longest shared prefix, which is what the
            // operator expects from a second tab press.
            let shared = longest_common_prefix(&matches);
            (shared.chars().count() > prefix.chars().count())
                .then(|| format!("{shown_dir}{shared}"))
        }
    }
}

/// the completion for a directory argument, which never offers a file.
fn complete_dir_token(token: &str) -> Option<String> {
    complete_token(token, true)
}

fn longest_common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut end = first.len();
    for value in values.iter().skip(1) {
        let shared = first
            .char_indices()
            .zip(value.chars())
            .take_while(|((_, a), b)| a == b)
            .map(|((index, character), _)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        end = end.min(shared);
    }
    first[..end].to_owned()
}
