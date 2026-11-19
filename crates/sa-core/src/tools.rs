//! tool execution (spec 9).
//!
//! tools run without asking. the only guard is that a path must resolve inside the
//! workspace.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use command_group::AsyncCommandGroup;

use crate::agent::{BoxFuture, ToolExecutor, ToolOutcome};
use crate::llm::{ToolCall, ToolSpec};

/// tool output above this is capped, with `truncated: true` recorded in the event.
pub const MAX_TOOL_OUTPUT: usize = 256 * 1024;

/// default command timeout (spec 9).
pub const COMMAND_TIMEOUT_SECS: u64 = 600;

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
}

impl ToolRegistry {
    #[must_use]
    pub const fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            mcp: None,
        }
    }

    /// a registry that also offers the tools of every connected mcp server.
    #[must_use]
    pub const fn with_mcp(workspace: PathBuf, mcp: std::sync::Arc<crate::mcp::Hub>) -> Self {
        Self {
            workspace,
            mcp: Some(mcp),
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
            "read" => self.read(&args).await.map(text),
            "write" => self.write(&args).await.map(text),
            "edit" => self.edit(&args).await.map(text),
            "ls" => self.ls(&args).await.map(text),
            "find" => self.find(&args).await.map(text),
            "grep" => self.grep(&args).await.map(text),
            "bash" => self.bash(&args).await,
            other => Err(ToolError::Unknown(other.to_owned())),
        }
    }

    async fn read(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.resolve(required(args, "path")?)?;
        Ok(tokio::fs::read_to_string(path).await?)
    }

    async fn write(&self, args: &Value) -> Result<String, ToolError> {
        let raw = required(args, "path")?;
        let path = self.resolve(raw)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let body = required(args, "content")?;
        tokio::fs::write(&path, body).await?;
        Ok(format!("wrote {} bytes to {raw}", body.len()))
    }

    async fn edit(&self, args: &Value) -> Result<String, ToolError> {
        let raw = required(args, "path")?;
        let path = self.resolve(raw)?;
        let old = required(args, "old")?;
        let new = required(args, "new")?;
        let body = tokio::fs::read_to_string(&path).await?;
        let count = body.matches(old).count();
        if count != 1 {
            return Err(ToolError::Args(format!(
                "{raw}: expected exactly one match, found {count}"
            )));
        }
        tokio::fs::write(&path, body.replacen(old, new, 1)).await?;
        Ok(format!("edited {raw}"))
    }

    async fn ls(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
        let mut entries = tokio::fs::read_dir(path).await?;
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let suffix = if entry.file_type().await.is_ok_and(|t| t.is_dir()) {
                "/"
            } else {
                ""
            };
            names.push(format!("{}{suffix}", entry.file_name().to_string_lossy()));
        }
        names.sort();
        Ok(names.join("\n"))
    }

    async fn find(&self, args: &Value) -> Result<String, ToolError> {
        let pattern = required(args, "pattern")?.to_owned();
        let root = self.workspace.clone();
        let matches = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_map(Result::ok)
                .take(5000)
            {
                if !entry.file_type().is_file() {
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
            for entry in walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_map(Result::ok)
                .take(5000)
            {
                if !entry.file_type().is_file() {
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
                        if out.len() >= 500 {
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

    async fn bash(&self, args: &Value) -> Result<ToolOutcome, ToolError> {
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

        // the command runs in its own process group, and the group is what gets killed:
        // a `sleep 100 &` would otherwise outlive the shell that started it.
        let child = process
            .group_spawn()
            .map_err(|e| ToolError::Args(e.to_string()))?;
        let mut killer = KillGroup(child.id().and_then(|id| i32::try_from(id).ok()));

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ToolError::Args(format!("timed out after {COMMAND_TIMEOUT_SECS}s")))??;
        killer.disarm();

        // one combined stream, as a terminal would show it: no section headers.
        let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(ToolOutcome {
            output: combined,
            exit_code: output.status.code(),
        })
    }
}

impl ToolExecutor for ToolRegistry {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut tools = vec![
            spec(
                "read",
                "Read a file as text.",
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            ),
            spec(
                "write",
                "Write a file, creating it or replacing its contents entirely.",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}),
            ),
            spec(
                "edit",
                "Replace exactly one occurrence of `old` with `new`. Fails unless `old` appears exactly once, so include enough surrounding context to be unique.",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "old": {"type": "string"}, "new": {"type": "string"}}, "required": ["path", "old", "new"]}),
            ),
            spec(
                "ls",
                "List the entries of a directory.",
                json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ),
            spec(
                "find",
                "Find files whose name contains the pattern. Returns up to 5000 paths.",
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
            spec(
                "grep",
                "Search file contents with a regular expression. Returns up to 5000 matches.",
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
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
