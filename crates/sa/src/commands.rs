//! the user-facing commands: run, attach, export, sessions (spec 14, v1.1).

use std::io::{BufRead, IsTerminal, Write};

use sa_client::Client;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

/// how much of a tool result is shown before it is elided.
const TOOL_PREVIEW_LINES: usize = 8;
/// the most characters one previewed line may occupy, so a single long line cannot flood
/// the terminal.
const TOOL_PREVIEW_CHARS: usize = 160;

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

// ---- rendering ----------------------------------------------------------

/// renders a session transcript for a terminal.
#[derive(Default)]
struct Renderer {
    show_reasoning: bool,
}

impl Renderer {
    fn event(&self, kind: &str, payload: &Value) {
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
            "tool.requested" => Self::tool_requested(payload),
            "tool.completed" => Self::tool_completed(payload),
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

    fn tool_requested(payload: &Value) {
        let tool = payload
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        println!(
            "\n  \x1b[36m→ {tool}\x1b[0m {}",
            summarise_args(&payload["args"])
        );
    }

    fn tool_completed(payload: &Value) {
        let output = payload.get("output").and_then(Value::as_str).unwrap_or("");
        for line in preview(output) {
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
fn preview(output: &str) -> Vec<String> {
    if output.trim().is_empty() {
        return vec!["(no output)".to_owned()];
    }
    let mut lines: Vec<String> = output
        .lines()
        .take(TOOL_PREVIEW_LINES)
        .map(|line| truncate_chars(line, TOOL_PREVIEW_CHARS))
        .collect();
    let total = output.lines().count();
    if total > TOOL_PREVIEW_LINES {
        lines.push(format!(
            "\x1b[2m… {total} lines, showing {TOOL_PREVIEW_LINES}\x1b[0m"
        ));
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

/// the active session already rooted at this directory, so opening `sa` where a session
/// is running returns to it instead of starting a second one over the same files.
async fn session_for_cwd(client: &mut Client) -> Result<Option<String>> {
    let here = std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let here = std::fs::canonicalize(&here).unwrap_or(here);
    let response = client
        .call("session.list", json!({}))
        .await
        .map_err(|e| format!("session.list failed: {e}"))?;
    Ok(response
        .as_array()
        .and_then(|sessions| {
            sessions.iter().find(|session| {
                session["state"] == "active"
                    && session["cwd"]
                        .as_str()
                        .is_some_and(|cwd| std::path::Path::new(cwd) == here)
            })
        })
        .and_then(|session| session["id"].as_str())
        .map(str::to_owned))
}

fn render_history(renderer: &Renderer, events: &[Value]) {
    for event in events {
        let kind = event["kind"].as_str().unwrap_or_default();
        // history is replayed as a flat transcript, not a live stream.
        if kind == "llm.delta" {
            continue;
        }
        renderer.event(kind, &event["payload"]);
    }
}

// ---- context ------------------------------------------------------------

/// shows what pi-contexts material would be injected for a directory.
pub fn context(dir: Option<String>) -> Result<()> {
    let workspace = match dir {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?,
    };
    let cwd = std::env::current_dir().ok();
    let roots = sa_core::workspace::context_roots(&workspace, cwd.as_deref());

    println!("roots, outermost scope first (each scope reads its deployed config, then the directory itself):");
    for root in &roots {
        println!("  {}", root.display());
    }

    let files = sa_core::workspace::context_files(&workspace, cwd.as_deref());
    if files.is_empty() {
        println!("\nno context files found");
    } else {
        println!("\ncontext files, outermost scope first:");
        for file in &files {
            println!(
                "  {:<18} {} ({} bytes)",
                file.name,
                file.path.display(),
                file.body.len()
            );
        }
    }

    let skills = sa_core::workspace::skills(&workspace, cwd.as_deref());
    if !skills.is_empty() {
        println!("\nskills:");
        for skill in &skills {
            println!(
                "  {:<18} {} — {}",
                skill.name,
                skill.path.display(),
                skill.summary
            );
        }
    }
    Ok(())
}

// ---- sessions -----------------------------------------------------------

pub async fn sessions() -> Result<()> {
    let mut client = connect().await?;
    sessions_with(&mut client).await
}

/// as [`sessions`], over a client the caller already holds, so a test can drive it against
/// a scripted daemon instead of a real one.
pub async fn sessions_with(client: &mut Client) -> Result<()> {
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
    println!("{:<38} {:<10} {:<8} TITLE", "ID", "STATE", "EFFORT");
    for session in sessions {
        println!(
            "{:<38} {:<10} {:<8} {}",
            session["id"].as_str().unwrap_or("-"),
            session["state"].as_str().unwrap_or("-"),
            session["effort"].as_str().unwrap_or("-"),
            session["title"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

// ---- lsp ----------------------------------------------------------------

pub async fn lsp() -> Result<()> {
    let mut client = connect().await?;
    lsp_with(&mut client).await
}

/// as [`lsp`], over a client the caller already holds.
pub async fn lsp_with(client: &mut Client) -> Result<()> {
    let response = client
        .call("lsp.list", json!({}))
        .await
        .map_err(|e| format!("lsp.list failed: {e}"))?;
    let Some(servers) = response["servers"].as_array() else {
        println!("no language servers configured");
        return Ok(());
    };
    if servers.is_empty() {
        println!("no language servers configured");
        return Ok(());
    }
    for server in servers {
        let name = server["name"].as_str().unwrap_or("-");
        let state = match (
            server["installed"].as_bool().unwrap_or(false),
            server["running"].as_bool().unwrap_or(false),
        ) {
            (_, true) => "running",
            (true, false) => "installed",
            (false, false) => "missing",
        };
        println!("{name:<28} {state}");
    }
    println!(
        "\nthe table is {}; an entry there is merged over the built-in servers.",
        sa_core::lsp::config_path().display()
    );
    let searched = response["search_path"]
        .as_array()
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_default();
    println!("searched: {searched}");
    Ok(())
}

// ---- mcp ----------------------------------------------------------------

pub async fn mcp() -> Result<()> {
    let mut client = connect().await?;
    mcp_with(&mut client).await
}

/// as [`mcp`], over a client the caller already holds.
pub async fn mcp_with(client: &mut Client) -> Result<()> {
    let response = client
        .call("mcp.list", json!({}))
        .await
        .map_err(|e| format!("mcp.list failed: {e}"))?;
    let Some(servers) = response.as_array() else {
        println!("no mcp servers configured");
        return Ok(());
    };
    if servers.is_empty() {
        println!(
            "no mcp servers configured in {}",
            sa_core::mcp::config_path().display()
        );
        return Ok(());
    }
    for server in servers {
        let name = server["name"].as_str().unwrap_or("-");
        let transport = server["transport"].as_str().unwrap_or("-");
        if let Some(error) = server["error"].as_str() {
            println!("{name:<20} {transport:<6} unavailable: {error}");
        } else {
            let tools = server["tools"].as_array().map_or(0, Vec::len);
            println!("{name:<20} {transport:<6} {tools} tools");
            for tool in server["tools"].as_array().into_iter().flatten() {
                if let Some(tool) = tool.as_str() {
                    println!("    {tool}");
                }
            }
        }
    }
    Ok(())
}

// ---- run ----------------------------------------------------------------

pub async fn run_once(prompt: &str) -> Result<()> {
    let mut client = connect().await?;
    run_once_with(&mut client, prompt).await
}

/// as [`run_once`], over a client the caller already holds.
pub async fn run_once_with(client: &mut Client, prompt: &str) -> Result<()> {
    let session = create_session(client).await?;

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

    let renderer = Renderer::default();
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
        "\x1b[2mcommands: /cancel /cost /exit /help /model <id> /new /reasoning \
         /sessions\n\
         ctrl-c cancels the running turn; at an idle prompt it exits.\x1b[0m"
    );
}

// the tui it hands off to holds an `Rc` editor clipboard, so this future is not `Send`.
// it is awaited by `main` directly and never spawned.
#[allow(clippy::future_not_send)]
pub async fn attach(session: Option<String>, new: bool, line: bool) -> Result<()> {
    let mut client = connect().await?;

    // a terminal gets the whole window: every session is a tab in it, so there is nothing
    // to choose here beyond which tab to land on.
    if !line && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return sa_tui::run(&mut client, session, new).await;
    }

    // a pipe gets plain lines for one session, which is also what the tests drive.
    let mut session_id = if new {
        create_session(&mut client).await?
    } else if let Some(id) = session {
        id
    } else {
        match session_for_cwd(&mut client).await? {
            Some(id) => id,
            None => create_session(&mut client).await?,
        }
    };

    let attached = client
        .call(
            "session.attach",
            json!({"session_id": session_id, "from_seq": 0}),
        )
        .await
        .map_err(|e| format!("attach failed: {e}"))?;

    let mut renderer = Renderer::default();
    if let Some(events) = attached["events"].as_array() {
        render_history(&renderer, events);
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
                    // a prompt sent while a turn runs redirects it rather than being
                    // refused (spec 3.7): the operator is not made to wait.
                    client
                        .call(
                            "turn.steer",
                            json!({"session_id": session_id, "prompt": text}),
                        )
                        .await
                        .map_err(|e| format!("turn.steer failed: {e}"))?;
                    println!("\x1b[2m[steering the running turn]\x1b[0m");
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

    Ok(())
}

// ---- export -------------------------------------------------------------

/// how many events one replay page asks for.
const REPLAY_PAGE: usize = 5000;

/// every event of a session, page by page. a long session runs past any single replay
/// limit, and exporting the first page of it is worse than not exporting it at all.
async fn replay_all(client: &mut Client, session_id: &str, mut page: Value) -> Result<Vec<Value>> {
    let mut events: Vec<Value> = Vec::new();
    let mut from_seq = 0;
    loop {
        events.extend(page["events"].as_array().cloned().unwrap_or_default());
        if page["more"].as_bool() != Some(true) {
            return Ok(events);
        }
        let next = page["next_seq"].as_u64().unwrap_or(from_seq);
        if next <= from_seq {
            return Ok(events);
        }
        from_seq = next;
        page = client
            .call(
                "events.replay",
                json!({"session_id": session_id, "from_seq": from_seq, "limit": REPLAY_PAGE}),
            )
            .await
            .map_err(|e| format!("replay failed: {e}"))?;
    }
}

pub async fn export(session: Option<String>, format: &str) -> Result<()> {
    let mut client = connect().await?;
    export_with(&mut client, session, format).await
}

/// as [`export`], over a client the caller already holds.
pub async fn export_with(client: &mut Client, session: Option<String>, format: &str) -> Result<()> {
    let session_id = match session {
        Some(id) => id,
        None => most_recent_session(client)
            .await?
            .ok_or_else(|| "no sessions to export".to_owned())?,
    };

    let replay = client
        .call(
            "events.replay",
            json!({"session_id": session_id, "from_seq": 0, "limit": REPLAY_PAGE}),
        )
        .await
        .map_err(|e| format!("replay failed: {e}"))?;
    let events = replay_all(client, &session_id, replay).await?;

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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    /// a scripted daemon on the far end of a socketpair, so the client half of a command
    /// can be exercised without a real daemon.
    fn daemon(
        replies: Vec<(&'static str, Value)>,
    ) -> (Client, tokio::task::JoinHandle<Vec<Value>>) {
        daemon_notifying(replies, Vec::new())
    }

    /// as [`daemon`], but also pushes a notification after answering the named method.
    fn daemon_notifying(
        replies: Vec<(&'static str, Value)>,
        notify: Vec<(&'static str, Value)>,
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
                // some methods end a turn, so the client is waiting for this.
                if let Some((_, params)) = notify.iter().find(|(name, _)| *name == method) {
                    let pushed = json!({"jsonrpc": "2.0", "method": "event", "params": params});
                    if write
                        .write_all(format!("{pushed}\n").as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            seen
        });
        (Client::from_stream(ours), handle)
    }

    /// as [`daemon`], but answers in the order it was given, so a method called twice can
    /// be answered differently the second time.
    fn daemon_ordered(
        replies: Vec<(&'static str, Value)>,
    ) -> (Client, tokio::task::JoinHandle<Vec<Value>>) {
        let (ours, theirs) = UnixStream::pair().expect("socketpair");
        let handle = tokio::spawn(async move {
            let (read, mut write) = theirs.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut seen = Vec::new();
            let mut served = 0usize;
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                seen.push(request.clone());
                let result = replies
                    .get(served)
                    .map_or_else(|| json!({}), |(_, value)| value.clone());
                served += 1;
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

    #[test]
    fn tool_arguments_summarise_to_their_most_useful_field() {
        assert_eq!(
            summarise_args(&json!({"path": "src/main.rs"})),
            "src/main.rs"
        );
        assert_eq!(summarise_args(&json!({"command": "ls -la"})), "ls -la");
        assert_eq!(summarise_args(&json!({"pattern": "fn main"})), "fn main");
        assert_eq!(
            summarise_args(&json!({"content": "abcdef"})),
            "6 bytes",
            "a write reports its size"
        );
        assert_eq!(summarise_args(&json!({})), "");
    }

    #[test]
    fn long_values_are_cut_with_a_marker() {
        assert_eq!(truncate_chars("short", 10), "short");
        assert_eq!(truncate_chars("abcdefghij", 5), "abcde\u{2026}");
    }

    #[test]
    fn a_tool_result_preview_is_bounded_and_says_how_much_it_hid() {
        let many = (0..20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let shown = preview(&many);
        assert_eq!(shown.len(), TOOL_PREVIEW_LINES + 1);
        assert!(shown.last().expect("tail").contains("20 lines"));

        assert_eq!(preview(""), vec!["(no output)".to_owned()]);
        assert_eq!(preview("one line"), vec!["one line".to_owned()]);
    }

    #[test]
    fn the_renderer_takes_every_event_kind() {
        let renderer = Renderer::default();
        // every kind must render without panicking, including one it does not know.
        for kind in [
            "compaction.applied",
            "llm.completed",
            "llm.delta",
            "llm.reasoning",
            "not.a.kind",
            "tool.completed",
            "tool.failed",
            "tool.requested",
            "turn.completed",
            "turn.interrupted",
        ] {
            renderer.event(
                kind,
                &json!({
                    "text": "x",
                    "call_id": "c1",
                    "tool": "bash",
                    "args": {"command": "ls"},
                    "output": "out",
                    "cause": "cause",
                    "usage": {"prompt_tokens": 1, "completion_tokens": 2},
                }),
            );
        }
    }

    #[test]
    fn history_replay_skips_the_live_deltas() {
        let renderer = Renderer::default();
        let events = vec![
            json!({"kind": "llm.delta", "payload": {"text": "streamed"}}),
            json!({"kind": "turn.completed", "payload": {"usage": {}}}),
        ];
        render_history(&renderer, &events);
    }

    #[tokio::test]
    async fn the_session_for_this_directory_is_found_by_its_cwd() {
        let here = std::env::current_dir().expect("cwd");
        let here = std::fs::canonicalize(&here).unwrap_or(here);
        let elsewhere = "/somewhere/else";
        let list = json!([
            {"id": "other", "state": "active", "cwd": elsewhere},
            {"id": "mine", "state": "active", "cwd": here.display().to_string()},
            {"id": "deleted", "state": "deleted", "cwd": here.display().to_string()},
        ]);
        let (mut client, _server) = daemon(vec![("session.list", list)]);

        assert_eq!(
            session_for_cwd(&mut client).await.expect("list"),
            Some("mine".to_owned()),
            "the live session rooted here, not another directory's"
        );
    }

    #[tokio::test]
    async fn the_most_recent_session_is_the_first_listed() {
        let list = json!([{"id": "newest"}, {"id": "older"}]);
        let (mut client, _server) = daemon(vec![("session.list", list)]);
        assert_eq!(
            most_recent_session(&mut client).await.expect("list"),
            Some("newest".to_owned())
        );

        let (mut client, _server) = daemon(vec![("session.list", json!([]))]);
        assert_eq!(most_recent_session(&mut client).await.expect("list"), None);
    }

    #[tokio::test]
    async fn creating_a_session_reports_the_id_or_the_failure() {
        let (mut client, _server) = daemon(vec![("session.create", json!({"session_id": "abc"}))]);
        assert_eq!(create_session(&mut client).await, Ok("abc".to_owned()));

        let (mut client, _server) = daemon(vec![("session.create", json!({}))]);
        assert!(create_session(&mut client).await.is_err());
    }

    #[test]
    fn the_context_command_reads_a_directory_without_a_daemon() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("AGENTS.md"), "be brief").expect("write");
        std::fs::create_dir_all(dir.path().join("skills/demo")).expect("dirs");
        std::fs::write(
            dir.path().join("skills/demo/SKILL.md"),
            "---\nname: demo\n---\nbody",
        )
        .expect("write");

        context(Some(dir.path().display().to_string())).expect("context");
    }

    #[tokio::test]
    async fn listing_sessions_works_against_a_held_client() {
        let list = json!([{"id": "one", "state": "active", "title": "t"}]);
        let (mut client, _server) = daemon(vec![("session.list", list)]);
        sessions_with(&mut client).await.expect("sessions");

        let (mut client, _server) = daemon(vec![("session.list", json!([]))]);
        sessions_with(&mut client)
            .await
            .expect("an empty list is fine");
    }

    #[tokio::test]
    async fn running_a_prompt_drives_the_turn_to_its_end() {
        let done = json!({
            "session_id": "s1",
            "kind": "turn.completed",
            "payload": {"turn_id": "t1", "usage": {"prompt_tokens": 1, "completion_tokens": 2}},
        });
        let (mut client, server) = daemon_notifying(
            vec![
                ("session.create", json!({"session_id": "s1"})),
                ("events.subscribe", json!({"subscription_id": 1})),
                ("turn.start", json!({"turn_id": "t1", "seq": 1})),
            ],
            vec![("turn.start", done)],
        );

        run_once_with(&mut client, "say hello").await.expect("run");

        drop(client);
        let seen = server.await.expect("server");
        let started = seen
            .iter()
            .find(|r| r["method"] == "turn.start")
            .expect("turn.start");
        assert_eq!(started["params"]["prompt"], "say hello");
        assert_eq!(
            started["params"]["detach"], true,
            "a one-shot turn is detached"
        );
    }

    #[tokio::test]
    async fn exporting_writes_the_session_in_either_format() {
        let replay = json!({"events": [
            {"kind": "turn.started", "payload": {"prompt": "hello"}},
            {"kind": "llm.completed", "payload": {"text": "hi"}},
            {"kind": "tool.requested", "payload": {"tool": "bash"}},
        ]});
        for format in ["markdown", "jsonl"] {
            let (mut client, _server) = daemon(vec![
                ("session.list", json!([{"id": "s1"}])),
                ("events.replay", replay.clone()),
            ]);
            export_with(&mut client, None, format)
                .await
                .expect("export");
            export_with(&mut client, Some("s1".to_owned()), format)
                .await
                .expect("export by id");
        }
    }
    #[tokio::test]
    async fn an_export_reads_every_page_of_a_long_session() {
        // the regression: export asked for one page of 5000 and ignored `more`, so a long
        // session was exported truncated with nothing to say it had been.
        let (mut client, server) = daemon_ordered(vec![
            (
                "events.replay",
                json!({
                    "events": [{"kind": "turn.started", "payload": {"prompt": "one"}}],
                    "more": true,
                    "next_seq": 1,
                }),
            ),
            (
                "events.replay",
                json!({
                    "events": [{"kind": "llm.completed", "payload": {"text": "two"}}],
                    "more": false,
                    "next_seq": 2,
                }),
            ),
        ]);

        export_with(&mut client, Some("s1".to_owned()), "jsonl")
            .await
            .expect("export");

        drop(client);
        let seen = server.await.expect("server");
        let replays: Vec<&Value> = seen
            .iter()
            .filter(|request| request["method"] == "events.replay")
            .collect();
        assert_eq!(replays.len(), 2, "the second page must be fetched");
        assert_eq!(
            replays[1]["params"]["from_seq"], 1,
            "the next page continues from where the last one stopped"
        );
    }

    #[tokio::test]
    async fn listing_language_servers_reports_their_state() {
        let list = json!({"servers": [
            {"name": "pyright", "installed": true, "running": true},
            {"name": "zls", "installed": false, "running": false},
        ], "search_path": ["/usr/bin"]});
        let (mut client, _server) = daemon(vec![("lsp.list", list)]);
        lsp_with(&mut client).await.expect("servers");

        // no servers is not an error.
        let (mut client, _server) = daemon(vec![("lsp.list", json!({"servers": []}))]);
        lsp_with(&mut client).await.expect("empty");
    }

    #[tokio::test]
    async fn listing_mcp_servers_reports_their_state_and_tools() {
        let list = json!([
            {"name": "filesystem", "transport": "stdio", "tools": ["mcp__filesystem__read_file"], "error": null},
            {"name": "broken", "transport": "http", "tools": [], "error": "connection refused"},
        ]);
        let (mut client, _server) = daemon(vec![("mcp.list", list)]);
        mcp_with(&mut client).await.expect("servers");

        // no servers is not an error.
        let (mut client, _server) = daemon(vec![("mcp.list", json!([]))]);
        mcp_with(&mut client).await.expect("empty");
    }
}
