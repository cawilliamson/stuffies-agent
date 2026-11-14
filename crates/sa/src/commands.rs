//! the user-facing commands: run, attach, export, sessions (spec 14, v1.1).

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};

use sa_client::Client;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

/// how much of a tool result is shown before it is elided.
const TOOL_PREVIEW_LINES: usize = 8;
const TOOL_PREVIEW_CHARS: usize = 1200;

fn socket() -> std::path::PathBuf {
    crate::cli::socket_path()
}

async fn connect() -> Result<Client> {
    let path = socket();
    let mut client = Client::connect(&path)
        .await
        .map_err(|e| format!("cannot reach daemon at {}: {e}", path.display()))?;
    client
        .handshake("sa")
        .await
        .map_err(|e| format!("handshake failed: {e}"))?;
    Ok(client)
}

fn cwd() -> Result<String> {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .map_err(|e| format!("cannot read cwd: {e}"))
}

/// the workspace branch, for the status bar. absent outside a repository.
fn git_branch() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
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

// ---- rendering ----------------------------------------------------------

/// renders a session transcript for a terminal.
#[derive(Default)]
struct Renderer {
    tool_names: HashMap<String, String>,
    show_reasoning: bool,
}

impl Renderer {
    fn event(&mut self, kind: &str, payload: &Value) {
        match kind {
            "llm.delta" => {
                if let Some(text) = payload.get("text").and_then(Value::as_str) {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            "llm.reasoning" => {
                if self.show_reasoning {
                    if let Some(text) = payload.get("text").and_then(Value::as_str) {
                        print!("\x1b[2m{text}\x1b[0m");
                        let _ = std::io::stdout().flush();
                    }
                }
            }
            "tool.requested" => self.tool_requested(payload),
            "tool.completed" => self.tool_completed(payload),
            "tool.failed" => {
                let error = payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                println!("\n  \x1b[31m✗ {error}\x1b[0m");
            }
            "turn.completed" => {
                println!();
                let usage = &payload["usage"];
                let prompt = usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let completion = usage
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                println!("\x1b[2m[{prompt} in / {completion} out]\x1b[0m");
            }
            "turn.interrupted" => {
                let cause = payload
                    .get("cause")
                    .and_then(Value::as_str)
                    .unwrap_or("interrupted");
                println!("\n\x1b[31m[turn interrupted: {cause}]\x1b[0m");
            }
            "compaction.applied" => {
                println!("\x1b[2m[compacted: context summarised]\x1b[0m");
            }
            _ => {}
        }
    }

    fn tool_requested(&mut self, payload: &Value) {
        let tool = payload
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
            self.tool_names.insert(call_id.to_owned(), tool.to_owned());
        }
        println!(
            "\n  \x1b[36m→ {tool}\x1b[0m {}",
            summarise_args(&payload["args"])
        );
    }

    fn tool_completed(&mut self, payload: &Value) {
        let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
        let tool = self.tool_names.get(call_id).cloned().unwrap_or_default();
        let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
        for line in preview(output, &tool) {
            println!("    {line}");
        }
        if payload.get("truncated").and_then(Value::as_bool) == Some(true) {
            println!("    \x1b[2m[truncated by the daemon]\x1b[0m");
        }
    }
}

/// a one-line summary of tool arguments.
fn summarise_args(args: &Value) -> String {
    let pick = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default()
    };
    for key in ["path", "command", "pattern"] {
        let value = pick(key);
        if !value.is_empty() {
            return truncate_chars(&value, 100);
        }
    }
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        return format!("{} bytes", content.len());
    }
    String::new()
}

/// the readable window of a tool result.
fn preview(output: &str, tool: &str) -> Vec<String> {
    if output.trim().is_empty() {
        return vec!["(no output)".to_owned()];
    }
    let mut lines: Vec<String> = output
        .lines()
        .take(TOOL_PREVIEW_LINES)
        .map(|line| truncate_chars(line, 160))
        .collect();
    let total = output.lines().count();
    if total > TOOL_PREVIEW_LINES {
        lines.push(format!(
            "\x1b[2m… {total} lines, showing {TOOL_PREVIEW_LINES}\x1b[0m"
        ));
    }
    if let Some(last) = lines.last_mut() {
        if last.len() > TOOL_PREVIEW_CHARS {
            *last = truncate_chars(last, TOOL_PREVIEW_CHARS);
        }
    }
    if tool == "bash" {
        // the exit code is the first line of a bash result and the most useful part.
        return lines;
    }
    lines
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

// ---- session helpers ----------------------------------------------------

async fn create_session(client: &mut Client) -> Result<String> {
    let response = client
        .call("session.create", json!({"cwd": cwd()?}))
        .await
        .map_err(|e| format!("session.create failed: {e}"))?;
    response["session_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "session.create returned no id".to_owned())
}

async fn most_recent_session(client: &mut Client) -> Result<Option<String>> {
    let response = client
        .call("session.list", json!({}))
        .await
        .map_err(|e| format!("session.list failed: {e}"))?;
    Ok(response
        .as_array()
        .and_then(|sessions| sessions.first())
        .and_then(|s| s["id"].as_str())
        .map(str::to_owned))
}

fn render_history(renderer: &mut Renderer, events: &[Value]) {
    for event in events {
        let kind = event["kind"].as_str().unwrap_or_default();
        // history is replayed as a flat transcript, not a live stream.
        if kind == "llm.delta" {
            continue;
        }
        renderer.event(kind, &event["payload"]);
    }
}

// ---- sessions -----------------------------------------------------------

pub async fn sessions() -> Result<()> {
    let mut client = connect().await?;
    let response = client
        .call("session.list", json!({}))
        .await
        .map_err(|e| format!("session.list failed: {e}"))?;
    let Some(sessions) = response.as_array() else {
        println!("no sessions");
        return Ok(());
    };
    if sessions.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    println!("{:<38} {:<12} TITLE", "ID", "STATE");
    for session in sessions {
        println!(
            "{:<38} {:<12} {}",
            session["id"].as_str().unwrap_or("-"),
            session["state"].as_str().unwrap_or("-"),
            session["title"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

// ---- run ----------------------------------------------------------------

pub async fn run_once(prompt: &str) -> Result<()> {
    let mut client = connect().await?;
    let session = create_session(&mut client).await?;

    client
        .call(
            "events.subscribe",
            json!({"session_id": session, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    let started = client
        .call(
            "turn.start",
            json!({"session_id": session, "prompt": prompt, "detach": true}),
        )
        .await
        .map_err(|e| format!("turn.start failed: {e}"))?;
    let turn_id = started["turn_id"].as_str().unwrap_or_default().to_owned();

    let mut renderer = Renderer::default();
    loop {
        let frame = client
            .next_notification()
            .await
            .map_err(|e| format!("connection lost: {e}"))?;
        if frame.get("method").and_then(Value::as_str) != Some("event") {
            continue;
        }
        let params = &frame["params"];
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        renderer.event(kind, &params["payload"]);
        if kind == "turn.completed" || kind == "turn.interrupted" {
            let matches = params["payload"]["turn_id"].as_str() == Some(turn_id.as_str());
            if matches || turn_id.is_empty() {
                println!();
                return Ok(());
            }
        }
    }
}

// ---- attach -------------------------------------------------------------

enum Incoming {
    Line(String),
    Frame(Value),
    Interrupt,
}

fn help() {
    println!(
        "\x1b[2mcommands: /new /sessions /model <id> /cancel /cost /reasoning /help /exit\n\
         ctrl-c cancels the running turn; at an idle prompt it exits.\x1b[0m"
    );
}

pub async fn attach(session: Option<String>, new: bool, line: bool) -> Result<()> {
    let mut client = connect().await?;

    let mut session_id = if new {
        create_session(&mut client).await?
    } else if let Some(id) = session {
        id
    } else {
        match most_recent_session(&mut client).await? {
            Some(id) => id,
            None => create_session(&mut client).await?,
        }
    };

    let attached = client
        .call(
            "session.attach",
            json!({"session_id": session_id, "from_seq": 0, "takeover": true}),
        )
        .await
        .map_err(|e| format!("attach failed: {e}"))?;

    // a terminal gets the tui; a pipe gets plain lines, which is also what the tests drive.
    if !line && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        // empty means "not known yet": the tui queries the daemon and omits the area.
        let model = attached["session"]["model"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let workspace = cwd().unwrap_or_default();
        return sa_tui::run(&mut client, session_id, model, git_branch(), workspace).await;
    }

    let mut renderer = Renderer::default();
    if let Some(events) = attached["events"].as_array() {
        render_history(&mut renderer, events);
    }

    client
        .call(
            "events.subscribe",
            json!({"session_id": session_id, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    println!(
        "sa: attached to {session_id}. /help for commands, ctrl-c to interrupt, /exit to leave."
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(text) => {
                    if tx.blocking_send(text).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let mut turn_running = false;
    let mut quit_after_turn = false;

    loop {
        let incoming = tokio::select! {
            line = rx.recv() => match line {
                Some(line) => Incoming::Line(line),
                None => break,
            },
            frame = client.next_notification() => Incoming::Frame(
                frame.map_err(|e| format!("connection lost: {e}"))?,
            ),
            _ = tokio::signal::ctrl_c() => Incoming::Interrupt,
        };

        match incoming {
            Incoming::Interrupt => {
                if turn_running {
                    client
                        .call("turn.cancel", json!({"session_id": session_id}))
                        .await
                        .map_err(|e| format!("turn.cancel failed: {e}"))?;
                    println!("\n\x1b[2m[cancelling]\x1b[0m");
                } else {
                    break;
                }
            }
            Incoming::Line(line) => {
                let text = line.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(command) = text.strip_prefix('/') {
                    match command
                        .split_once(' ')
                        .map_or((command, ""), |(a, b)| (a, b.trim()))
                    {
                        ("exit" | "quit" | "q", _) => {
                            if turn_running {
                                quit_after_turn = true;
                            } else {
                                break;
                            }
                        }
                        ("help", _) => help(),
                        ("new", _) => {
                            session_id = create_session(&mut client).await?;
                            client
                                .call(
                                    "events.subscribe",
                                    json!({"session_id": session_id, "from_seq": 0}),
                                )
                                .await
                                .map_err(|e| format!("subscribe failed: {e}"))?;
                            println!("sa: new session {session_id}");
                        }
                        ("sessions", _) => sessions().await?,
                        ("model", model) if !model.is_empty() => {
                            client
                                .call(
                                    "model.select",
                                    json!({"session_id": session_id, "model": model}),
                                )
                                .await
                                .map_err(|e| format!("model.select failed: {e}"))?;
                            println!("sa: model set to {model}");
                        }
                        ("cancel", _) => {
                            client
                                .call("turn.cancel", json!({"session_id": session_id}))
                                .await
                                .map_err(|e| format!("turn.cancel failed: {e}"))?;
                        }
                        ("cost", _) => {
                            let usage = client
                                .call("session.usage", json!({"session_id": session_id}))
                                .await
                                .map_err(|e| format!("session.usage failed: {e}"))?;
                            println!(
                                "{} in / {} out",
                                usage["prompt_tokens"], usage["completion_tokens"]
                            );
                        }
                        ("reasoning", arg) => {
                            renderer.show_reasoning = matches!(arg, "on" | "true" | "1");
                            println!(
                                "sa: reasoning {}",
                                if renderer.show_reasoning { "on" } else { "off" }
                            );
                        }
                        (other, _) => println!("sa: unknown command /{other}, try /help"),
                    }
                    continue;
                }
                if turn_running {
                    println!("\x1b[2m[busy: /cancel or ctrl-c to stop the turn]\x1b[0m");
                    continue;
                }
                client
                    .call(
                        "turn.start",
                        json!({
                            "session_id": session_id,
                            "prompt": text,
                            "detach": false,
                        }),
                    )
                    .await
                    .map_err(|e| format!("turn.start failed: {e}"))?;
                turn_running = true;
            }
            Incoming::Frame(frame) => {
                let method = frame
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match method {
                    "event" => {
                        let params = &frame["params"];
                        let kind = params
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        renderer.event(kind, &params["payload"]);
                        if kind == "turn.completed" || kind == "turn.interrupted" {
                            turn_running = false;
                            if quit_after_turn {
                                break;
                            }
                        }
                    }
                    "evicted" => {
                        println!("\n[evicted: another client took this session]");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = client
        .call("session.detach", json!({"session_id": session_id}))
        .await;
    Ok(())
}

// ---- export -------------------------------------------------------------

pub async fn export(session: Option<String>, format: &str) -> Result<()> {
    let mut client = connect().await?;
    let session_id = match session {
        Some(id) => id,
        None => most_recent_session(&mut client)
            .await?
            .ok_or_else(|| "no sessions to export".to_owned())?,
    };

    let replay = client
        .call(
            "events.replay",
            json!({"session_id": session_id, "from_seq": 0, "limit": 5000}),
        )
        .await
        .map_err(|e| format!("replay failed: {e}"))?;
    let events = replay["events"].as_array().cloned().unwrap_or_default();

    if format == "markdown" {
        println!("# session {session_id}\n");
        for event in &events {
            let kind = event["kind"].as_str().unwrap_or_default();
            let payload = &event["payload"];
            match kind {
                "turn.started" => {
                    println!(
                        "## you\n\n{}\n",
                        payload["prompt"].as_str().unwrap_or_default()
                    );
                }
                "llm.completed" => {
                    let text = payload["text"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        println!("## sa\n\n{text}\n");
                    }
                }
                "tool.requested" => {
                    println!("> tool: {}", payload["tool"].as_str().unwrap_or_default());
                }
                _ => {}
            }
        }
    } else {
        for event in &events {
            println!(
                "{}",
                serde_json::to_string(event).map_err(|e| e.to_string())?
            );
        }
    }
    Ok(())
}
