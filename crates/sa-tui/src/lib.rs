//! terminal front end: a scrolling transcript, an input line and a status bar.
//!
//! deliberately minimal. no mouse, no selection, no markdown, no syntax highlighting. the
//! line-mode client in the `sa` binary remains the fallback when there is no tty.

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

/// restores the terminal however the app exits, including on panic.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

/// the transcript as renderable lines.
#[must_use]
pub fn render(app: &App) -> Vec<Line<'static>> {
    app.lines()
}

/// draws the active theme. returns where the cursor belongs.
pub fn draw<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    terminal
        .draw(|frame| {
            // the scroll ceiling is measured in rendered lines: wrapped content occupies more
            // rows than it has entries, so counting entries leaves the viewport behind.
            let content = render(app);
            let theme = theme::at(app.theme);
            let width = usize::from(frame.area().width).max(1);
            let viewport = usize::from(theme.transcript_rows(frame.area()));
            let visual: usize = content
                .iter()
                .map(|line| {
                    let w = line.width();
                    if w == 0 {
                        1
                    } else {
                        (w / width) + 1
                    }
                })
                .sum();
            let max = visual.saturating_sub(viewport);
            app.max_scroll = u16::try_from(max).unwrap_or(u16::MAX);
            if app.follow {
                app.scroll = app.max_scroll;
            } else {
                app.scroll = app.scroll.min(app.max_scroll);
            }

            if let Some((x, y)) = theme.render(frame, app) {
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
) -> Result<(), String> {
    let mut session = session;
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let _restore = Restore;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;

    let mut app = App::new(session.clone(), model.clone());
    app.branch = branch;
    app.workspace = (!workspace.is_empty()).then_some(workspace);

    // the cli asks the daemon what it is serving rather than displaying a placeholder.
    if app.status.model.is_empty() {
        if let Ok(list) = client.call("model.list", json!({})).await {
            if let Some(id) = list
                .as_array()
                .and_then(|models| models.first())
                .and_then(|m| m["id"].as_str())
            {
                app.status.model = id.to_owned();
            }
        }
    }
    let mut keys = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // replay what already happened so the transcript is continuous across attaches.
    app.replaying = true;
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
                app.on_event(kind, &event["payload"]);
            }
        }
    }
    app.replaying = false;

    client
        .call(
            "events.subscribe",
            json!({"session_id": session, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    draw(&mut terminal, &mut app).map_err(|e| e.to_string())?;

    loop {
        enum Incoming {
            Key(crossterm::event::KeyEvent),
            Frame(Value),
            Tick,
        }

        let incoming = tokio::select! {
            event = keys.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => Incoming::Key(key),
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
            Incoming::Tick => app.tick(),
            Incoming::Key(key) => action = app.on_key(key),
            Incoming::Frame(frame) => {
                let method = frame
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match method {
                    "event" => {
                        let kind = frame["params"]["kind"].as_str().unwrap_or_default();
                        app.on_event(kind, &frame["params"]["payload"]);
                    }
                    "evicted" => {
                        app.push(
                            app::Role::System,
                            "evicted: another client took this session",
                        );
                        break;
                    }
                    _ => {}
                }
            }
        }

        match action {
            Action::None => {}
            Action::Quit => break,
            Action::Cancel => {
                // escape interrupts a turn; with nothing running it does nothing.
                if app.status.turn_running {
                    let _ = client
                        .call("turn.cancel", json!({"session_id": session}))
                        .await;
                    app.push(app::Role::System, "interrupting");
                }
            }
            Action::Command(command) => {
                let (name, arg) = command
                    .split_once(' ')
                    .map_or((command.as_str(), ""), |(head, tail)| (head, tail.trim()));
                match name {
                    "/exit" | "/quit" => break,
                    "/help" => app.push(
                        app::Role::System,
                        "esc interrupt · ctrl-c quit · /new /model <id> /cost /cancel",
                    ),
                    "/cancel" => {
                        let _ = client
                            .call("turn.cancel", json!({"session_id": session}))
                            .await;
                        app.push(app::Role::System, "interrupting");
                    }
                    "/cost" => match client
                        .call("session.usage", json!({"session_id": session}))
                        .await
                    {
                        Ok(usage) => app.push(
                            app::Role::System,
                            format!(
                                "{} in / {} out",
                                usage["prompt_tokens"], usage["completion_tokens"]
                            ),
                        ),
                        Err(err) => app.push(app::Role::Error, err.to_string()),
                    },
                    "/new" => match client
                        .call(
                            "session.create",
                            json!({"cwd": std::env::current_dir()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(), }),
                        )
                        .await
                    {
                        Ok(created) => {
                            session = created["session_id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned();
                            app = App::new(session.clone(), model.clone());
                            let _ = client
                                .call(
                                    "events.subscribe",
                                    json!({"session_id": session, "from_seq": 0}),
                                )
                                .await;
                            app.push(app::Role::System, "new session");
                        }
                        Err(err) => app.push(app::Role::Error, err.to_string()),
                    },
                    "/theme" if arg.is_empty() => {
                        let names: Vec<&str> = theme::THEMES.iter().map(|t| t.name()).collect();
                        app.push(app::Role::System, format!("themes: {}", names.join(", ")));
                    }
                    "/theme" => match theme::index_of(arg) {
                        Some(index) => {
                            app.theme = index;
                            app.push(app::Role::System, format!("theme: {arg}"));
                        }
                        None => app.push(
                            app::Role::Error,
                            format!("no theme named {arg}, try /theme"),
                        ),
                    },
                    "/model" if !arg.is_empty() => {
                        let _ = client
                            .call("model.select", json!({"session_id": session, "model": arg}))
                            .await;
                        app.status.model = arg.to_owned();
                        app.push(app::Role::System, format!("model: {arg}"));
                    }
                    // the command itself is not chat, so the reply does not repeat it.
                    _ => app.push(app::Role::System, "unknown command, /help for the list"),
                }
            }
            Action::Submit(prompt) => {
                app.push(app::Role::User, prompt.clone());
                app.status.turn_running = true;
                match client
                    .call(
                        "turn.start",
                        json!({"session_id": session, "prompt": prompt, "detach": false}),
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        app.status.turn_running = false;
                        app.push(app::Role::Error, format!("turn.start failed: {err}"));
                    }
                }
            }
        }

        draw(&mut terminal, &mut app).map_err(|e| e.to_string())?;
        let _ = std::io::stdout().flush();
    }

    let _ = client
        .call("session.detach", json!({"session_id": session}))
        .await;
    Ok(())
}
