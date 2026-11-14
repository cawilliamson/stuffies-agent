//! terminal front end: a scrolling transcript, an input line and a status bar.
//!
//! deliberately minimal. no mouse, no selection, no markdown, no syntax highlighting. the
//! line-mode client in the `sa` binary remains the fallback when there is no tty.

pub mod app;

use std::io::Write;

use app::{Action, App};
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
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

#[must_use]
pub fn render(app: &App) -> Vec<Line<'static>> {
    app.lines()
}

pub fn draw<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        // the scroll ceiling is measured in rendered lines: wrapped content occupies more
        // rows than it has entries, so counting entries leaves the viewport behind.
        let content = render(app);
        let width = usize::from(chunks[0].width).max(1);
        let viewport = usize::from(chunks[0].height);
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

        let body = Paragraph::new(content)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0));
        frame.render_widget(body, chunks[0]);

        let title = if app.pending_approval.is_some() {
            " approval: y=once t=turn s=session c=class n=deny "
        } else {
            " prompt "
        };
        let input = Paragraph::new(app.input.as_str()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(input, chunks[1]);

        // without this the terminal hides the cursor in raw mode and typing is blind.
        let caret = u16::try_from(app.input.chars().count()).unwrap_or(u16::MAX);
        let limit = chunks[1].width.saturating_sub(2);
        frame.set_cursor_position((chunks[1].x + 1 + caret.min(limit), chunks[1].y + 1));

        let state = if app.status.turn_running {
            "running"
        } else {
            "idle"
        };
        let short: String = app.status.session.chars().take(8).collect();
        let status = Line::from(vec![
            ratatui::text::Span::styled(
                format!(" {short} "),
                Style::default().fg(Color::Black).bg(Color::DarkGray),
            ),
            ratatui::text::Span::raw(format!(
                " {state}  {}  {} in / {} out  esc interrupt  ctrl-c quit",
                app.status.model, app.status.prompt_tokens, app.status.completion_tokens
            )),
        ]);
        frame.render_widget(Paragraph::new(status), chunks[2]);
    })?;
    Ok(())
}

/// runs the interactive terminal front end against an attached session.
pub async fn run(client: &mut Client, session: String, model: String) -> Result<(), String> {
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let _restore = Restore;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;

    let mut app = App::new(session.clone(), model);
    let mut keys = EventStream::new();

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
        };

        let mut action = Action::None;
        match incoming {
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
                    "approval.request" => {
                        let detail = frame["params"]["detail"]
                            .as_str()
                            .unwrap_or("operation")
                            .to_owned();
                        app.push(app::Role::System, format!("approval needed: {detail}"));
                        app.pending_approval =
                            frame["params"]["approval_id"].as_str().map(str::to_owned);
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
            Action::Command(command) => match command.as_str() {
                "/exit" | "/quit" => break,
                "/help" => app.push(
                    app::Role::System,
                    "/exit /quit leave, esc interrupts, ctrl-c quits",
                ),
                other => app.push(
                    app::Role::System,
                    format!("unknown command {other}, try /help"),
                ),
            },
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
            Action::Answer { allow, scope } => {
                if let Some(id) = app.pending_approval.take() {
                    let _ = client
                        .call(
                            "approval.respond",
                            json!({
                                "approval_id": id,
                                "decision": if allow { "allow" } else { "deny" },
                                "scope": scope,
                            }),
                        )
                        .await;
                }
            }
        }

        draw(&mut terminal, &mut app).map_err(|e| e.to_string())?;
        let _ = std::io::stdout().flush();
    }

    // deny anything still waiting so the daemon does not block until the approval timeout.
    if let Some(approval_id) = app.pending_approval.take() {
        let _ = client
            .call(
                "approval.respond",
                json!({"approval_id": approval_id, "decision": "deny"}),
            )
            .await;
    }
    let _ = client
        .call("session.detach", json!({"session_id": session}))
        .await;
    Ok(())
}
