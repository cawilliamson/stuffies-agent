//! terminal front end: session tabs, a framed transcript pane, prompt and status bar.
//!
//! one visual, no theming layer. the renderer is dirty-gated: a frame is only built when
//! something visible changed, so an idle session costs nothing.

pub mod app;
pub mod ui;

use std::io::Write;

use app::{Action, App};
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::text::Line;
use ratatui::Terminal;
use sa_client::Client;
use serde_json::{json, Value};

/// the fastest a frame may be rebuilt. streamed deltas invalidate the render on every
/// token, so an unthrottled loop spends all its time rebuilding the transcript and the UI
/// stops answering keys.
const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// frame pacing: only rebuild when the view changed, and never faster than the interval.
struct Frame {
    drawn: u64,
    last: std::time::Instant,
    /// set by input, which must never be starved by the throttle.
    forced: bool,
}

impl Frame {
    fn new() -> Self {
        Self {
            drawn: 0,
            // the first frame is always due: otherwise the screen stays blank for one
            // interval before anything appears.
            last: std::time::Instant::now()
                .checked_sub(FRAME_INTERVAL)
                .unwrap_or_else(std::time::Instant::now),
            forced: false,
        }
    }

    /// makes the next render unconditional, so input is never starved by the throttle.
    const fn force(&mut self) {
        self.forced = true;
    }

    /// whether a frame is due: the view changed and the interval has elapsed. taking the
    /// clock as an argument keeps the rule testable without waiting on real time.
    fn due(&self, now: std::time::Instant, revision: u64) -> bool {
        self.forced || (revision != self.drawn && now.duration_since(self.last) >= FRAME_INTERVAL)
    }

    fn render<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        app: &mut App,
    ) -> std::io::Result<()> {
        let now = std::time::Instant::now();
        let revision = app.revision();
        if !self.due(now, revision) {
            return Ok(());
        }
        self.drawn = revision;
        self.last = now;
        self.forced = false;
        draw(terminal, app)
    }
}

struct Restore {
    /// whether the keyboard enhancement protocol was turned on, so it is turned back off.
    enhanced: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PopKeyboardEnhancementFlags
            );
        }
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

/// draws the tab strip: one row, no background bands. the active tab is a lit chip; the
/// rest are plain text separated by dim dividers.
pub fn draw<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    terminal
        .draw(|frame| {
            if let Some((x, y)) = ui::render(frame, app) {
                frame.set_cursor_position((x, y));
            }
            ui::attachment_preview(frame, app);
            ui::picker(frame, app);
            ui::confirm_prompt(frame, app);
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

/// runs the interactive terminal front end. every session the daemon knows about is a tab
/// in this one window; `session` only says which tab to land on.
// the editor holds an `Rc` clipboard, so the app is not `Send` and neither is this future.
// it is awaited by the one task that owns the terminal, never spawned.
#[allow(clippy::future_not_send)]
pub async fn run(client: &mut Client, session: Option<String>, new: bool) -> Result<(), String> {
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    // without this the terminal reports shift+enter as a plain enter, and the operator's
    // newline arrives as a send. it is pushed unconditionally: asking first needs a reply
    // many terminals never send, and one that does not speak the protocol ignores the
    // sequence.
    // SA_KEYLOG writes what the terminal actually delivers, which is the only way to tell a
    // key the terminal cannot report from a key we mishandle.
    let keylog = std::env::var_os("SA_KEYLOG").map(std::path::PathBuf::from);
    if let Some(path) = &keylog {
        use std::io::Write as _;
        let mut file = std::fs::File::create(path).ok();
        if let Some(file) = file.as_mut() {
            let _ = writeln!(
                file,
                "supports_keyboard_enhancement: {:?}",
                crossterm::terminal::supports_keyboard_enhancement()
            );
        }
    }
    let enhanced = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PushKeyboardEnhancementFlags(
            // 7, as pi requests: disambiguate, report event types and alternate keys.
            // disambiguate alone does not make every terminal report shift+enter.
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | crossterm::event::KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )
    .is_ok();
    let _restore = Restore { enhanced };

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;
    run_with(client, session, new, &mut terminal, EventStream::new()).await
}

/// the interaction itself, over a terminal and an event source the caller supplies. a test
/// drives this with a test backend and a scripted key stream, which is the only way to reach
/// the loop without owning a tty.
#[allow(clippy::future_not_send)]
async fn run_with<S, B>(
    client: &mut Client,
    session: Option<String>,
    new: bool,
    terminal: &mut Terminal<B>,
    mut keys: S,
) -> Result<(), String>
where
    B: ratatui::backend::Backend,
    S: futures_util::Stream<Item = std::io::Result<Event>> + Unpin,
{
    let here = std::env::current_dir()
        .map(|path| {
            std::fs::canonicalize(&path)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .unwrap_or_default();
    let catalog = model_catalog(client).await;
    let fallback_model = catalog
        .first()
        .map(|(id, _)| id.clone())
        .unwrap_or_default();
    let mut rows = list_sessions(client).await?;

    // landing tab: the one already working here, or a fresh session for this directory.
    let landing = if new {
        None
    } else {
        session
            .clone()
            .filter(|id| rows.iter().any(|row| row["id"] == *id))
            .or_else(|| {
                rows.iter()
                    .find(|row| row["state"] == "active" && row["cwd"] == here.as_str())
                    .and_then(|row| row["id"].as_str())
                    .map(str::to_owned)
            })
    };
    let landing = if let Some(id) = landing {
        id
    } else {
        let id = create_session(client, &here).await?;
        rows = list_sessions(client).await?;
        id
    };

    // the strip is ordered by creation time so tabs never shuffle under the operator.
    rows.sort_by_key(|row| row["created_at"].as_i64().unwrap_or_default());
    let Some(first) = rows.first() else {
        return Err("the daemon reported no sessions".to_owned());
    };
    let mut app = App::new(
        first["id"].as_str().unwrap_or_default().to_owned(),
        tab_model(first, &fallback_model),
    );
    app.image_picker = ratatui_image::picker::Picker::from_query_stdio().ok();
    app.served = catalog.into_iter().collect();
    for row in &rows {
        open_tab(client, &mut app, row, &fallback_model).await;
    }
    if let Some(index) = app.tabs.iter().position(|tab| tab.session == landing) {
        app.switch_tab(index);
    }

    // what the terminal actually delivers, when SA_KEYLOG names a file.
    let keylog = std::env::var_os("SA_KEYLOG").map(std::path::PathBuf::from);

    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut frame = Frame::new();
    frame
        .render(terminal, &mut app)
        .map_err(|e| e.to_string())?;

    loop {
        enum Incoming {
            Key(crossterm::event::KeyEvent),
            Frame(Value),
            Tick,
            Resize,
        }

        let incoming = tokio::select! {
            event = keys.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => Incoming::Key(key),
                // a resize must force a redraw, or the frames keep their old width.
                Some(Ok(Event::Resize(_, _))) => Incoming::Resize,
                Some(Ok(_)) => continue,
                Some(Err(err)) => return Err(err.to_string()),
                None => break,
            },
            frame = client.next_notification() => Incoming::Frame(
                frame.map_err(|e| format!("connection lost: {e}"))?,
            ),
            _ = ticker.tick() => Incoming::Tick,
        };

        let mut action = Action::None;
        let mut force_draw = false;
        match incoming {
            Incoming::Tick => {
                for tab in &mut app.tabs {
                    tab.tick();
                }
                // animate the spinner while anything is in flight.
                if app.tabs.iter().any(|t| t.status.turn_running) {
                    app.spinner = app.spinner.wrapping_add(1);
                    app.touch();
                }
            }
            Incoming::Resize => app.touch(),
            Incoming::Key(key) => {
                app.note_key(key);
                if let Some(path) = &keylog {
                    use std::io::Write as _;
                    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                        let _ = writeln!(file, "{key:?}");
                    }
                }
                action = app.on_key(key);
                // a key press must always produce a frame, even inside the throttle window.
                force_draw = true;
            }
            Incoming::Frame(frame) => {
                let method = frame
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match method {
                    "event" => {
                        let sid = frame["params"]["session_id"].as_str().unwrap_or_default();
                        let kind = frame["params"]["kind"].as_str().unwrap_or_default();
                        if let Some(tab) = app.tab_for(sid) {
                            tab.on_event(kind, &frame["params"]["payload"]);
                        }
                        // the title call is announced, then its result, then applied.
                        if kind == "title.generating" {
                            let payload = &frame["params"]["payload"];
                            let model = payload["model"].as_str().unwrap_or("model");
                            let effort = payload["effort"].as_str().unwrap_or("low");
                            if let Some(tab) = app.tab_for(sid) {
                                tab.push(
                                    app::Role::System,
                                    format!("generating a session title with {model}:{effort}"),
                                );
                            }
                        }
                        if kind == "session.renamed" {
                            if let Some(title) = frame["params"]["payload"]["title"].as_str() {
                                app.apply_rename(sid, title);
                            }
                        }
                    }
                    "sessions.changed" => {
                        // a session was created or deleted somewhere: every window shows
                        // every session, so bring the strip back in line.
                        let cwd = tab_cwd(&app);
                        if let Err(err) = sync_tabs(client, &mut app, &fallback_model, &cwd).await {
                            app.tab_mut().push(app::Role::Error, err);
                        }
                    }
                    _ => {}
                }
            }
        }

        match action {
            Action::None => {}
            Action::Quit => break,
            Action::NextTab => app.next_tab(),
            Action::SwitchTab(index) => app.switch_tab(index),
            Action::NewTab => {
                // a new tab starts where the current one works, not where the process
                // happens to have been started.
                let cwd = tab_cwd(&app);
                match create_session(client, &cwd).await {
                    Ok(id) => {
                        if let Err(err) = sync_tabs(client, &mut app, &fallback_model, &cwd).await {
                            app.tab_mut().push(app::Role::Error, err);
                        } else if let Some(index) =
                            app.tabs.iter().position(|tab| tab.session == id)
                        {
                            app.switch_tab(index);
                        }
                    }
                    Err(err) => app.tab_mut().push(app::Role::Error, err),
                }
            }
            Action::ResumePicked(id) => {
                // a closed session comes back before it can be shown.
                let _ = rpc(client, "session.restore", json!({"session_id": id})).await;
                if let Some(index) = app.tabs.iter().position(|tab| tab.session == id) {
                    app.switch_tab(index);
                } else {
                    let cwd = tab_cwd(&app);
                    if let Err(err) = sync_tabs(client, &mut app, &fallback_model, &cwd).await {
                        app.tab_mut().push(app::Role::Error, err);
                    } else if let Some(index) = app.tabs.iter().position(|tab| tab.session == id) {
                        app.switch_tab(index);
                    }
                }
            }
            Action::EffortPicked(level) => {
                let sid = app.tab().session.clone();
                match rpc(
                    client,
                    "model.effort",
                    json!({"session_id": sid, "effort": level}),
                )
                .await
                {
                    Ok(_) => {
                        app.tab_mut().status.effort = level.clone();
                        app.touch();
                        app.tab_mut()
                            .push(app::Role::System, format!("effort: {level}"));
                    }
                    Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                }
            }
            Action::CloseTab => {
                let cwd = tab_cwd(&app);
                if let Some(closed) = app.close_tab() {
                    // a closed tab is retired, not parked: sessions are tabs, not history.
                    let _ = rpc(client, "session.delete", json!({"session_id": closed})).await;
                } else {
                    // the last tab: retire it and let the strip refill, rather than leaving
                    // the window with nothing to type into.
                    let previous = app.tab().session.clone();
                    let _ = rpc(client, "session.delete", json!({"session_id": previous})).await;
                    if let Err(err) = sync_tabs(client, &mut app, &fallback_model, &cwd).await {
                        app.tab_mut().push(app::Role::Error, err);
                    }
                }
            }
            Action::Cancel => {
                // always sent: the view reverts on the key press, so a running turn is not
                // visible from the flag any more.
                let sid = app.tab().session.clone();
                let _ = rpc(client, "turn.cancel", json!({"session_id": sid})).await;
            }
            Action::Submit(prompt) => {
                let sid = app.tab().session.clone();
                let images: Vec<serde_json::Value> = app
                    .tab()
                    .attachments
                    .iter()
                    .map(|a| json!({"media_type": a.media_type, "data": a.data, "name": a.name}))
                    .collect();
                // a prompt sent while a turn runs redirects it rather than being refused.
                if app.tab().status.turn_running {
                    match rpc(
                        client,
                        "turn.steer",
                        json!({"session_id": sid, "prompt": prompt.clone()}),
                    )
                    .await
                    {
                        Ok(_) => {
                            app.touch();
                            frame.force();
                            frame
                                .render(terminal, &mut app)
                                .map_err(|e| e.to_string())?;
                            continue;
                        }
                        // the daemon has no turn for this session, so this window's idea of
                        // "running" is stale. start a real turn instead of losing the text.
                        Err(err) => {
                            app.tab_mut().status.turn_running = false;
                            app.tab_mut().working = None;
                            app.tab_mut().push(
                                app::Role::System,
                                format!("no turn to steer ({err}); sending as a new turn"),
                            );
                        }
                    }
                }
                app.tab_mut().status.turn_running = true;
                app.tab_mut().working = Some(app::working_word());
                app.touch();
                match rpc(
                    client,
                    "turn.start",
                    json!({
                        "session_id": sid,
                        "prompt": prompt,
                        "detach": false,
                        "images": images,
                    }),
                )
                .await
                {
                    Ok(_) => app.tab_mut().attachments.clear(),
                    Err(err) => {
                        app.tab_mut().status.turn_running = false;
                        app.tab_mut().working = None;
                        app.tab_mut()
                            .push(app::Role::Error, format!("turn.start failed: {err}"));
                    }
                }
            }
            Action::Command(command) => {
                let (name, arg) = command
                    .split_once(' ')
                    .map_or((command.as_str(), ""), |(head, tail)| (head, tail.trim()));
                let sid = app.tab().session.clone();
                match name {
                    "/exit" | "/quit" => break,
                    "/help" => {
                        app.tab_mut().push(app::Role::System, app::HELP_COMMANDS);
                    }
                    "/new" => {
                        let cwd = tab_cwd(&app);
                        match create_session(client, &cwd).await {
                            Ok(id) => {
                                if let Err(err) =
                                    sync_tabs(client, &mut app, &fallback_model, &cwd).await
                                {
                                    app.tab_mut().push(app::Role::Error, err);
                                } else if let Some(index) =
                                    app.tabs.iter().position(|tab| tab.session == id)
                                {
                                    app.switch_tab(index);
                                }
                            }
                            Err(err) => app.tab_mut().push(app::Role::Error, err),
                        }
                    }
                    "/keys" => {
                        let seen = app
                            .recent_keys
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let held = if app.shift_held { "down" } else { "up" };
                        let text = if seen.is_empty() {
                            "no keys seen yet".to_owned()
                        } else {
                            format!(
                                "keys the terminal delivered: {seen} \u{00b7} shift is {held} \
                                 \u{00b7} shift+enter shows as SHIFT when the terminal can \
                                 report it, and a bare enter while shift is down still makes \
                                 a new line; ctrl+j always does"
                            )
                        };
                        app.tab_mut().push(app::Role::System, text);
                    }
                    "/mcp" => match rpc(client, "mcp.list", json!({})).await {
                        Ok(list) => {
                            let rows = list.as_array().cloned().unwrap_or_default();
                            if rows.is_empty() {
                                app.tab_mut()
                                    .push(app::Role::System, "no mcp servers configured");
                            }
                            for row in rows {
                                let name = row["name"].as_str().unwrap_or("?");
                                let text = row["error"].as_str().map_or_else(
                                    || {
                                        let tools = row["tools"]
                                            .as_array()
                                            .map(|tools| {
                                                tools
                                                    .iter()
                                                    .filter_map(|tool| tool.as_str())
                                                    .collect::<Vec<_>>()
                                                    .join(", ")
                                            })
                                            .unwrap_or_default();
                                        format!("{name}: {tools}")
                                    },
                                    |error| format!("{name}: {error}"),
                                );
                                app.tab_mut().push(app::Role::System, text);
                            }
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                    },
                    "/effort" if arg.is_empty() => {
                        // the catalog is read at startup, before the daemon's background
                        // probe has finished, so refresh it when there is nothing to offer.
                        if app.effort_levels(app.tab()).is_empty() {
                            app.served = model_catalog(client).await.into_iter().collect();
                        }
                        // the endpoint's own values, not a list this build invented.
                        let model = app.tab().status.model.clone();
                        let current = app.tab().status.effort.clone();
                        let levels = app.effort_levels(app.tab());
                        if levels.is_empty() {
                            app.tab_mut().push(
                                app::Role::System,
                                format!("{model}: the endpoint advertises no effort levels"),
                            );
                        } else {
                            let items = levels
                                .into_iter()
                                .map(|level| app::PickerItem {
                                    label: if level == current {
                                        format!("{level} (current)")
                                    } else {
                                        level.clone()
                                    },
                                    id: level,
                                })
                                .collect();
                            app.open_picker(
                                app::PickerKind::Effort,
                                format!("{model} effort"),
                                items,
                            );
                        }
                    }
                    "/effort" => match rpc(
                        client,
                        "model.effort",
                        json!({"session_id": sid, "effort": arg}),
                    )
                    .await
                    {
                        Ok(_) => {
                            app.tab_mut().status.effort = arg.to_owned();
                            app.touch();
                            app.tab_mut()
                                .push(app::Role::System, format!("effort: {arg}"));
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                    },
                    "/model" if !arg.is_empty() => {
                        let _ = rpc(
                            client,
                            "model.select",
                            json!({"session_id": sid, "model": arg}),
                        )
                        .await;
                        app.tab_mut().status.model = arg.to_owned();
                        app.touch();
                    }
                    "/cost" => {
                        let tab = app.tab();
                        let text = format!(
                            "{} in / {} out",
                            tab.status.prompt_tokens, tab.status.completion_tokens
                        );
                        app.tab_mut().push(app::Role::System, text);
                    }
                    "/cancel" => {
                        let _ = rpc(client, "turn.cancel", json!({"session_id": sid})).await;
                    }
                    "/cwd" | "/cd" | "/pwd" if arg.is_empty() => {
                        let cwd = app.tab().cwd.clone();
                        app.tab_mut().push(app::Role::System, format!("cwd: {cwd}"));
                    }
                    "/cwd" | "/cd" => match resolve_dir(&app.tab().cwd, arg) {
                        Some(cwd) => {
                            match rpc(
                                client,
                                "session.cwd",
                                json!({"session_id": sid, "cwd": cwd}),
                            )
                            .await
                            {
                                Ok(result) => {
                                    let cwd = result["cwd"].as_str().unwrap_or(&cwd).to_owned();
                                    let branch = app::git_branch(&cwd);
                                    let tab = app.tab_mut();
                                    tab.cwd = cwd.clone();
                                    tab.branch = branch;
                                    app.touch();
                                    app.tab_mut().push(app::Role::System, format!("cwd: {cwd}"));
                                }
                                Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                            }
                        }
                        None => app
                            .tab_mut()
                            .push(app::Role::Error, format!("not a directory: {arg}")),
                    },
                    "/resume" => {
                        // every live session is already a tab, so what is worth bringing
                        // back is a session that was closed while working in this directory.
                        let cwd = app.tab().cwd.clone();
                        match rpc(client, "session.list", json!({"include_deleted": true})).await {
                            Ok(list) => {
                                let mut rows: Vec<&Value> = list
                                    .as_array()
                                    .map(|rows| {
                                        rows.iter()
                                            .filter(|row| {
                                                row["state"] == "deleted"
                                                    && row["cwd"].as_str() == Some(cwd.as_str())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                rows.sort_by_key(|row| {
                                    std::cmp::Reverse(
                                        row["updated_at"].as_i64().unwrap_or_default(),
                                    )
                                });
                                let items: Vec<app::PickerItem> = rows
                                    .iter()
                                    .take(RESUME_LIMIT)
                                    .filter_map(|row| {
                                        let id = row["id"].as_str()?;
                                        let title = row["title"].as_str().unwrap_or("(untitled)");
                                        let short: String = id.chars().take(8).collect();
                                        Some(app::PickerItem {
                                            id: id.to_owned(),
                                            label: format!("{title} ({short})"),
                                        })
                                    })
                                    .collect();
                                if items.is_empty() {
                                    app.tab_mut().push(
                                        app::Role::System,
                                        "no closed sessions in this directory",
                                    );
                                } else {
                                    app.open_picker(app::PickerKind::Resume, "resume", items);
                                }
                            }
                            Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                        }
                    }
                    _ => app
                        .tab_mut()
                        .push(app::Role::System, "unknown command, /help"),
                }
            }
        }

        if force_draw {
            frame.force();
        }
        frame
            .render(terminal, &mut app)
            .map_err(|e| e.to_string())?;
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// the model the daemon will use when a session has not chosen one.
async fn model_catalog(client: &mut Client) -> Vec<(String, app::ServedModel)> {
    rpc(client, "model.list", json!({}))
        .await
        .ok()
        .and_then(|list| {
            list.as_array().map(|models| {
                models
                    .iter()
                    .filter_map(|model| {
                        let id = model["id"].as_str()?;
                        let effort_levels = model["effort_levels"]
                            .as_array()
                            .map(|levels| {
                                levels
                                    .iter()
                                    .filter_map(|level| level.as_str())
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some((
                            id.to_owned(),
                            app::ServedModel {
                                context_limit: model["context_limit"].as_u64(),
                                effort_levels,
                            },
                        ))
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// the session rows the daemon knows about.
async fn list_sessions(client: &mut Client) -> Result<Vec<Value>, String> {
    let list = rpc(client, "session.list", json!({})).await?;
    Ok(list.as_array().cloned().unwrap_or_default())
}

/// the model a tab starts on: the session's own choice, or the daemon's default.
fn tab_model(row: &Value, fallback: &str) -> String {
    row["model"]
        .as_str()
        .filter(|model| !model.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

/// creates a session rooted at `cwd` and returns its id.
async fn create_session(client: &mut Client, cwd: &str) -> Result<String, String> {
    let created = rpc(client, "session.create", json!({"cwd": cwd})).await?;
    created["session_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "session.create returned no id".to_owned())
}

/// fills one tab from a session row: its directory, model, effort, title and history, then
/// subscribes it. a tab that already exists is left alone.
#[allow(clippy::future_not_send)]
async fn open_tab(client: &mut Client, app: &mut App, row: &Value, fallback_model: &str) {
    let Some(id) = row["id"].as_str() else {
        return;
    };
    // a tab standing in the strip without history is deaf: it shows nothing, and a turn it
    // starts is never seen to finish. hydrate on the flag, not on whether a tab object
    // happens to exist, or the app's own first slot is left permanently blank.
    if !app.needs_hydration(id) {
        if let Some(tab) = app.tab_for(id) {
            tab.apply_session_row(row);
        }
        return;
    }
    if !app.tabs.iter().any(|tab| tab.session == id) {
        app.add_tab(id.to_owned(), tab_model(row, fallback_model));
    }
    if let Some(tab) = app.tab_for(id) {
        tab.apply_session_row(row);
        tab.hydrated = true;
    }

    if let Ok(replay) = rpc(
        client,
        "events.replay",
        json!({"session_id": id, "from_seq": 0, "limit": 5000}),
    )
    .await
    {
        if let Some(events) = replay["events"].as_array() {
            for event in events {
                let kind = event["kind"].as_str().unwrap_or_default();
                if let Some(tab) = app.tab_for(id) {
                    // history, not generation: it must not touch the rate meter.
                    tab.on_replay(kind, &event["payload"]);
                }
            }
            // work that was still going when this window went away is still going now, so
            // the tab keeps showing that it is busy.
            if let Some(running) = turn_still_running(events) {
                if let Some(tab) = app.tab_for(id) {
                    tab.status.turn_running = running;
                    if running {
                        tab.working.get_or_insert_with(|| "working".to_owned());
                    } else {
                        tab.working = None;
                    }
                    tab.touch();
                }
            }
        }
    }
    let _ = rpc(
        client,
        "events.subscribe",
        json!({"session_id": id, "from_seq": 0}),
    )
    .await;
}

/// whether the history ends with a turn still in flight. `none` when it holds no turn at all.
fn turn_still_running(events: &[Value]) -> Option<bool> {
    events
        .iter()
        .rev()
        .find_map(|event| match event["kind"].as_str()? {
            "turn.started" => Some(true),
            "turn.completed" | "turn.interrupted" => Some(false),
            _ => None,
        })
}

/// brings the strip in line with the daemon: a session created anywhere becomes a tab, a
/// deleted one loses its tab, and the window never ends up with no tabs at all.
#[allow(clippy::future_not_send)]
async fn sync_tabs(
    client: &mut Client,
    app: &mut App,
    fallback_model: &str,
    cwd: &str,
) -> Result<(), String> {
    let mut rows = list_sessions(client).await?;
    if rows.is_empty() {
        // a window with no tabs has nothing to type into, so make one to fill it.
        create_session(client, cwd).await?;
        rows = list_sessions(client).await?;
    }
    let live: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
    let active = app.tab().session.clone();
    app.retain_tabs(&live);
    for row in &rows {
        open_tab(client, app, row, fallback_model).await;
    }
    if let Some(index) = app.tabs.iter().position(|tab| tab.session == active) {
        app.switch_tab(index);
    }
    Ok(())
}

/// the directory the active tab works in, falling back to the process directory.
fn tab_cwd(app: &App) -> String {
    if app.tab().cwd.is_empty() {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    } else {
        app.tab().cwd.clone()
    }
}

/// resolves a `/cwd` argument against the tab's own directory, which is what a relative
/// path means to the operator.
fn resolve_dir(current: &str, arg: &str) -> Option<String> {
    let expanded = app::expand_home(arg);
    let path = std::path::Path::new(&expanded);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::Path::new(current).join(path)
    };
    let resolved = std::fs::canonicalize(joined).ok()?;
    resolved.is_dir().then(|| resolved.display().to_string())
}

/// how many closed sessions `/resume` offers for a directory.
const RESUME_LIMIT: usize = 100;

/// how long a ui-initiated rpc may take before the front end gives up on it.
const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// calls the daemon with a deadline, so a stalled daemon cannot freeze the interface.
async fn rpc(client: &mut Client, method: &str, params: Value) -> Result<Value, String> {
    match tokio::time::timeout(RPC_TIMEOUT, client.call(method, params)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!(
            "{method} timed out after {}s",
            RPC_TIMEOUT.as_secs()
        )),
    }
}

/// the transcript of the active tab as renderable lines, for tests.
#[must_use]
pub fn render(app: &App, width: u16) -> Vec<Line<'static>> {
    app::transcript_lines(app.tab(), width)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_throttle_bounds_redraws_and_input_can_force_one() {
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::new("s", "m");
        let mut frame = Frame::new();

        frame.render(&mut terminal, &mut app).expect("draw");
        let first = frame.drawn;
        assert_ne!(first, 0, "the first frame must draw");

        // the rule is decided by the revision and the clock, not by wall time, so the test
        // cannot flake when instrumentation slows everything down.
        let now = frame.last;
        assert!(!frame.due(now, first), "an unchanged view is not due");
        assert!(
            !frame.due(now + FRAME_INTERVAL, first),
            "elapsed time alone is not enough"
        );
        assert!(
            frame.due(now + FRAME_INTERVAL, first.wrapping_add(1)),
            "a changed view after the interval is due"
        );
        assert!(
            !frame.due(now, first.wrapping_add(1)),
            "too soon is not due"
        );

        // input forces the next frame regardless of the interval, which is what keeps a
        // key press from being starved by the throttle.
        frame.force();
        assert!(
            frame.due(now, first),
            "forcing must make a frame due immediately"
        );
        frame.render(&mut terminal, &mut app).expect("draw");
        assert!(!frame.forced, "the force is spent once it is used");
        assert_eq!(
            frame.drawn, first,
            "the frame is drawn on the current revision"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod daemon_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    /// a scripted daemon on the far end of a socketpair. it answers the named methods with
    /// canned results and records every request, so a test can assert what the client
    /// actually asked the daemon to do.
    fn daemon(
        replies: Vec<(&'static str, Value)>,
    ) -> (Client, tokio::task::JoinHandle<Vec<Value>>) {
        let (ours, theirs) = UnixStream::pair().expect("socketpair");
        let handle = tokio::spawn(async move {
            let (read, mut write) = theirs.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut seen = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                seen.push(request.clone());
                let method = request["method"].as_str().unwrap_or_default();
                let result = replies
                    .iter()
                    .find(|(name, _)| *name == method)
                    .map_or_else(|| json!({}), |(_, value)| value.clone());
                let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
                if write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            seen
        });
        (Client::from_stream(ours), handle)
    }

    fn session_row(id: &str, cwd: &str) -> Value {
        json!({
            "id": id,
            "title": "work",
            "state": "active",
            "cwd": cwd,
            "model": "m",
            "effort": "high",
            "created_at": 1,
            "updated_at": 2,
        })
    }

    #[tokio::test]
    async fn open_tab_hydrates_a_tab_that_already_exists() {
        // the regression: the app's own first slot is a tab object with no history, and it
        // must still be replayed and subscribed or it stays deaf for the whole run.
        let replay = json!({"events": [
            {"kind": "turn.started", "payload": {"prompt": "hello"}},
            {"kind": "llm.delta", "payload": {"text": "hi"}},
        ]});
        let (mut client, server) = daemon(vec![
            ("events.replay", replay),
            ("events.subscribe", json!({"subscription_id": 1})),
        ]);

        let mut app = App::new("s1", "m");
        assert!(app.needs_hydration("s1"));

        open_tab(
            &mut client,
            &mut app,
            &session_row("s1", "/tmp"),
            "fallback",
        )
        .await;

        assert!(app.tab().hydrated, "the tab must be hydrated");
        assert!(
            !app.tab().transcript.is_empty(),
            "history must be replayed into the tab"
        );
        assert_eq!(app.tab().cwd, "/tmp");

        drop(client);
        let seen = server.await.expect("server");
        assert!(seen.iter().any(|r| r["method"] == "events.replay"));
        assert!(
            seen.iter().any(|r| r["method"] == "events.subscribe"),
            "a tab that is not subscribed never learns its turn finished"
        );
    }

    #[tokio::test]
    async fn open_tab_replays_only_once() {
        let (mut client, server) = daemon(vec![
            ("events.replay", json!({"events": []})),
            ("events.subscribe", json!({"subscription_id": 1})),
        ]);

        let mut app = App::new("s1", "m");
        let row = session_row("s1", "/tmp");
        open_tab(&mut client, &mut app, &row, "fallback").await;
        open_tab(&mut client, &mut app, &row, "fallback").await;

        drop(client);
        let seen = server.await.expect("server");
        let replays = seen
            .iter()
            .filter(|r| r["method"] == "events.replay")
            .count();
        assert_eq!(replays, 1, "a hydrated tab is not replayed again");
    }

    #[tokio::test]
    async fn sync_tabs_opens_a_tab_for_a_session_it_has_not_seen() {
        let list = json!([session_row("s1", "/tmp"), session_row("s2", "/tmp")]);
        let (mut client, _server) = daemon(vec![
            ("session.list", list),
            ("events.replay", json!({"events": []})),
            ("events.subscribe", json!({"subscription_id": 1})),
        ]);

        let mut app = App::new("s1", "m");
        sync_tabs(&mut client, &mut app, "m", "/tmp")
            .await
            .expect("sync");

        let sessions: Vec<&str> = app.tabs.iter().map(|tab| tab.session.as_str()).collect();
        assert_eq!(sessions, ["s1", "s2"]);
        assert!(!app.needs_hydration("s1"), "the tab is hydrated");
    }

    #[tokio::test]
    async fn sync_tabs_drops_the_tab_of_a_deleted_session() {
        let (mut client, _server) = daemon(vec![
            ("session.list", json!([session_row("s1", "/tmp")])),
            ("events.replay", json!({"events": []})),
            ("events.subscribe", json!({"subscription_id": 1})),
        ]);

        let mut app = App::new("s1", "m");
        app.tab_mut().hydrated = true;
        app.add_tab("gone", "m");
        app.tab_mut().hydrated = true;

        sync_tabs(&mut client, &mut app, "m", "/tmp")
            .await
            .expect("sync");

        let sessions: Vec<&str> = app.tabs.iter().map(|tab| tab.session.as_str()).collect();
        assert_eq!(sessions, ["s1"], "a deleted session loses its tab");
    }

    #[tokio::test]
    async fn the_catalog_reads_what_the_endpoint_advertises() {
        let list = json!([
            {"id": "big", "context_limit": 1_000_000, "effort_levels": ["none", "high"]},
            {"id": "silent"},
        ]);
        let (mut client, _server) = daemon(vec![("model.list", list)]);

        let catalog = model_catalog(&mut client).await;
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].0, "big");
        assert_eq!(catalog[0].1.context_limit, Some(1_000_000));
        assert_eq!(catalog[0].1.effort_levels, ["none", "high"]);
        assert_eq!(
            catalog[1].1.context_limit, None,
            "an endpoint that says nothing is not given a guess"
        );
    }

    #[tokio::test]
    async fn creating_a_session_returns_the_id_and_reports_a_failure() {
        let (mut client, _server) = daemon(vec![(
            "session.create",
            json!({"session_id": "abc", "seq": 1}),
        )]);
        assert_eq!(
            create_session(&mut client, "/tmp").await,
            Ok("abc".to_owned())
        );

        let (mut client, _server) = daemon(vec![("session.create", json!({}))]);
        assert!(create_session(&mut client, "/tmp").await.is_err());
    }

    #[test]
    fn a_cwd_argument_resolves_against_the_tab() {
        let base = std::env::temp_dir().join("sa-tui-resolve-dir");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("inner")).expect("dirs");
        let base = std::fs::canonicalize(&base).expect("canonical");

        let resolved = resolve_dir(&base.display().to_string(), "inner").expect("resolved");
        assert_eq!(
            std::path::Path::new(&resolved),
            base.join("inner"),
            "a relative path means relative to where the tab works"
        );
        assert!(
            resolve_dir(&base.display().to_string(), "nope").is_none(),
            "a path that does not exist is refused"
        );
    }

    #[test]
    fn the_tab_directory_falls_back_to_the_process_directory() {
        let mut app = App::new("s1", "m");
        assert_eq!(
            tab_cwd(&app),
            std::env::current_dir().expect("cwd").display().to_string(),
            "an unset tab directory uses where the process was started"
        );

        app.tab_mut().cwd = "/work/project".to_owned();
        assert_eq!(tab_cwd(&app), "/work/project");
    }
    /// keys as crossterm would deliver them, so the loop can be driven without a tty.
    fn keys(
        events: Vec<Event>,
    ) -> impl futures_util::Stream<Item = std::io::Result<Event>> + Unpin {
        futures_util::stream::iter(events.into_iter().map(Ok))
    }

    fn typed(text: &str) -> Vec<Event> {
        text.chars()
            .map(|c| {
                Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                ))
            })
            .collect()
    }

    fn press(code: crossterm::event::KeyCode) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ))
    }

    /// the replies a run needs to get as far as the composer.
    fn startup_replies() -> Vec<(&'static str, Value)> {
        vec![
            (
                "model.list",
                json!([{"id": "m", "context_limit": 1_000_000}]),
            ),
            ("session.list", json!([session_row("s1", "/tmp")])),
            ("events.replay", json!({"events": []})),
            ("events.subscribe", json!({"subscription_id": 1})),
            ("turn.start", json!({"turn_id": "t1", "seq": 1})),
        ]
    }

    #[tokio::test]
    async fn a_typed_prompt_starts_a_turn() {
        let (mut client, server) = daemon(startup_replies());
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let mut stream = typed("hello");
        stream.push(press(crossterm::event::KeyCode::Enter));

        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(stream),
        )
        .await
        .expect("run");

        drop(client);
        let seen = server.await.expect("server");
        let started = seen
            .iter()
            .find(|r| r["method"] == "turn.start")
            .expect("a turn must have been started");
        assert_eq!(started["params"]["prompt"], "hello");
        assert_eq!(started["params"]["session_id"], "s1");
    }

    #[tokio::test]
    async fn the_bar_shows_the_effort_and_the_context_meter() {
        let mut replies = startup_replies();
        // the session carries an effort, and the endpoint advertises a window.
        replies[0] = (
            "model.list",
            json!([{"id": "m", "context_limit": 1_000_000}]),
        );
        replies[1] = (
            "session.list",
            json!([{
                "id": "s1", "title": "work", "state": "active", "cwd": "/tmp",
                "model": "m", "effort": "medium", "created_at": 1, "updated_at": 2,
            }]),
        );
        let (mut client, _server) = daemon(replies);

        let backend = ratatui::backend::TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let quit = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(vec![quit]),
        )
        .await
        .expect("run");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            screen.contains("m:medium"),
            "the effort is shown:\n{screen}"
        );
        assert!(
            screen.contains('%'),
            "the meter has a percentage:\n{screen}"
        );
    }

    #[tokio::test]
    async fn ctrl_c_leaves_the_loop() {
        let (mut client, _server) = daemon(startup_replies());
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let quit = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(vec![quit]),
        )
        .await
        .expect("run");
    }

    #[test]
    fn help_lists_every_command() {
        for command in [
            "/cancel", "/cd", "/cost", "/cwd", "/effort", "/exit", "/help", "/keys", "/mcp",
            "/model", "/new", "/pwd", "/resume",
        ] {
            assert!(
                app::HELP_COMMANDS.contains(command),
                "{command} is missing from /help"
            );
        }
    }

    #[test]
    fn a_turn_in_flight_is_recognised_in_a_replayed_history() {
        let running = vec![
            json!({"kind": "turn.started"}),
            json!({"kind": "llm.delta"}),
        ];
        assert_eq!(turn_still_running(&running), Some(true));

        let finished = vec![
            json!({"kind": "turn.started"}),
            json!({"kind": "turn.completed"}),
        ];
        assert_eq!(turn_still_running(&finished), Some(false));

        assert_eq!(turn_still_running(&[]), None, "no turn at all");
    }

    #[tokio::test]
    async fn pwd_answers_locally_and_starts_no_turn() {
        let (mut client, server) = daemon(startup_replies());
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let mut stream = typed("/pwd");
        stream.push(press(crossterm::event::KeyCode::Enter));
        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(stream),
        )
        .await
        .expect("run");

        drop(client);
        let seen = server.await.expect("server");
        assert!(
            !seen.iter().any(|r| r["method"] == "turn.start"),
            "/pwd prints, it does not prompt"
        );
    }

    #[tokio::test]
    async fn cd_is_an_alias_for_cwd() {
        let (mut client, server) = daemon(startup_replies());
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let mut stream = typed("/cd /tmp");
        stream.push(press(crossterm::event::KeyCode::Enter));
        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(stream),
        )
        .await
        .expect("run");

        drop(client);
        let seen = server.await.expect("server");
        assert!(
            seen.iter()
                .any(|r| r["method"] == "session.cwd" && r["params"]["cwd"] == "/tmp"),
            "/cd must change the directory like /cwd: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_command_reaches_the_daemon() {
        // "/cost" is answered from local state, but it must not reach turn.start.
        let (mut client, server) = daemon(startup_replies());
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let mut stream = typed("/cost");
        stream.push(press(crossterm::event::KeyCode::Enter));

        run_with(
            &mut client,
            Some("s1".to_owned()),
            false,
            &mut terminal,
            keys(stream),
        )
        .await
        .expect("run");

        drop(client);
        let seen = server.await.expect("server");
        assert!(
            !seen.iter().any(|r| r["method"] == "turn.start"),
            "a slash command is not a prompt"
        );
    }
}
