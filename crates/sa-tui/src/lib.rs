//! terminal front end: session tabs, a framed transcript pane, prompt and status bar.
//!
//! one visual, no theming layer. the renderer is dirty-gated: a frame is only built when
//! something visible changed, so an idle session costs nothing.

pub mod app;
pub mod theme;

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
        }
    }

    /// makes the next render unconditional, so input is never starved by the throttle.
    const fn force(&mut self) {
        self.drawn = 0;
    }

    fn render<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        app: &mut App,
    ) -> std::io::Result<()> {
        let revision = app.revision();
        if revision == self.drawn || self.last.elapsed() < FRAME_INTERVAL {
            return Ok(());
        }
        self.drawn = revision;
        self.last = std::time::Instant::now();
        draw(terminal, app)
    }
}

struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
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
            if let Some((x, y)) = theme::at(app.theme).render(frame, app) {
                frame.set_cursor_position((x, y));
            }
            crate::theme::parts::attachment_preview(frame, app);
            crate::theme::parts::picker(frame, app);
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

/// runs the interactive terminal front end against an attached session.
pub async fn run(
    client: &mut Client,
    session: String,
    model: String,
    branch: Option<String>,
    workspace: String,
    effort: String,
) -> Result<(), String> {
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let _restore = Restore;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;

    let mut app = App::new(session.clone(), model.clone());
    app.image_picker = ratatui_image::picker::Picker::from_query_stdio().ok();
    app.branch = branch;
    app.workspace = (!workspace.is_empty()).then_some(workspace);
    app.tab_mut().status.effort = effort;
    if app.tab().status.model.is_empty() {
        if let Ok(list) = rpc(client, "model.list", json!({})).await {
            if let Some(id) = list
                .as_array()
                .and_then(|m| m.first())
                .and_then(|m| m["id"].as_str())
            {
                app.tab_mut().status.model = id.to_owned();
            }
        }
    }

    if let Ok(replay) = rpc(
        client,
        "events.replay",
        json!({"session_id": session, "from_seq": 0, "limit": 5000}),
    )
    .await
    {
        if let Some(events) = replay["events"].as_array() {
            for event in events {
                let kind = event["kind"].as_str().unwrap_or_default();
                app.tab_mut().on_event(kind, &event["payload"]);
            }
        }
    }
    rpc(
        client,
        "events.subscribe",
        json!({"session_id": session, "from_seq": 0}),
    )
    .await
    .map_err(|e| format!("subscribe failed: {e}"))?;

    let mut keys = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut frame = Frame::new();
    frame
        .render(&mut terminal, &mut app)
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
                    "evicted" => {
                        let sid = frame["params"]["session_id"].as_str().unwrap_or_default();
                        if let Some(tab) = app.tab_for(sid) {
                            tab.push(app::Role::System, "another client took this session");
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
                let cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                match rpc(client, "session.create", json!({"cwd": cwd})).await {
                    Ok(created) => {
                        if let Some(id) = created["session_id"].as_str() {
                            app.add_tab(id.to_owned(), model.clone());
                            let _ = rpc(
                                client,
                                "events.subscribe",
                                json!({"session_id": id, "from_seq": 0}),
                            )
                            .await;
                        }
                    }
                    Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                }
            }
            Action::ResumePicked(id) => {
                if let Some(index) = app.tabs.iter().position(|tab| tab.session == id) {
                    app.switch_tab(index);
                } else {
                    app.add_tab(id.clone(), model.clone());
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
                                if let Some(tab) = app.tab_for(&id) {
                                    tab.on_event(kind, &event["payload"]);
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
            }
            Action::CloseTab => {
                if app.tabs.len() <= 1 {
                    // closing the last tab leaves a fresh one rather than an empty screen.
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    match rpc(client, "session.create", json!({"cwd": cwd})).await {
                        Ok(created) => {
                            if let Some(id) = created["session_id"].as_str() {
                                let id = id.to_owned();
                                let previous = app.tab().session.clone();
                                app.replace_active(id.clone(), model.clone());
                                let _ =
                                    rpc(client, "session.delete", json!({"session_id": previous}))
                                        .await;
                                let _ = rpc(
                                    client,
                                    "events.subscribe",
                                    json!({"session_id": id, "from_seq": 0}),
                                )
                                .await;
                            }
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                    }
                } else if let Some(closed) = app.close_tab() {
                    // a closed tab is retired, not parked: sessions are tabs, not history.
                    let _ = rpc(client, "session.delete", json!({"session_id": closed})).await;
                }
            }
            Action::Cancel => {
                let tab = app.tab();
                if tab.status.turn_running {
                    let sid = tab.session.clone();
                    let _ = rpc(client, "turn.cancel", json!({"session_id": sid})).await;
                    app.tab_mut().push(app::Role::System, "interrupting");
                }
            }
            Action::Submit(prompt) => {
                let sid = app.tab().session.clone();
                let images: Vec<serde_json::Value> = app
                    .tab()
                    .attachments
                    .iter()
                    .map(|a| json!({"media_type": a.media_type, "data": a.data}))
                    .collect();
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
                    "/help" => app.tab_mut().push(
                        app::Role::System,
                        "ctrl+n new tab · ctrl+w close tab · shift+tab switch · esc interrupt · ctrl-c quit",
                    ),
                    "/new" => {
                        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
                        if let Ok(created) = rpc(client, "session.create", json!({"cwd": cwd})).await {
                            if let Some(id) = created["session_id"].as_str() {
                                app.add_tab(id.to_owned(), model.clone());
                                let _ = rpc(client, "events.subscribe", json!({"session_id": id, "from_seq": 0}))
                                    .await;
                            }
                        }
                    }
                    "/theme" if arg.is_empty() => {
                        let names: Vec<&str> =
                            theme::THEMES.iter().map(|t| t.name()).collect();
                        app.tab_mut().push(
                            app::Role::System,
                            format!("themes: {}", names.join(", ")),
                        );
                    }
                    "/theme" => match theme::index_of(arg) {
                        Some(index) => {
                            app.theme = index;
                            app.touch();
                            app.tab_mut()
                                .push(app::Role::System, format!("theme: {arg}"));
                        }
                        None => app.tab_mut().push(
                            app::Role::Error,
                            format!("no theme named {arg}, try /theme"),
                        ),
                    },
                    "/effort" if arg.is_empty() => {
                        let levels = crate::app::Status::default();
                        let _ = levels;
                        app.tab_mut().push(
                            app::Role::System,
                            "effort levels: low, medium, high",
                        );
                    }
                    "/effort" => match rpc(client, "model.effort", json!({"session_id": sid, "effort": arg}))
                        .await
                    {
                        Ok(_) => {
                            app.tab_mut().status.effort = arg.to_owned();
                            app.touch();
                            app.tab_mut().push(app::Role::System, format!("effort: {arg}"));
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                    },
                    "/model" if !arg.is_empty() => {
                        let _ = rpc(client, "model.select", json!({"session_id": sid, "model": arg}))
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
                    "/resume" => match rpc(client, "session.list", json!({})).await {
                        Ok(list) => {
                            let items: Vec<app::PickerItem> = list
                                .as_array()
                                .map(|rows| {
                                    rows.iter()
                                        .filter_map(|row| {
                                            let id = row["id"].as_str()?;
                                            let title =
                                                row["title"].as_str().unwrap_or("(untitled)");
                                            let short: String = id.chars().take(8).collect();
                                            Some(app::PickerItem {
                                                id: id.to_owned(),
                                                label: format!("{title} ({short})"),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            if items.is_empty() {
                                app.tab_mut().push(app::Role::System, "no sessions to resume");
                            } else {
                                app.open_picker("resume", items);
                            }
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.clone()),
                    },
                    _ => app.tab_mut().push(app::Role::System, "unknown command, /help"),
                }
            }
        }

        if force_draw {
            frame.force();
        }
        frame
            .render(&mut terminal, &mut app)
            .map_err(|e| e.to_string())?;
        let _ = std::io::stdout().flush();
    }

    for tab in &app.tabs {
        let _ = rpc(client, "session.detach", json!({"session_id": tab.session})).await;
    }
    Ok(())
}

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

        app.touch();
        frame.render(&mut terminal, &mut app).expect("draw");
        assert_eq!(
            frame.drawn, first,
            "a redraw inside the interval must be skipped"
        );

        // input forces the next frame regardless of the interval.
        frame.force();
        frame.render(&mut terminal, &mut app).expect("draw");
        assert_ne!(frame.drawn, first, "forcing must redraw");
    }
}
