//! the user-facing commands: run, attach, export.

use std::io::{BufRead, Write};

use sa_client::Client;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

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

// ---- session helpers ----------------------------------------------------

async fn create_session(client: &mut Client) -> Result<String> {
    let response = client
        .call(
            "session.create",
            json!({"cwd": cwd()?, "autonomy": "allowlisted"}),
        )
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
    let Some(sessions) = response.as_array() else {
        return Ok(None);
    };
    let active = sessions
        .iter()
        .find(|s| s["state"].as_str() == Some("active"))
        .or_else(|| sessions.first());
    Ok(active.and_then(|s| s["id"].as_str()).map(str::to_owned))
}

/// prints an assistant text delta.
fn render_delta(payload: &Value) {
    if let Some(text) = payload.get("text").and_then(Value::as_str) {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

fn render_event(kind: &str, payload: &Value) {
    match kind {
        "llm.delta" => render_delta(payload),
        "llm.reasoning" => {}
        "turn.completed" => println!(),
        "turn.interrupted" => {
            let cause = payload
                .get("cause")
                .and_then(Value::as_str)
                .unwrap_or("interrupted");
            println!("\n[turn interrupted: {cause}]");
        }
        "tool.requested" => {
            let tool = payload
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            println!("\n  → {tool}");
        }
        "tool.failed" => {
            let error = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            println!("  ✗ {error}");
        }
        _ => {}
    }
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

    loop {
        let frame = client
            .next_notification()
            .await
            .map_err(|e| format!("connection lost: {e}"))?;
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            continue;
        };
        if method != "event" {
            continue;
        }
        let params = &frame["params"];
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        render_event(kind, &params["payload"]);
        if kind == "turn.completed" || kind == "turn.interrupted" {
            let matches = params["payload"]["turn_id"].as_str() == Some(turn_id.as_str());
            if matches || turn_id.is_empty() {
                println!();
                return Ok(());
            }
        }
    }
}

// ---- attach (interactive) ----------------------------------------------

enum Incoming {
    Line(String),
    Frame(Value),
}

pub async fn attach(session: Option<String>) -> Result<()> {
    let mut client = connect().await?;

    let session_id = match session {
        Some(id) => id,
        None => match most_recent_session(&mut client).await? {
            Some(id) => id,
            None => create_session(&mut client).await?,
        },
    };

    let attached = client
        .call(
            "session.attach",
            json!({"session_id": session_id, "from_seq": 0, "takeover": true}),
        )
        .await
        .map_err(|e| format!("attach failed: {e}"))?;

    if let Some(events) = attached["events"].as_array() {
        for event in events {
            let kind = event["kind"].as_str().unwrap_or_default();
            render_event(kind, &event["payload"]);
        }
    }

    client
        .call(
            "events.subscribe",
            json!({"session_id": session_id, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    println!(
        "sa: attached to {session_id}. type a prompt, `exit` to leave, `!cancel` to stop a turn."
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

    let mut pending_approval: Option<String> = None;
    let mut turn_running = false;
    // leaving mid-turn would truncate the transcript, so `exit` waits for the turn.
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
        };

        match incoming {
            Incoming::Line(line) => {
                let text = line.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(approval_id) = pending_approval.take() {
                    let allow = matches!(text, "y" | "yes" | "Y" | "YES");
                    client
                        .call(
                            "approval.respond",
                            json!({
                                "approval_id": approval_id,
                                "decision": if allow { "allow" } else { "deny" },
                                "scope": "once",
                            }),
                        )
                        .await
                        .map_err(|e| format!("approval.respond failed: {e}"))?;
                    continue;
                }
                match text {
                    "exit" | "quit" | ":q" => {
                        if turn_running {
                            println!("[leaving after the current turn]");
                            quit_after_turn = true;
                        } else {
                            break;
                        }
                    }
                    "!cancel" => {
                        client
                            .call("turn.cancel", json!({"session_id": session_id}))
                            .await
                            .map_err(|e| format!("turn.cancel failed: {e}"))?;
                    }
                    prompt => {
                        if turn_running {
                            println!("[busy: waiting for the current turn]");
                            continue;
                        }
                        client
                            .call(
                                "turn.start",
                                json!({
                                    "session_id": session_id,
                                    "prompt": prompt,
                                    "detach": false,
                                }),
                            )
                            .await
                            .map_err(|e| format!("turn.start failed: {e}"))?;
                        turn_running = true;
                    }
                }
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
                        render_event(kind, &params["payload"]);
                        if kind == "turn.completed" || kind == "turn.interrupted" {
                            turn_running = false;
                            if quit_after_turn {
                                break;
                            }
                            print!("sa> ");
                            let _ = std::io::stdout().flush();
                        }
                    }
                    "approval.request" => {
                        let params = &frame["params"];
                        let detail = params
                            .get("detail")
                            .and_then(Value::as_str)
                            .unwrap_or("operation");
                        println!("\n[approval] {detail}  (y/n)");
                        pending_approval = params
                            .get("approval_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
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

    match format {
        "markdown" => {
            println!("# session {session_id}\n");
            for event in &events {
                let kind = event["kind"].as_str().unwrap_or_default();
                let payload = &event["payload"];
                match kind {
                    "turn.started" => {
                        let prompt = payload["prompt"].as_str().unwrap_or_default();
                        println!("## you\n\n{prompt}\n");
                    }
                    "llm.completed" => {
                        let text = payload["text"].as_str().unwrap_or_default();
                        if !text.is_empty() {
                            println!("## sa\n\n{text}\n");
                        }
                    }
                    "tool.requested" => {
                        let tool = payload["tool"].as_str().unwrap_or_default();
                        println!("> tool: {tool}");
                    }
                    _ => {}
                }
            }
        }
        _ => {
            for event in &events {
                println!(
                    "{}",
                    serde_json::to_string(event).map_err(|e| e.to_string())?
                );
            }
        }
    }
    Ok(())
}
