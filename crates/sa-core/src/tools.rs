//! tool execution (spec 9).
//!
//! tools run without asking. the only guard is that a path must resolve inside the
//! workspace.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::agent::{BoxFuture, ToolExecutor};
use crate::llm::{ToolCall, ToolSpec};

/// tool output above this is truncated in the event, with `truncated: true` recorded.
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
}

impl ToolRegistry {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
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

    async fn call(&self, call: &ToolCall) -> Result<String, ToolError> {
        let args = parse_args(&call.arguments)?;
        match call.name.as_str() {
            "read" => self.read(&args).await,
            "write" => self.write(&args).await,
            "edit" => self.edit(&args).await,
            "ls" => self.ls(&args).await,
            "find" => self.find(&args).await,
            "grep" => self.grep(&args).await,
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
            let suffix = if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
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
                    out.push(
                        entry
                            .path()
                            .strip_prefix(&root)
                            .unwrap_or(entry.path())
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
                        out.push(format!(
                            "{}:{}:{}",
                            entry
                                .path()
                                .strip_prefix(&root)
                                .unwrap_or(entry.path())
                                .display(),
                            number + 1,
                            line.trim_end()
                        ));
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

    async fn bash(&self, args: &Value) -> Result<String, ToolError> {
        let command = required(args, "command")?.to_owned();

        let mut process = tokio::process::Command::new("sh");
        process
            .arg("-c")
            .arg(&command)
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", "/run/current-system/sw/bin:/usr/bin:/bin")
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS),
            process.output(),
        )
        .await
        .map_err(|_| ToolError::Args(format!("timed out after {COMMAND_TIMEOUT_SECS}s")))??;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(format!(
            "exit {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

impl ToolExecutor for ToolRegistry {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![
            spec(
                "read",
                "read a file inside the workspace",
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            ),
            spec(
                "write",
                "write a file inside the workspace",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}),
            ),
            spec(
                "edit",
                "replace one unique occurrence in a file",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "old": {"type": "string"}, "new": {"type": "string"}}, "required": ["path", "old", "new"]}),
            ),
            spec(
                "ls",
                "list a directory",
                json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ),
            spec(
                "find",
                "find files by name substring",
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
            spec(
                "grep",
                "search file contents by regex",
                json!({"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}),
            ),
            spec(
                "bash",
                "run a shell command in the workspace",
                json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            ),
        ]
    }

    fn execute(&self, call: ToolCall) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move { self.call(&call).await.map_err(|e| e.to_string()) })
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
