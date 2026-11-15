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
    app.branch = branch;
    app.workspace = (!workspace.is_empty()).then_some(workspace);
    app.tab_mut().status.effort = effort;
    if app.tab().status.model.is_empty() {
        if let Ok(list) = client.call("model.list", json!({})).await {
            if let Some(id) = list
                .as_array()
                .and_then(|m| m.first())
                .and_then(|m| m["id"].as_str())
            {
                app.tab_mut().status.model = id.to_owned();
            }
        }
    }

    if let Ok(replay) = client
        .call(
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
    client
        .call(
            "events.subscribe",
            json!({"session_id": session, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    let mut keys = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // only build a frame when something visible changed.
    let mut drawn = 0u64;
    let mut render = |terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App| {
        let revision = app.revision();
        if revision == drawn {
            return Ok(());
        }
        drawn = revision;
        draw(terminal, app)
    };
    render(&mut terminal, &mut app).map_err(|e| e.to_string())?;

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
            Incoming::Key(key) => action = app.on_key(key),
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
                            let model = frame["params"]["payload"]["model"]
                                .as_str()
                                .unwrap_or("model")
                                .to_owned();
                            if let Some(tab) = app.tab_for(sid) {
                                tab.push(
                                    app::Role::System,
                                    format!("generating a session title with {model} (low effort)"),
                                );
                            }
                        }
                        if kind == "session.renamed" {
                            if let Some(title) = frame["params"]["payload"]["title"].as_str() {
                                let title = title.to_owned();
                                let short: String = sid.chars().take(8).collect();
                                if let Some(tab) = app.tab_for(sid) {
                                    // the id always travels with the summary.
                                    tab.title = format!("{title} ({short})");
                                    tab.push(app::Role::System, format!("title: {title}"));
                                }
                                app.touch();
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
                match client.call("session.create", json!({"cwd": cwd})).await {
                    Ok(created) => {
                        if let Some(id) = created["session_id"].as_str() {
                            app.add_tab(id.to_owned(), model.clone());
                            let _ = client
                                .call("events.subscribe", json!({"session_id": id, "from_seq": 0}))
                                .await;
                        }
                    }
                    Err(err) => app.tab_mut().push(app::Role::Error, err.to_string()),
                }
            }
            Action::CloseTab => {
                if app.tabs.len() <= 1 {
                    // closing the last tab leaves a fresh one rather than an empty screen.
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    match client.call("session.create", json!({"cwd": cwd})).await {
                        Ok(created) => {
                            if let Some(id) = created["session_id"].as_str() {
                                let id = id.to_owned();
                                let previous = app.tab().session.clone();
                                app.replace_active(id.clone(), model.clone());
                                let _ = client
                                    .call("session.delete", json!({"session_id": previous}))
                                    .await;
                                let _ = client
                                    .call(
                                        "events.subscribe",
                                        json!({"session_id": id, "from_seq": 0}),
                                    )
                                    .await;
                            }
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.to_string()),
                    }
                } else if let Some(closed) = app.close_tab() {
                    // a closed tab is retired, not parked: sessions are tabs, not history.
                    let _ = client
                        .call("session.delete", json!({"session_id": closed}))
                        .await;
                }
            }
            Action::Cancel => {
                let tab = app.tab();
                if tab.status.turn_running {
                    let sid = tab.session.clone();
                    let _ = client.call("turn.cancel", json!({"session_id": sid})).await;
                    app.tab_mut().push(app::Role::System, "interrupting");
                }
            }
            Action::Submit(prompt) => {
                let sid = app.tab().session.clone();
                app.tab_mut().status.turn_running = true;
                app.touch();
                match client
                    .call(
                        "turn.start",
                        json!({"session_id": sid, "prompt": prompt, "detach": false}),
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        app.tab_mut().status.turn_running = false;
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
                        if let Ok(created) = client.call("session.create", json!({"cwd": cwd})).await {
                            if let Some(id) = created["session_id"].as_str() {
                                app.add_tab(id.to_owned(), model.clone());
                                let _ = client
                                    .call("events.subscribe", json!({"session_id": id, "from_seq": 0}))
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
                    "/effort" => match client
                        .call("model.effort", json!({"session_id": sid, "effort": arg}))
                        .await
                    {
                        Ok(_) => {
                            app.tab_mut().status.effort = arg.to_owned();
                            app.touch();
                            app.tab_mut().push(app::Role::System, format!("effort: {arg}"));
                        }
                        Err(err) => app.tab_mut().push(app::Role::Error, err.to_string()),
                    },
                    "/model" if !arg.is_empty() => {
                        let _ = client
                            .call("model.select", json!({"session_id": sid, "model": arg}))
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
                        let _ = client.call("turn.cancel", json!({"session_id": sid})).await;
                    }
                    _ => app.tab_mut().push(app::Role::System, "unknown command, /help"),
                }
            }
        }

        render(&mut terminal, &mut app).map_err(|e| e.to_string())?;
        let _ = std::io::stdout().flush();
    }

    for tab in &app.tabs {
        let _ = client
            .call("session.detach", json!({"session_id": tab.session}))
            .await;
    }
    Ok(())
}

/// the transcript of the active tab as renderable lines, for tests.
#[must_use]
pub fn render(app: &App, width: u16) -> Vec<Line<'static>> {
    app::transcript_lines(app.tab(), width)
}
