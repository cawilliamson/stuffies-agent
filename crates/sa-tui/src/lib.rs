//! terminal front end: session tabs, framed transcript windows, prompt and status bar.
//!
//! deliberately one visual. no theming layer.

pub mod app;

use std::io::Write;

use app::{Action, App, Tab};
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use sa_client::Client;
use serde_json::{json, Value};

const RULE: Color = Color::Rgb(0x24, 0x28, 0x30);
const MUTED: Color = Color::Rgb(0x64, 0x6d, 0x7c);
const FAINT: Color = Color::Rgb(0x3a, 0x40, 0x4b);
const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);
const SEP: &str = "\u{e0b0}";

/// restores the terminal however the app exits, including on panic.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

fn tab_strip(app: &App) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        " sa ",
        Style::default().fg(Color::Rgb(0x10, 0x14, 0x1a)).bg(ACCENT),
    )];
    for (index, tab) in app.tabs.iter().enumerate() {
        let active = index == app.active;
        let label = format!(" {} {} \u{2715} ", index + 1, tab.title);
        spans.push(Span::styled(
            label,
            if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(0xd8, 0xd8, 0xd8))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED).bg(Color::Rgb(0x1b, 0x1f, 0x26))
            },
        ));
        spans.push(Span::styled(" ", Style::default()));
    }
    spans.push(Span::styled(
        "\u{002b}",
        Style::default().fg(FAINT).bg(Color::Rgb(0x1b, 0x1f, 0x26)),
    ));
    Line::from(spans)
}

fn status_bar(app: &App, width: u16) -> Line<'static> {
    let tab = app.tab();
    let short: String = tab.session.chars().take(8).collect();
    let mut segments: Vec<(String, Color, Color)> = vec![(
        format!(" {} ", short),
        Color::Rgb(0xd8, 0xd8, 0xd8),
        Color::Rgb(0x2a, 0x2a, 0x2a),
    )];
    if let Some(branch) = &app.branch {
        segments.push((
            format!(" \u{e725} {branch} "),
            Color::Rgb(0xd8, 0xd8, 0xf0),
            Color::Rgb(0x3b, 0x3b, 0x5c),
        ));
    }
    if !tab.status.model.is_empty() {
        segments.push((
            format!(" \u{f2db} {} ", tab.status.model),
            Color::Rgb(0xe0, 0xe8, 0xff),
            Color::Rgb(0x2f, 0x45, 0x66),
        ));
    }
    segments.push((
        format!(" \u{f0e4} {:.1} t/s ", tab.tok_per_sec),
        if tab.flash_highlight {
            Color::Rgb(0xff, 0xff, 0xff)
        } else {
            Color::Rgb(0xff, 0xd8, 0xd8)
        },
        if tab.flash_highlight {
            Color::Rgb(0xc0, 0x30, 0x30)
        } else {
            Color::Rgb(0x5c, 0x2f, 0x2f)
        },
    ));
    segments.push((
        format!(
            " \u{f0e7} {}/{} ",
            compact(tab.status.prompt_tokens),
            compact(tab.status.completion_tokens)
        ),
        Color::Rgb(0xd8, 0xd8, 0xd8),
        Color::Rgb(0x2a, 0x2a, 0x2a),
    ));

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, (text, fg, bg)) in segments.iter().enumerate() {
        spans.push(Span::styled(text.clone(), Style::default().fg(*fg).bg(*bg)));
        let next = segments.get(index + 1).map_or(Color::Reset, |s| s.2);
        spans.push(Span::styled(SEP, Style::default().fg(*bg).bg(next)));
    }
    let used = Line::from(spans.clone()).width();
    let hints = " shift+tab tab \u{00b7} \u{21e7}ctrl+n new \u{00b7} \u{21e7}ctrl+w close ";
    let pad = usize::from(width).saturating_sub(used + hints.chars().count());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(hints, Style::default().fg(FAINT)));
    Line::from(spans)
}

fn compact(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{}M", count / 1_000_000)
    } else if count >= 1_000 {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

pub fn draw<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    terminal
        .draw(|frame| {
            let area = frame.area();
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(3),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .split(area);

            frame.render_widget(Paragraph::new(tab_strip(app)), rows[0]);

            // pin the viewport to the newest content when following.
            let content = app::windows(app, rows[1].width);
            let width = usize::from(rows[1].width).max(1);
            let viewport = usize::from(rows[1].height);
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
            let tab = app.tab_mut();
            tab.max_scroll = u16::try_from(visual.saturating_sub(viewport)).unwrap_or(u16::MAX);
            if tab.follow {
                tab.scroll = tab.max_scroll;
            } else {
                tab.scroll = tab.scroll.min(tab.max_scroll);
            }
            let scroll = tab.scroll;

            frame.render_widget(
                Paragraph::new(content)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0)),
                rows[1],
            );

            let tab = app.tab();
            let field_width = usize::from(rows[2].width.saturating_sub(4));
            let (hscroll, column) = tab.input_view(field_width);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{276f} ", Style::default().fg(ACCENT)),
                    Span::raw(tab.input_text().to_owned()),
                ]))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::default().fg(RULE))
                        .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
                ),
                rows[2],
            );
            frame.set_cursor_position((
                rows[2].x + 3 + u16::try_from(column).unwrap_or(0),
                rows[2].y + 1,
            ));

            frame.render_widget(Paragraph::new(status_bar(app, rows[3].width)), rows[3]);
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
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let _restore = Restore;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;

    let mut app = App::new(session.clone(), model.clone());
    app.branch = branch;
    app.workspace = (!workspace.is_empty()).then_some(workspace);
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

    // replay history into the first tab, then subscribe.
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
            Incoming::Tick => {
                for tab in &mut app.tabs {
                    tab.tick();
                }
            }
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
                if let Some(closed) = app.close_tab() {
                    let _ = client
                        .call("session.detach", json!({"session_id": closed}))
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
                        "shift+tab next tab · shift+ctrl+n new tab · shift+ctrl+w close tab · esc interrupt · ctrl-c quit",
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
                    "/model" if !arg.is_empty() => {
                        let _ = client
                            .call("model.select", json!({"session_id": sid, "model": arg}))
                            .await;
                        app.tab_mut().status.model = arg.to_owned();
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

        draw(&mut terminal, &mut app).map_err(|e| e.to_string())?;
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
    app::windows(app, width)
}

/// a tab with sample content, for tests and previews.
#[must_use]
pub fn sample_tab(session: &str, model: &str) -> Tab {
    Tab::new(session, String::new(), model)
}
