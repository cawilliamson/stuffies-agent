//! tool execution (spec 9).
//!
//! tools run without asking. the only guard is that a path must resolve inside the
//! workspace.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use command_group::AsyncCommandGroup;

use crate::agent::{BoxFuture, ToolExecutor, ToolOutcome};
use crate::llm::{ToolCall, ToolSpec};

/// tool output above this is capped, with `truncated: true` recorded in the event.
pub const MAX_TOOL_OUTPUT: usize = 256 * 1024;

/// default command timeout (spec 9).
pub const COMMAND_TIMEOUT_SECS: u64 = 600;

/// how many lines a read returns when the caller does not say.
pub const READ_LINES: usize = 2000;

/// the most lines a read returns however many are asked for.
pub const MAX_READ_LINES: usize = 5000;

/// the most bytes one read returns, so a file of enormous lines cannot flood the context.
pub const MAX_READ_BYTES: usize = 50 * 1024;

/// how many lines a search walks before it gives up.
pub const MAX_WALKED: usize = 5000;

/// how many matches a search returns.
pub const MAX_MATCHES: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("path escapes the workspace: {0}")]
    Escape(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad arguments: {0}")]
    Args(String),
    #[error("unknown tool: {0}")]
    Unknown(String),
}

pub struct ToolRegistry {
    workspace: PathBuf,
    /// the mcp servers this daemon reached, if any. their tools are the model's too.
    mcp: Option<std::sync::Arc<crate::mcp::Hub>>,
    /// the structural lens for this session, when one is wired in: the read-before-edit
    /// guard and the workspace's own rules.
    lens: Option<std::sync::Arc<crate::lens::Lens>>,
}

impl ToolRegistry {
    #[must_use]
    pub const fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            mcp: None,
            lens: None,
        }
    }

    /// a registry that also offers the tools of every connected mcp server.
    #[must_use]
    pub const fn with_mcp(workspace: PathBuf, mcp: std::sync::Arc<crate::mcp::Hub>) -> Self {
        Self {
            workspace,
            mcp: Some(mcp),
            lens: None,
        }
    }

    /// a registry with the session's lens: the guard that refuses a blind edit, and the
    /// structural check a written file is run through.
    #[must_use]
    pub const fn with_lens(
        workspace: PathBuf,
        mcp: std::sync::Arc<crate::mcp::Hub>,
        lens: std::sync::Arc<crate::lens::Lens>,
    ) -> Self {
        Self {
            workspace,
            mcp: Some(mcp),
            lens: Some(lens),
        }
    }

    /// canonicalises a caller-supplied path and refuses anything outside the workspace.
    fn resolve(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let joined = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            self.workspace.join(raw)
        };
        let normalised = normalise(&joined);
        let root = normalise(&self.workspace);
        if !normalised.starts_with(&root) {
            return Err(ToolError::Escape(normalised.display().to_string()));
        }
        Ok(normalised)
    }

    async fn call(&self, call: &ToolCall) -> Result<ToolOutcome, ToolError> {
        let args = parse_args(&call.arguments)?;
        // a server tool is namespaced, so it can never collide with a built-in.
        if call.name.starts_with(crate::mcp::TOOL_PREFIX) {
            let Some(hub) = &self.mcp else {
                return Err(ToolError::Unknown(call.name.clone()));
            };
            let output = hub.call(&call.name, &args).await.map_err(ToolError::Args)?;
            return Ok(text(output));
        }
        match call.name.as_str() {
            "bash" => {
                self.bash_streaming(&args, &call.id, &crate::agent::NoOutput)
                    .await
            }
            "edit" => self.edit(&args).await.map(text),
            "find" => self.find(&args).await.map(text),
            "grep" => self.grep(&args).await.map(text),
            "outline" => self.outline(&args).await.map(text),
            "read" => self.read(&args).await.map(text),
            "web_fetch" => crate::web::fetch(required(&args, "url")?).await.map(text),
            "web_search" => {
                let query = required(&args, "query")?;
                crate::web::search(query, crate::web::limit_from(&args))
                    .await
                    .map(text)
            }
            "write" => self.write(&args).await.map(text),
            other => Err(ToolError::Unknown(other.to_owned())),
        }
    }

    async fn read(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.resolve(required(args, "path")?)?;
        let body = tokio::fs::read_to_string(&path).await?;
        // a read is what licenses an edit, so it is recorded here rather than at the call
        // site that happened to read.
        if let Some(lens) = &self.lens {
            lens.guard.record(&path);
        }
        // the hashed form is what makes a line addressable; the raw text is one flag away.
        let hashed = args
            .get("hashline")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(1)
            .max(1);
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(READ_LINES)
            .clamp(1, MAX_READ_LINES);
        Ok(window(&body, offset, limit, hashed))
    }

    /// the declarations a file makes, so the model reads a range instead of a whole file.
    async fn outline(&self, args: &Value) -> Result<String, ToolError> {
        let raw = required(args, "path")?;
        let path = self.resolve(raw)?;
        let body = tokio::fs::read_to_string(&path).await?;
        if let Some(lens) = &self.lens {
            lens.guard.record(&path);
        }
        let symbols = crate::lens::outline(&path, &body);
        if symbols.is_empty() {
            return Ok(format!(
                "{raw}: no declarations ({})",
                crate::lens::language_of(&path).unwrap_or_else(|| "unparsed".to_owned())
            ));
        }
        let mut out = format!("{raw}: {} declaration(s)\n", symbols.len());
        for symbol in symbols {
            let _ = writeln!(
                out,
                "{}{} {}-{} {} ({})",
                "  ".repeat(symbol.depth),
                symbol.kind,
                symbol.start,
                symbol.end,
                symbol.name,
                symbol.end - symbol.start + 1
            );
        }
        Ok(out)
    }

    /// refuses an edit to a file the model has not read, and checks what it wrote.
    fn guard_edit(&self, path: &Path, raw: &str) -> Result<(), ToolError> {
        let Some(lens) = &self.lens else {
            return Ok(());
        };
        if lens.guard.may_edit(path) {
            return Ok(());
        }
        Err(ToolError::Args(format!(
            "{raw} has not been read in this session; read it first so the edit is made to \
             the text that is really there"
        )))
    }

    /// runs the workspace's rules over a file that was just written, and appends what they
    /// found to the tool's own result.
    fn check(&self, path: &Path, raw: &str, body: &str) -> String {
        let Some(lens) = &self.lens else {
            return String::new();
        };
        let findings = lens.rules.scan(path, body);
        lens.rules.report(raw, &findings)
    }

    async fn write(&self, args: &Value) -> Result<String, ToolError> {
        let raw = required(args, "path")?;
        let path = self.resolve(raw)?;
        self.guard_edit(&path, raw)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // what the file held, so replacing it is a change that can be read rather than a
        // claim that something was written.
        let before = tokio::fs::read_to_string(&path).await.ok();
        let body = required(args, "content")?;
        tokio::fs::write(&path, body).await?;
        // what was written is what the model has now seen, so the next edit needs no read.
        if let Some(lens) = &self.lens {
            lens.guard.record(&path);
        }
        let report = self.check(&path, raw, body);
        let change = before.map_or_else(String::new, |before| {
            format!("\n\n{}", diff(&before, body, raw))
        });
        Ok(format!(
            "wrote {} bytes to {raw}{change}{report}",
            body.len()
        ))
    }

    async fn edit(&self, args: &Value) -> Result<String, ToolError> {
        let raw = required(args, "path")?;
        let path = self.resolve(raw)?;
        // an edit is made to text the model has seen, or it is made to a guess.
        self.guard_edit(&path, raw)?;
        let body = tokio::fs::read_to_string(&path).await?;
        // the hashline form: each edit names the line by the hash of its own text.
        let (edited, changed) = if let Some(Value::Array(items)) = args.get("edits") {
            let edits = parse_hashline_edits(items)?;
            let applied = crate::hashline::apply(&body, &edits).map_err(|failure| {
                // a refusal carries the lines to retry with, so the model does not have to
                // guess what the file holds now.
                ToolError::Args(format!(
                    "{raw}: {}",
                    crate::hashline::guidance(&body, &failure)
                ))
            })?;
            (applied.source, applied.changed)
        } else {
            let old = required(args, "old")?;
            let new = required(args, "new")?;
            // a model that copied the hashed line back still matches the file: the prefix
            // is addressing, not content.
            let old = crate::hashline::strip_hashed(old);
            let count = body.matches(old.as_str()).count();
            if count != 1 {
                return Err(ToolError::Args(format!(
                    "{raw}: expected exactly one match, found {count}{}",
                    nearby(&body, &old)
                )));
            }
            (body.replacen(old.as_str(), new, 1), Vec::new())
        };
        tokio::fs::write(&path, &edited).await?;
        if let Some(lens) = &self.lens {
            lens.guard.record(&path);
        }
        let report = self.check(&path, raw, &edited);
        // the change itself, in place: a model that has just edited a file should see what
        // it did rather than a byte count it has to take on trust.
        let change = diff(&body, &edited, raw);
        // and the hashes of what it wrote, so the next edit needs no read.
        let fresh = changed.first().map_or_else(String::new, |line| {
            format!(
                "\n\nhashes as written — reuse them for the next edit:\n{}",
                crate::hashline::window(&edited, *line, 2)
            )
        });
        Ok(format!("edited {raw}\n\n{change}{fresh}{report}"))
    }

    async fn find(&self, args: &Value) -> Result<String, ToolError> {
        let pattern = required(args, "pattern")?.to_owned();
        let root = self.workspace.clone();
        let matches = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walk(&root).filter_map(Result::ok).take(MAX_WALKED) {
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(&pattern) {
                    let path = entry.path();
                    out.push(
                        path.strip_prefix(root.as_path())
                            .unwrap_or(path)
                            .display()
                            .to_string(),
                    );
                    if out.len() >= MAX_MATCHES {
                        break;
                    }
                }
            }
            out.sort();
            out
        })
        .await
        .map_err(|e| ToolError::Args(e.to_string()))?;
        Ok(matches.join("\n"))
    }

    async fn grep(&self, args: &Value) -> Result<String, ToolError> {
        let pattern = required(args, "pattern")?.to_owned();
        let regex =
            regex::Regex::new(&pattern).map_err(|e| ToolError::Args(format!("bad regex: {e}")))?;
        let root = self.workspace.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walk(&root).filter_map(Result::ok).take(MAX_WALKED) {
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                for (number, line) in body.lines().enumerate() {
                    if regex.is_match(line) {
                        let path = entry.path();
                        let shown = path
                            .strip_prefix(&root)
                            .unwrap_or(path)
                            .display()
                            .to_string();
                        out.push(format!("{shown}:{}:{}", number + 1, line.trim_end()));
                        if out.len() >= MAX_MATCHES {
                            return out;
                        }
                    }
                }
            }
            out
        })
        .await
        .map_err(|e| ToolError::Args(e.to_string()))?;
        Ok(hits.join("\n"))
    }

    /// runs a command in its own process group, reporting its output as it arrives.
    ///
    /// the group is what gets killed, so a `sleep 100 &` cannot outlive the shell that
    /// started it, and a cancelled turn takes the whole tree with it.
    async fn bash_streaming(
        &self,
        args: &Value,
        call_id: &str,
        sink: &dyn crate::agent::OutputSink,
    ) -> Result<ToolOutcome, ToolError> {
        let command = required(args, "command")?.to_owned();

        let mut process = tokio::process::Command::new("sh");
        process
            .arg("-c")
            .arg(&command)
            .current_dir(&self.workspace)
            // the environment is inherited from the running daemon, never rebuilt: a
            // hardcoded PATH is wrong on every machine that is not the one it was set on.
            .stdin(std::process::Stdio::null())
            // group_spawn inherits the parent's streams unless they are asked for.
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = process
            .group_spawn()
            .map_err(|e| ToolError::Args(e.to_string()))?;
        let mut killer = KillGroup(child.id().and_then(|id| i32::try_from(id).ok()));

        // both streams are read as they arrive: a build says what it is doing while it
        // does it, and waiting for the end makes a slow command look like a hung one.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        {
            let inner = child.inner();
            if let Some(stdout) = inner.stdout.take() {
                tokio::spawn(pump(stdout, tx.clone()));
            }
            if let Some(stderr) = inner.stderr.take() {
                tokio::spawn(pump(stderr, tx.clone()));
            }
        }
        drop(tx);

        let collected = async {
            let mut bytes: Vec<u8> = Vec::new();
            while let Some(chunk) = rx.recv().await {
                // the live view is best effort: a chunk that splits a character shows a
                // replacement for an instant. the recorded output is decoded whole below.
                sink.output(call_id, &String::from_utf8_lossy(&chunk));
                bytes.extend_from_slice(&chunk);
            }
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((String::from_utf8_lossy(&bytes).into_owned(), status))
        };

        let (output, status) = tokio::time::timeout(
            std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS),
            collected,
        )
        .await
        .map_err(|_| ToolError::Args(format!("timed out after {COMMAND_TIMEOUT_SECS}s")))??;
        killer.disarm();

        Ok(ToolOutcome {
            output,
            exit_code: status.code(),
        })
    }
}

impl ToolExecutor for ToolRegistry {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut tools = vec![
            spec(
                "bash",
                &format!(
                    "Run a shell command with `sh -c` in the workspace. Returns combined stdout \
                     and stderr plus the exit code. Times out after {COMMAND_TIMEOUT_SECS} seconds. \
                     Output is capped at {} KiB and marked when cut.",
                    MAX_TOOL_OUTPUT / 1024
                ),
                json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            ),
            spec(
                "edit",
                "Replace lines by the hash they carry: `edits: [{hash, content}]`, where hash \
                 is the `hhhhhh` prefix read returned. The hash names the text itself, so a \
                 line that has moved is still found and a line that has changed is refused. \
                 Add `line` to choose between identical lines. `old`/`new` also work. The \
                 file must have been read in this session first.",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "edits": {"type": "array", "items": {"type": "object"}}, "old": {"type": "string"}, "new": {"type": "string"}}, "required": ["path"]}),
            ),
            spec(
                "find",
                &format!(
                    "Find files whose name contains the pattern. Walks at most {MAX_WALKED} \
                     entries and returns at most {MAX_MATCHES} paths."
                ),
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
            spec(
                "grep",
                &format!(
                    "Search file contents with a regular expression. Walks at most {MAX_WALKED} \
                     files and returns at most {MAX_MATCHES} matches."
                ),
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
            spec(
                "outline",
                "List the declarations a file makes, with the line range of each. Read a range \
                 instead of the whole file.",
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            ),
            spec(
                "read",
                &format!(
                    "Read a file as text, each line prefixed with `hhhhhh|`: the hash names that \
                     line for `edit`. Pass `hashline: false` for the raw text. Returns at most \
                     {READ_LINES} lines or {} KiB; when there is more, the last line says the \
                     offset to continue from. Reading a file is what allows it to be edited.",
                    MAX_READ_BYTES / 1024
                ),
                json!({"type": "object", "properties": {"path": {"type": "string"}, "hashline": {"type": "boolean"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}}, "required": ["path"]}),
            ),
            spec(
                "web_fetch",
                "Fetch a web page through an emulated browser and return it as readable \
                 markdown. Only http and https are fetched; the page is capped at 4 MiB and \
                 marked when cut.",
                json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
            ),
            spec(
                "web_search",
                "Search the web and return a numbered list of results. Five by default.",
                json!({"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer"}}, "required": ["query"]}),
            ),
            spec(
                "write",
                "Write a file, creating it or replacing its contents entirely. An existing file \
                 must have been read in this session first.",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}),
            ),
        ];
        // every connected server's tools, named so they cannot shadow a built-in.
        if let Some(hub) = &self.mcp {
            tools.extend(hub.specs());
        }
        tools
    }

    fn execute(&self, call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
        Box::pin(async move { self.call(&call).await.map_err(|e| e.to_string()) })
    }

    fn execute_streaming<'a>(
        &'a self,
        call: ToolCall,
        sink: &'a dyn crate::agent::OutputSink,
    ) -> BoxFuture<'a, Result<ToolOutcome, String>> {
        Box::pin(async move {
            // only a tool that runs a process has anything to say before it finishes.
            if call.name == "bash" {
                let args = parse_args(&call.arguments).map_err(|e| e.to_string())?;
                return self
                    .bash_streaming(&args, &call.id, sink)
                    .await
                    .map_err(|e| e.to_string());
            }
            self.call(&call).await.map_err(|e| e.to_string())
        })
    }
}

/// wraps plain text output as a tool outcome.
const fn text(output: String) -> ToolOutcome {
    ToolOutcome {
        output,
        exit_code: None,
    }
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
    }
}

/// reads one of a child's streams into the live sink until the stream closes.
async fn pump<R>(mut reader: R, tx: tokio::sync::mpsc::Sender<Vec<u8>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut buf = vec![0_u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if tx.send(buf[..read].to_vec()).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// the files a search walks.
///
/// the project's own ignore files decide what is searched, so a coding agent sees the
/// files the developer sees rather than the build output. a dotfile is still source —
/// `.github/workflows/ci.yml` is searched — but the git directory itself never is.
fn walk(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
}

/// one hashline edit, read as leniently as the model writes it.
///
/// a model gets this wrong often enough that the shapes it is likely to reach for are all
/// accepted: the hash with its separator still attached, `new` for the replacement, and
/// `line_number` for the hint. anything genuinely missing is named.
fn parse_hashline_edits(items: &[Value]) -> Result<Vec<crate::hashline::Edit>, ToolError> {
    let mut edits = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .get("hash")
            .or_else(|| item.get("line_hash"))
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Args("each edit needs a hash".to_owned()))?;
        // `hhhhhh|content` copied whole still names the line.
        let hash = raw.split('|').next().unwrap_or(raw).trim().to_owned();
        if hash.is_empty() {
            return Err(ToolError::Args("each edit needs a hash".to_owned()));
        }
        let content = item
            .get("content")
            .or_else(|| item.get("new"))
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Args("each edit needs content".to_owned()))?;
        let line = item
            .get("line")
            .or_else(|| item.get("line_number"))
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok());
        edits.push(crate::hashline::Edit {
            hash,
            line,
            content: content.to_owned(),
        });
    }
    Ok(edits)
}

/// where the text a model quoted still is, when an `old`/`new` edit finds nothing.
///
/// an edit that matches nothing is the model quoting a file that has moved on. pointing at
/// the line its quote starts on turns a dead end into a hashline edit it can make instead.
fn nearby(body: &str, quoted: &str) -> String {
    let Some(anchor) = quoted.lines().find(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let Some(at) = body.lines().position(|line| line == anchor) else {
        return String::new();
    };
    format!(
        "\n\nwhat the file holds there now — edit by hash instead:\n{}",
        crate::hashline::window(body, at + 1, 4)
    )
}

/// one window of a file, and what it says about the rest of it.
///
/// a read is bounded on purpose: a model that wants more asks for the next window, which is
/// cheaper than paying for a whole long file to look at one part of it. the footer is what
/// makes that possible, because a cut a model cannot see is a cut it will edit blind.
fn window(source: &str, offset: usize, limit: usize, hashed: bool) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    let start = offset.saturating_sub(1).min(total);

    let mut body = String::new();
    let mut shown = 0_usize;
    let mut bytes = 0_usize;
    for line in lines.iter().skip(start).take(limit) {
        let cost = line.len() + 1 + if hashed { 7 } else { 0 };
        // the byte budget never splits a line: a half line is worse than one fewer.
        if shown > 0 && bytes + cost > MAX_READ_BYTES {
            break;
        }
        if hashed {
            let _ = writeln!(body, "{}|{line}", crate::hashline::hash_line(line));
        } else {
            let _ = writeln!(body, "{line}");
        }
        bytes += cost;
        shown += 1;
    }

    if shown == 0 {
        return format!(
            "(no lines at offset {offset}; the file has {total}){}",
            if total == 0 { " and is empty" } else { "" }
        );
    }
    let last = start + shown;
    if last >= total {
        return body;
    }
    let _ = write!(
        body,
        "\n… lines {offset}-{last} of {total}; continue with offset={}",
        last + 1
    );
    body
}

/// the change an edit made, as a unified diff with the context that shows it in place.
///
/// a diff is what makes an edit reviewable: the lines around it say whether the change
/// landed where it was meant to, which a byte count cannot.
fn diff(before: &str, after: &str, path: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

fn parse_args(raw: &str) -> Result<Value, ToolError> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| ToolError::Args(format!("bad json arguments: {e}")))
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Args(format!("missing argument: {key}")))
}

/// lexical normalisation: resolve `.` and `..` without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_collapses_dot_segments() {
        assert_eq!(normalise(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn path_escape_is_refused() {
        let registry = ToolRegistry::new(PathBuf::from("/workspace"));
        assert!(registry.resolve("sub/file.rs").is_ok());
        assert!(matches!(
            registry.resolve("../outside"),
            Err(ToolError::Escape(_))
        ));
        assert!(matches!(
            registry.resolve("/etc/passwd"),
            Err(ToolError::Escape(_))
        ));
    }
}

/// kills the whole process group when dropped, so a command that is abandoned by a
/// cancelled turn takes everything it started with it.
struct KillGroup(Option<i32>);

impl KillGroup {
    const fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for KillGroup {
    fn drop(&mut self) {
        if let Some(group) = self.0 {
            // the group may already be gone, which is not an error worth reporting.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod group_tests {
    use super::*;

    /// a command that backgrounds a child and waits, as a long build does.
    const BACKGROUNDED: &str = "sleep 30 & echo $! > ";

    #[tokio::test]
    async fn abandoning_a_command_kills_what_it_started() {
        let dir = std::env::temp_dir().join(format!("sa-group-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pid_file = dir.join("child.pid");
        let _ = std::fs::remove_file(&pid_file);

        let registry = std::sync::Arc::new(ToolRegistry::new(dir.clone()));
        let command = format!("{BACKGROUNDED}{}; wait", pid_file.display());
        let call = ToolCall {
            id: "c1".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({ "command": command }).to_string(),
        };
        let running = tokio::spawn({
            let registry = std::sync::Arc::clone(&registry);
            async move { registry.execute(call).await }
        });

        // let the shell start and write the grandchild's pid.
        for _ in 0..100 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("the grandchild must report its pid")
            .trim()
            .parse()
            .expect("a pid");
        assert!(alive(pid), "the grandchild is running before the drop");

        // aborting the task is what escape does: the future is dropped.
        running.abort();

        let mut dead = false;
        for _ in 0..100 {
            if !alive(pid) {
                dead = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(dead, "the grandchild outlived the turn: {pid}");
    }

    fn alive(pid: i32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }
}
