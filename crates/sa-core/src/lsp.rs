//! language servers: which one a file belongs to, how it is started, and the queries an
//! editor makes of it.
//!
//! the protocol is `lspf`'s job: it frames the messages, performs the handshake, supervises
//! the child process and types every request. what lives here is the part that is ours:
//! where the server table is read from, which server a path belongs to, when a server is
//! started and stopped, and how a finding is rendered for the model.
//!
//! the server table is the shape `oh-my-pi` ships in `defaults.json` (mit), so a copy of
//! that file drops in unchanged: `fileTypes`, `rootMarkers`, `initOptions`, `settings` and
//! `isLinter` are all read.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lspf::types::{
    ClientCapabilities, DefinitionParams, DefinitionRequest, DidChangeTextDocumentNotification,
    DidChangeTextDocumentParams, DidOpenTextDocumentNotification, DidOpenTextDocumentParams,
    DidSaveTextDocumentNotification, DidSaveTextDocumentParams, DocumentDiagnosticParams,
    DocumentDiagnosticReport, DocumentDiagnosticRequest, DocumentSymbolParams,
    DocumentSymbolRequest, GotoDefinitionResponse, Hover, HoverParams, HoverRequest, Location,
    Position, PublishDiagnosticsNotification, ReferenceParams, ReferencesRequest, RenameParams,
    RenameRequest, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
};
use serde::Deserialize;
use serde_json::Value;

/// how long a server may take to start before the query is abandoned.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);
/// how long one request may take once the server is up.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// how long `edit` and `write` will wait for findings before reporting the change alone.
pub const EDIT_BUDGET: Duration = Duration::from_secs(8);
/// how long a query waits after the last report before believing it.
///
/// a server answers a pull before it has analysed anything: rust-analyzer returns an empty
/// report for a file that does not compile until its first pass finishes, and reports again
/// as it learns more. believing the first empty answer is how a broken file is called clean.
pub const SETTLE: Duration = Duration::from_millis(2500);
/// how many findings a tool result carries.
pub const MAX_FINDINGS: usize = 40;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("config: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no language server covers {0}")]
    NoServer(String),
    #[error("{server}: {error}")]
    Server { server: String, error: String },
}

/// asks a server what version it is, blocking.
///
/// `--version` is the convention, but not universal: `gopls` rejects it and answers to a
/// bare `version`. the caller bounds how long this may take, so nothing here waits.
fn probe_version(command: &str) -> Option<String> {
    let program = resolve(command)?;
    for args in [["--version"].as_slice(), ["version"].as_slice()] {
        let output = std::process::Command::new(&program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(version) = version_token(&text) {
            return Some(version);
        }
    }
    None
}

/// the first version-looking token in a line of output.
///
/// a server prints its name, then a version, and versions are not all semver: rust-analyzer
/// reports a date. a leading `v` is part of the spelling, not the number.
fn version_token(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let token =
            token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
        let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
        let core: String = token
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        let digits = core.chars().filter(char::is_ascii_digit).count();
        let marks = core.matches(['.', '-']).count();
        if digits >= 2 && marks >= 1 && core.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(core);
        }
    }
    None
}

/// one configured language server, as the list command reports it.
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub name: String,
    pub installed: bool,
    pub running: bool,
    /// what the server says its version is, when it can be asked cheaply.
    pub version: Option<String>,
}

/// one language server, in the shape `oh-my-pi`'s `defaults.json` uses.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(rename = "fileTypes", default)]
    pub file_types: Vec<String>,
    #[serde(rename = "rootMarkers", default)]
    pub root_markers: Vec<String>,
    #[serde(rename = "initOptions", default)]
    pub init_options: Value,
    /// a checker that only reports, rather than a server that also navigates. a
    /// navigator is preferred for a file both could serve.
    #[serde(rename = "isLinter", default)]
    pub is_linter: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    true
}

/// the whole table, keyed by the name the model and the logs use.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub servers: BTreeMap<String, ServerEntry>,
}

/// where a server table may be added to the built-in one.
#[must_use]
pub fn config_path() -> PathBuf {
    crate::workspace::config_dir().join("lsp.json")
}

impl Config {
    /// the servers this build knows about.
    ///
    /// a subset of `oh-my-pi`'s `defaults.json`, kept to the servers a project here meets.
    /// an entry whose command is absent from `PATH` is simply never chosen.
    #[must_use]
    pub fn builtin() -> Self {
        let entry =
            |command: &str, args: &[&str], types: &[&str], markers: &[&str], linter| ServerEntry {
                command: command.to_owned(),
                args: args.iter().map(|a| (*a).to_owned()).collect(),
                file_types: types.iter().map(|t| (*t).to_owned()).collect(),
                root_markers: markers.iter().map(|m| (*m).to_owned()).collect(),
                init_options: Value::Null,
                is_linter: linter,
                enabled: true,
            };
        let servers = [
            (
                "bash-language-server",
                entry(
                    "bash-language-server",
                    &["start"],
                    &[".sh", ".bash", ".zsh"],
                    &[".git", "shellcheckrc"],
                    false,
                ),
            ),
            (
                "clangd",
                entry(
                    "clangd",
                    &["--background-index", "--clang-tidy"],
                    &[".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx"],
                    &["CMakeLists.txt", "compile_commands.json", "meson.build"],
                    false,
                ),
            ),
            (
                "gopls",
                entry("gopls", &["serve"], &[".go"], &["go.mod", "go.work"], false),
            ),
            (
                "lua-language-server",
                entry(
                    "lua-language-server",
                    &[],
                    &[".lua"],
                    &[".luarc.json", ".stylua.toml"],
                    false,
                ),
            ),
            (
                "marksman",
                entry(
                    "marksman",
                    &["server"],
                    &[".md", ".markdown"],
                    &[".marksman.toml", ".git"],
                    false,
                ),
            ),
            (
                "nixd",
                entry(
                    "nixd",
                    &[],
                    &[".nix"],
                    &["flake.nix", "default.nix", "shell.nix"],
                    false,
                ),
            ),
            (
                "phpactor",
                entry(
                    "phpactor",
                    &["language-server"],
                    &[".php"],
                    &["composer.json", ".phpactor.json"],
                    false,
                ),
            ),
            (
                "pyright",
                entry(
                    "pyright-langserver",
                    &["--stdio"],
                    &[".py", ".pyi"],
                    &["pyproject.toml", "pyrightconfig.json", "setup.cfg"],
                    false,
                ),
            ),
            (
                "ruff",
                entry(
                    "ruff",
                    &["server"],
                    &[".py", ".pyi"],
                    &["pyproject.toml", "ruff.toml"],
                    true,
                ),
            ),
            (
                "rust-analyzer",
                entry(
                    "rust-analyzer",
                    &[],
                    &[".rs"],
                    &["Cargo.toml", "rust-analyzer.toml"],
                    false,
                ),
            ),
            (
                "taplo",
                entry(
                    "taplo",
                    &["lsp", "stdio"],
                    &[".toml"],
                    &[".taplo.toml", "taplo.toml"],
                    false,
                ),
            ),
            (
                "typescript-language-server",
                entry(
                    "typescript-language-server",
                    &["--stdio"],
                    &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
                    &["package.json", "tsconfig.json", "jsconfig.json"],
                    false,
                ),
            ),
            (
                "yamlls",
                entry(
                    "yaml-language-server",
                    &["--stdio"],
                    &[".yaml", ".yml"],
                    &[".git", "Chart.yaml"],
                    false,
                ),
            ),
            (
                "zls",
                entry(
                    "zls",
                    &[],
                    &[".zig"],
                    &["build.zig", "build.zig.zon"],
                    false,
                ),
            ),
        ];
        Self {
            servers: servers
                .into_iter()
                .map(|(name, entry)| (name.to_owned(), entry))
                .collect(),
        }
    }

    /// reads a table. a missing file is not an error: it means the built-in servers alone.
    pub fn load(path: &Path) -> Result<Self, LspError> {
        match std::fs::read_to_string(path) {
            Ok(body) => serde_json::from_str(&body)
                .map_err(|err| LspError::Config(format!("{}: {err}", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(LspError::Io(err)),
        }
    }

    /// the built-in servers with the configured ones laid over them. a configured entry of
    /// the same name corrects the built-in one rather than replacing it: what an operator
    /// changes is the command or an argument, and the file types the server already claims
    /// are not restated. an entry that does state them overrides them.
    #[must_use]
    pub fn load_default() -> Self {
        let Ok(configured) = Self::load(&config_path()) else {
            return Self::builtin();
        };
        Self::merged(Self::builtin(), configured)
    }

    /// a configured table laid over the built-in one. an entry of the same name corrects the
    /// built-in one rather than replacing it: what an operator changes is the command or an
    /// argument, and the file types the server already claims are not restated.
    #[must_use]
    pub fn merged(builtin: Self, configured: Self) -> Self {
        let mut merged = builtin;
        for (name, mut entry) in configured.servers {
            if let Some(builtin) = merged.servers.get(&name) {
                if entry.file_types.is_empty() {
                    entry.file_types.clone_from(&builtin.file_types);
                }
                if entry.root_markers.is_empty() {
                    entry.root_markers.clone_from(&builtin.root_markers);
                }
            }
            merged.servers.insert(name, entry);
        }
        merged
    }

    /// the servers that claim this path, most suitable first: a navigator before a linter,
    /// and a narrower file-type match before a broader one.
    #[must_use]
    pub fn candidates(&self, path: &Path) -> Vec<(&str, &ServerEntry)> {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            return Vec::new();
        };
        let wanted = format!(".{extension}");
        let mut found: Vec<(&str, &ServerEntry)> = self
            .servers
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .filter(|(_, entry)| entry.file_types.contains(&wanted))
            .map(|(name, entry)| (name.as_str(), entry))
            .collect();
        // a navigator answers more than a linter does, and this is a stable order the caller
        // can rely on when it takes the first that starts.
        found.sort_by_key(|(name, entry)| (entry.is_linter, *name));
        found
    }

    /// the first server that claims the path and whose command is installed.
    #[must_use]
    pub fn select(&self, path: &Path) -> Option<(&str, &ServerEntry)> {
        self.candidates(path)
            .into_iter()
            .find(|(_, entry)| installed(&entry.command))
    }
}

/// where a command is looked for.
///
/// `SA_LSP_PATH`, then the daemon's own path, then the path a login shell reports, then
/// `~/.local/bin`. asking the shell is the portable part: a service is handed a thin path
/// that omits whatever the user's profile adds, on every distribution, and a login shell is
/// what a terminal would have had. no distribution's directories are named here.
#[must_use]
pub fn search_paths() -> &'static [PathBuf] {
    static PATHS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    PATHS.get_or_init(|| {
        compose_paths(
            std::env::var_os("SA_LSP_PATH").as_deref(),
            std::env::var_os("PATH").as_deref(),
            login_shell_path(),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    })
}

/// the search path, in the order a command is looked for. a caller's own setting wins over
/// everything, because it is the one answer that is never a guess.
#[must_use]
fn compose_paths(
    explicit: Option<&std::ffi::OsStr>,
    own: Option<&std::ffi::OsStr>,
    shell: Vec<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for value in [explicit, own].into_iter().flatten() {
        paths.extend(std::env::split_paths(value));
    }
    paths.extend(shell);
    if let Some(home) = home {
        paths.push(home.join(".local/bin"));
    }
    // the first place a command is found is the one that runs, so a later repeat of a
    // directory that is already in the list can never change the answer.
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| !path.as_os_str().is_empty() && seen.insert(path.clone()));
    paths
}

/// the path a login shell reports. bounded, because a profile that waits on a network mount
/// must not hold up the first query.
fn login_shell_path() -> Vec<PathBuf> {
    const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
    let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) else {
        return Vec::new();
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = std::process::Command::new(shell)
            .args(["-l", "-c", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(output);
    });
    match rx.recv_timeout(SHELL_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => {
            let reported = String::from_utf8_lossy(&output.stdout);
            std::env::split_paths(std::ffi::OsStr::new(reported.as_ref())).collect()
        }
        // a shell that fails or is slow contributes nothing; the other sources still stand.
        _ => Vec::new(),
    }
}

/// whether a command can be started, without starting it.
fn installed(command: &str) -> bool {
    resolve(command).is_some()
}

/// where a configured command actually is, the way a shell would find it. a caller other
/// than the language servers uses this too: it is the only answer to "is this installed".
#[must_use]
pub fn resolve(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    search_paths()
        .iter()
        .map(|dir| dir.join(command))
        .find(|path| path.is_file())
}

/// the directory a server should be rooted at: the nearest ancestor holding one of its
/// markers, or the workspace when no marker is found.
#[must_use]
pub fn root_for(path: &Path, markers: &[String], workspace: &Path) -> PathBuf {
    let mut dir = path.parent();
    while let Some(current) = dir {
        if markers.iter().any(|marker| holds_marker(current, marker)) {
            return current.to_path_buf();
        }
        if current == workspace {
            break;
        }
        dir = current.parent();
    }
    workspace.to_path_buf()
}

/// whether a directory holds a marker, where a marker may name an extension (`*.cabal`).
fn holds_marker(dir: &Path, marker: &str) -> bool {
    marker.strip_prefix("*.").map_or_else(
        || dir.join(marker).exists(),
        |suffix| {
            std::fs::read_dir(dir).is_ok_and(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e == suffix)
                })
            })
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Information,
    Warning,
    Error,
}

impl Severity {
    const fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "info",
            Self::Hint => "hint",
        }
    }
}

/// one problem a server reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    pub source: String,
}

impl Finding {
    /// `path:line:column: severity: message [source]`, the form a compiler uses.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "{}:{}:{}: {}: {} [{}]",
            self.path,
            self.line,
            self.column,
            self.severity.label(),
            self.message,
            self.source
        )
    }
}

/// renders findings worst first, capped, with a line saying how many were left out.
#[must_use]
pub fn render(findings: &[Finding]) -> String {
    let mut sorted = findings.to_vec();
    sorted.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.column.cmp(&b.column))
    });
    sorted.dedup();
    let total = sorted.len();
    let mut out = String::new();
    for finding in sorted.iter().take(MAX_FINDINGS) {
        out.push_str(&finding.render());
        out.push('\n');
    }
    if total > MAX_FINDINGS {
        let _ = writeln!(out, "… {} more", total - MAX_FINDINGS);
    }
    out
}

/// a document a server has been told about, and the version it was told.
struct Open {
    version: i32,
}

/// one running server and the documents it holds.
struct Running {
    child: lspf::ChildConnection,
    handle: lspf::ServerHandle,
    open: Mutex<BTreeMap<String, Open>>,
    diagnostics: Arc<Mutex<BTreeMap<String, Vec<Finding>>>>,
    /// how many times the server has reported on a document. a clean file is a report with
    /// no findings, which is not the same as a server that has not spoken yet.
    reports: Arc<Mutex<BTreeMap<String, u64>>>,
}

/// the servers this daemon has started, keyed by server and root so one project's server is
/// never asked about another's files.
pub struct Hub {
    config: Config,
    running: Mutex<BTreeMap<String, Arc<Running>>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    /// a hub holding no servers until one is asked for. it is the daemon's, so a turn never
    /// pays for a start it did not need and a second turn reuses the one already running.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Config::load_default(),
            running: Mutex::new(BTreeMap::new()),
        }
    }

    /// the server table this hub was built from.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// what is configured, installed and running, for an operator asking.
    #[must_use]
    pub fn status(&self) -> Vec<ServerStatus> {
        /// how long the whole list may wait for versions. the probes run together, so this
        /// is the budget for all of them rather than for each.
        const VERSION_BUDGET: Duration = Duration::from_secs(3);

        let active: BTreeMap<String, usize> = self
            .running
            .lock()
            .map(|guard| {
                guard
                    .keys()
                    .filter_map(|key| key.split('\u{0}').next().map(str::to_owned))
                    .fold(BTreeMap::new(), |mut acc, name| {
                        *acc.entry(name).or_insert(0) += 1;
                        acc
                    })
            })
            .unwrap_or_default();

        let mut statuses = Vec::with_capacity(self.config.servers.len());
        let mut probes = Vec::new();
        for (name, entry) in &self.config.servers {
            let installed = installed(&entry.command);
            // only ask a server that is there: probing a missing one is a failed spawn.
            if installed {
                let command = entry.command.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(probe_version(&command));
                });
                probes.push((statuses.len(), rx));
            }
            statuses.push(ServerStatus {
                name: name.clone(),
                installed,
                running: active.contains_key(name),
                version: None,
            });
        }

        // every probe started together, so one slow server cannot hold up the rest and the
        // list waits no longer than the budget in total.
        let deadline = std::time::Instant::now() + VERSION_BUDGET;
        for (index, rx) in probes {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if let Ok(version) = rx.recv_timeout(left) {
                statuses[index].version = version;
            }
        }
        statuses
    }

    /// stops every server this hub started.
    pub async fn shutdown(&self) {
        let drained: Vec<Arc<Running>> = self
            .running
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard).into_values().collect())
            .unwrap_or_default();
        for server in drained {
            if let Ok(running) = Arc::try_unwrap(server) {
                let _ = running.child.shutdown().await;
            }
        }
    }

    /// the running server for this path, starting one if the path's server is installed.
    async fn ensure(&self, path: &Path, workspace: &Path) -> Result<Arc<Running>, LspError> {
        let (name, entry) = self
            .config
            .select(path)
            .ok_or_else(|| LspError::NoServer(path.display().to_string()))?;
        let root = root_for(path, &entry.root_markers, workspace);
        let key = format!("{name}\u{0}{}", root.display());
        if let Some(server) = self.running.lock().ok().and_then(|g| g.get(&key).cloned()) {
            return Ok(server);
        }
        let server = Arc::new(
            start(entry, &root)
                .await
                .map_err(|error| LspError::Server {
                    server: name.to_owned(),
                    error,
                })?,
        );
        if let Ok(mut guard) = self.running.lock() {
            // a second start that raced this one is dropped rather than kept, so one key
            // never names two children.
            let entry = guard.entry(key).or_insert_with(|| Arc::clone(&server));
            return Ok(Arc::clone(entry));
        }
        Ok(server)
    }

    /// tells the server what the file now holds, so what it reports is about this text.
    async fn sync(&self, server: &Arc<Running>, path: &Path) -> Result<Uri, LspError> {
        let uri = uri_of(path)?;
        let text = tokio::fs::read_to_string(path).await?;
        let key = uri.to_string();
        let previous = server
            .open
            .lock()
            .ok()
            .and_then(|guard| guard.get(&key).map(|open| open.version));
        match previous {
            None => {
                server
                    .handle
                    .notify::<DidOpenTextDocumentNotification>(DidOpenTextDocumentParams {
                        text_document: TextDocumentItem {
                            uri: uri.clone(),
                            language_id: language_id(path).into(),
                            version: 1,
                            text,
                        },
                    })
                    .map_err(|error| LspError::Server {
                        server: "notify".to_owned(),
                        error: error.to_string(),
                    })?;
                if let Ok(mut guard) = server.open.lock() {
                    guard.insert(key.clone(), Open { version: 1 });
                }
                forget(server, &key);
            }
            Some(version) => {
                let version = version + 1;
                server
                    .handle
                    .notify::<DidChangeTextDocumentNotification>(DidChangeTextDocumentParams {
                        text_document: VersionedTextDocumentIdentifier {
                            version,
                            text_document_identifier: TextDocumentIdentifier { uri: uri.clone() },
                        },
                        content_changes: vec![
                            TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                                lspf::types::TextDocumentContentChangeWholeDocument { text },
                            ),
                        ],
                    })
                    .map_err(|error| LspError::Server {
                        server: "notify".to_owned(),
                        error: error.to_string(),
                    })?;
                if let Ok(mut guard) = server.open.lock() {
                    guard.insert(key.clone(), Open { version });
                }
                forget(server, &key);
            }
        }
        Ok(uri)
    }

    /// every finding a server reports for this file, pushed or pulled, waiting until the
    /// server has stopped reporting or the budget is spent.
    pub async fn diagnostics(
        &self,
        path: &Path,
        workspace: &Path,
        budget: Duration,
    ) -> Result<Vec<Finding>, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        // a save is what makes a server re-check a file it has already read.
        let _ =
            server
                .handle
                .notify::<DidSaveTextDocumentNotification>(DidSaveTextDocumentParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    text: None,
                });
        let deadline = std::time::Instant::now() + budget;
        let mut reports = reports_of(&server, &uri);
        let mut quiet_since = std::time::Instant::now();
        loop {
            // a pull that answers is the server's own view of this text, which is better
            // than anything it pushed about an earlier one.
            if let Ok(Ok(report)) = tokio::time::timeout(
                REQUEST_TIMEOUT,
                server
                    .handle
                    .request::<DocumentDiagnosticRequest>(DocumentDiagnosticParams {
                        text_document: TextDocumentIdentifier { uri: uri.clone() },
                        identifier: None,
                        previous_result_id: None,
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: lspf::types::PartialResultParams::default(),
                    }),
            )
            .await
            {
                let found = findings_from_report(&report, &uri, &server, path);
                if !found.is_empty() {
                    return Ok(found);
                }
            }
            if let Some(pushed) = server
                .diagnostics
                .lock()
                .ok()
                .and_then(|guard| guard.get(&uri.to_string()).cloned())
            {
                return Ok(pushed);
            }
            let now = reports_of(&server, &uri);
            if now != reports {
                reports = now;
                quiet_since = std::time::Instant::now();
            }
            if quiet_since.elapsed() >= SETTLE || std::time::Instant::now() >= deadline {
                return Ok(Vec::new());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// where the symbol at a position is defined.
    pub async fn definition(
        &self,
        path: &Path,
        workspace: &Path,
        line: u32,
        character: u32,
    ) -> Result<String, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        let response = self
            .ask::<DefinitionRequest>(
                &server,
                DefinitionParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri },
                        position: Position::new(line, character),
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: lspf::types::PartialResultParams::default(),
                },
            )
            .await?;
        Ok(render_locations(
            response.map(locations_of).unwrap_or_default(),
        ))
    }

    /// every place the symbol at a position is used.
    pub async fn references(
        &self,
        path: &Path,
        workspace: &Path,
        line: u32,
        character: u32,
    ) -> Result<String, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        let found = self
            .ask::<ReferencesRequest>(
                &server,
                ReferenceParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri },
                        position: Position::new(line, character),
                    },
                    context: lspf::types::ReferenceContext {
                        include_declaration: true,
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: lspf::types::PartialResultParams::default(),
                },
            )
            .await?;
        Ok(render_locations(found.unwrap_or_default()))
    }

    /// what the symbol at a position is, as the server documents it.
    pub async fn hover(
        &self,
        path: &Path,
        workspace: &Path,
        line: u32,
        character: u32,
    ) -> Result<String, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        let found = self
            .ask::<HoverRequest>(
                &server,
                HoverParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri },
                        position: Position::new(line, character),
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            )
            .await?;
        Ok(found.map_or_else(
            || "no hover for that position".to_owned(),
            |hover| render_hover(&hover),
        ))
    }

    /// the declarations a file makes, with the line each is on.
    pub async fn symbols(&self, path: &Path, workspace: &Path) -> Result<String, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        let found = self
            .ask::<DocumentSymbolRequest>(
                &server,
                DocumentSymbolParams {
                    text_document: TextDocumentIdentifier { uri },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: lspf::types::PartialResultParams::default(),
                },
            )
            .await?;
        Ok(found.map_or_else(
            || "no symbols".to_owned(),
            |response| render_symbols(&response),
        ))
    }

    /// renames the symbol at a position across the project, and reports what changed.
    pub async fn rename(
        &self,
        path: &Path,
        workspace: &Path,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<String, LspError> {
        let server = self.ensure(path, workspace).await?;
        let uri = self.sync(&server, path).await?;
        let edit = self
            .ask::<RenameRequest>(
                &server,
                RenameParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri },
                        position: Position::new(line, character),
                    },
                    new_name: new_name.to_owned(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            )
            .await?
            .ok_or_else(|| LspError::Server {
                server: "rename".to_owned(),
                error: "the server has no name for that position".to_owned(),
            })?;
        apply_workspace_edit(&edit).await
    }

    /// one request, bounded by a deadline so a wedged server cannot hold a turn.
    async fn ask<R>(&self, server: &Arc<Running>, params: R::Params) -> Result<R::Result, LspError>
    where
        R: lspf::types::Request,
        R::Params: Send,
        R::Result: Send,
    {
        tokio::time::timeout(REQUEST_TIMEOUT, server.handle.request::<R>(params))
            .await
            .map_err(|_| LspError::Server {
                server: R::METHOD.to_string(),
                error: format!("no answer in {}s", REQUEST_TIMEOUT.as_secs()),
            })?
            .map_err(|error| LspError::Server {
                server: R::METHOD.to_string(),
                error: error.to_string(),
            })
    }
}

/// starts one server, rooted at `root`, with a handler that keeps what it pushes.
async fn start(entry: &ServerEntry, root: &Path) -> Result<Running, String> {
    let diagnostics: Arc<Mutex<BTreeMap<String, Vec<Finding>>>> = Arc::default();
    let reports: Arc<Mutex<BTreeMap<String, u64>>> = Arc::default();
    let sink = Arc::clone(&diagnostics);
    let counted = Arc::clone(&reports);
    // the executable is resolved on the path it was found on, and the child runs with that
    // path: a language server shells out to its own toolchain (`cargo`, `rustc`, `tsc`), and
    // a server that starts but cannot find its compiler reports less than it knows.
    let program = Path::new(&entry.command)
        .is_absolute()
        .then(|| PathBuf::from(&entry.command))
        .or_else(|| {
            search_paths()
                .iter()
                .map(|dir| dir.join(&entry.command))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| PathBuf::from(&entry.command));
    let mut command = tokio::process::Command::new(&program);
    command.args(&entry.args).current_dir(root);
    if let Ok(path) = std::env::join_paths(search_paths()) {
        command.env("PATH", path);
    }
    let child = tokio::time::timeout(
        START_TIMEOUT,
        lspf::Client::builder(ClientCapabilities::default())
            .client_info(lspf::types::ClientInfo {
                name: "stuffies-agent".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            })
            .initialization_options(entry.init_options.clone())
            .notification::<PublishDiagnosticsNotification, _, _>(move |_ctx, params| {
                let sink = Arc::clone(&sink);
                let counted = Arc::clone(&counted);
                async move {
                    let key = params.uri.to_string();
                    let findings = params
                        .diagnostics
                        .iter()
                        .map(|diagnostic| Finding {
                            path: path_of(&key),
                            line: diagnostic.range.start.line,
                            column: diagnostic.range.start.character,
                            severity: severity_of(diagnostic.severity),
                            message: message_text(&diagnostic.message),
                            source: diagnostic
                                .source
                                .clone()
                                .unwrap_or_else(|| "server".to_owned()),
                        })
                        .collect::<Vec<_>>();
                    if let Ok(mut guard) = sink.lock() {
                        if findings.is_empty() {
                            guard.remove(&key);
                        } else {
                            guard.insert(key.clone(), findings);
                        }
                    }
                    if let Ok(mut guard) = counted.lock() {
                        *guard.entry(key).or_insert(0) += 1;
                    }
                }
            })
            .spawn(command),
    )
    .await
    .map_err(|_| format!("no handshake in {}s", START_TIMEOUT.as_secs()))?
    .map_err(|error| error.to_string())?;
    let handle = child.server();
    Ok(Running {
        child,
        handle,
        open: Mutex::default(),
        diagnostics,
        reports,
    })
}

/// how many reports the server has made about a document.
fn reports_of(server: &Arc<Running>, uri: &Uri) -> u64 {
    server
        .reports
        .lock()
        .ok()
        .and_then(|guard| guard.get(&uri.to_string()).copied())
        .unwrap_or_default()
}

/// drops what the server said about a document, because the text has changed and the old
/// report describes text that is no longer there.
fn forget(server: &Arc<Running>, uri: &str) {
    if let Ok(mut guard) = server.diagnostics.lock() {
        guard.remove(uri);
    }
}

fn uri_of(path: &Path) -> Result<Uri, LspError> {
    let text = format!("file://{}", path.display());
    text.parse()
        .map_err(|_| LspError::Config(format!("not a uri: {text}")))
}

/// the path a uri names, for reporting rather than for opening.
fn path_of(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_owned()
}

fn language_id(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "bash" => "shellscript",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "cpp",
        "cjs" | "js" | "jsx" | "mjs" | "ts" | "tsx" => "typescript",
        "pyi" => "python",
        "yml" => "yaml",
        other => other,
    }
    .to_owned()
}

const fn severity_of(severity: Option<lspf::types::DiagnosticSeverity>) -> Severity {
    match severity {
        Some(lspf::types::DiagnosticSeverity::Error) => Severity::Error,
        Some(lspf::types::DiagnosticSeverity::Warning) => Severity::Warning,
        Some(lspf::types::DiagnosticSeverity::Information) => Severity::Information,
        _ => Severity::Hint,
    }
}

fn findings_from_report(
    report: &DocumentDiagnosticReport,
    uri: &Uri,
    server: &Arc<Running>,
    path: &Path,
) -> Vec<Finding> {
    let items = match report {
        DocumentDiagnosticReport::RelatedFullDocumentDiagnosticReport(full) => {
            &full.full_document_diagnostic_report.items
        }
        DocumentDiagnosticReport::RelatedUnchangedDocumentDiagnosticReport(_) => {
            return server
                .diagnostics
                .lock()
                .ok()
                .and_then(|guard| guard.get(&uri.to_string()).cloned())
                .unwrap_or_default();
        }
    };
    let key = uri.to_string();
    let findings: Vec<Finding> = items
        .iter()
        .map(|diagnostic| Finding {
            path: path.display().to_string(),
            line: diagnostic.range.start.line,
            column: diagnostic.range.start.character,
            severity: severity_of(diagnostic.severity),
            message: message_text(&diagnostic.message),
            source: diagnostic
                .source
                .clone()
                .unwrap_or_else(|| "server".to_owned()),
        })
        .collect();
    if let Ok(mut guard) = server.diagnostics.lock() {
        if findings.is_empty() {
            guard.remove(&key);
        } else {
            guard.insert(key, findings.clone());
        }
    }
    findings
}

fn locations_of(response: GotoDefinitionResponse) -> Vec<Location> {
    match response {
        GotoDefinitionResponse::Definition(lspf::types::Definition::Location(location)) => {
            vec![location]
        }
        GotoDefinitionResponse::Definition(lspf::types::Definition::LocationList(locations)) => {
            locations
        }
        GotoDefinitionResponse::DefinitionLinkList(links) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    }
}

fn render_locations(locations: Vec<Location>) -> String {
    if locations.is_empty() {
        return "nothing found".to_owned();
    }
    let mut out = String::new();
    for location in locations {
        let _ = writeln!(
            out,
            "{}:{}:{}",
            path_of(location.uri.as_ref()),
            location.range.start.line,
            location.range.start.character
        );
    }
    out
}

fn render_hover(hover: &Hover) -> String {
    match &hover.contents {
        lspf::types::HoverContents::MarkedString(marked) => marked_string(marked),
        lspf::types::HoverContents::MarkedStringList(items) => items
            .iter()
            .map(marked_string)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lspf::types::HoverContents::MarkupContent(content) => content.value.clone(),
    }
}

/// a diagnostic's text, whether the server sent it plain or as markup.
fn message_text(message: &lspf::types::Message) -> String {
    match message {
        lspf::types::Message::String(text) => text.clone(),
        lspf::types::Message::MarkupContent(content) => content.value.clone(),
    }
}

// the protocol still allows the older marked-string shape, so a server may send it even
// though markup is preferred.
#[allow(deprecated)]
fn marked_string(marked: &lspf::types::MarkedString) -> String {
    match marked {
        lspf::types::MarkedString::String(text) => text.clone(),
        lspf::types::MarkedString::MarkedStringWithLanguage(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    }
}

fn render_symbols(response: &lspf::types::DocumentSymbolResponse) -> String {
    let mut out = String::new();
    match response {
        lspf::types::DocumentSymbolResponse::SymbolInformationList(items) => {
            for item in items {
                let _ = writeln!(
                    out,
                    "{}:{}:{}: {:?} {}",
                    path_of(item.location.uri.as_ref()),
                    item.location.range.start.line,
                    item.location.range.start.character,
                    item.base_symbol_information.kind,
                    item.base_symbol_information.name
                );
            }
        }
        lspf::types::DocumentSymbolResponse::DocumentSymbolList(items) => {
            for item in items {
                walk_symbol(item, 0, &mut out);
            }
        }
    }
    if out.is_empty() {
        "no symbols".to_owned()
    } else {
        out
    }
}

fn walk_symbol(symbol: &lspf::types::DocumentSymbol, depth: usize, out: &mut String) {
    let _ = writeln!(
        out,
        "{}{}:{}: {:?} {}",
        "  ".repeat(depth),
        symbol.range.start.line,
        symbol.range.start.character,
        symbol.kind,
        symbol.name
    );
    if let Some(children) = &symbol.children {
        for child in children {
            walk_symbol(child, depth + 1, out);
        }
    }
}

/// applies a rename to the files it names and reports each change as a diff.
async fn apply_workspace_edit(edit: &lspf::types::WorkspaceEdit) -> Result<String, LspError> {
    let mut by_file: BTreeMap<PathBuf, Vec<lspf::types::TextEdit>> = BTreeMap::new();
    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            by_file
                .entry(PathBuf::from(path_of(uri.as_ref())))
                .or_default()
                .extend(edits.clone());
        }
    }
    if let Some(document_changes) = &edit.document_changes {
        for change in document_changes {
            let lspf::types::DocumentChange::TextDocumentEdit(document) = change else {
                continue;
            };
            by_file
                .entry(PathBuf::from(path_of(
                    document.text_document.text_document_identifier.uri.as_ref(),
                )))
                .or_default()
                .extend(document.edits.iter().filter_map(|edit| match edit {
                    lspf::types::Edit::TextEdit(text) => Some(text.clone()),
                    lspf::types::Edit::AnnotatedTextEdit(annotated) => {
                        Some(annotated.text_edit.clone())
                    }
                    // a snippet is only sent to a client that declared support, which this
                    // one does not.
                    lspf::types::Edit::SnippetTextEdit(_) => None,
                }));
        }
    }
    if by_file.is_empty() {
        return Ok("rename changed nothing".to_owned());
    }
    let mut out = String::new();
    for (path, mut edits) in by_file {
        let Ok(before) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        // later edits first, so an earlier one does not move the text a later one addresses.
        edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
        let mut after = before.clone();
        for edit in edits.iter().rev() {
            let Some(start) = offset_of(&after, edit.range.start) else {
                continue;
            };
            let Some(end) = offset_of(&after, edit.range.end) else {
                continue;
            };
            if start <= end && end <= after.len() {
                after.replace_range(start..end, &edit.new_text);
            }
        }
        if after == before {
            continue;
        }
        tokio::fs::write(&path, &after).await?;
        let _ = writeln!(
            out,
            "{}",
            crate::tools::diff(&before, &after, &path.display().to_string())
        );
    }
    if out.is_empty() {
        Ok("rename changed nothing".to_owned())
    } else {
        Ok(out)
    }
}

/// the byte offset a position names, counting `character` as utf-16 code units the way the
/// protocol does.
#[must_use]
fn offset_of(text: &str, position: Position) -> Option<usize> {
    let mut line_start = 0;
    for _ in 0..position.line {
        line_start += text[line_start..].find('\n')? + 1;
    }
    let mut units = 0_u32;
    for (index, character) in text[line_start..].char_indices() {
        if units >= position.character || character == '\n' {
            return Some(line_start + index);
        }
        units += character.len_utf16() as u32;
    }
    Some(text.len())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_file_is_served_by_rust_analyzer() {
        let config = Config::builtin();
        let found = config.candidates(Path::new("src/main.rs"));
        assert_eq!(found.first().map(|(name, _)| *name), Some("rust-analyzer"));
    }

    #[test]
    fn a_navigator_is_preferred_to_a_linter() {
        let config = Config::builtin();
        let found = config.candidates(Path::new("tool.py"));
        assert_eq!(found.first().map(|(name, _)| *name), Some("pyright"));
    }

    #[test]
    fn an_explicit_search_path_comes_first_and_the_rest_follow() {
        let paths = compose_paths(
            Some(std::ffi::OsStr::new("/opt/tools/bin")),
            Some(std::ffi::OsStr::new("/usr/bin")),
            vec![PathBuf::from("/home/someone/.nix-profile/bin")],
            Some(PathBuf::from("/home/someone")),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/opt/tools/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/home/someone/.nix-profile/bin"),
                PathBuf::from("/home/someone/.local/bin"),
            ]
        );
    }

    #[test]
    fn a_repeated_directory_is_named_once() {
        let paths = compose_paths(
            None,
            Some(std::ffi::OsStr::new("/usr/bin:/usr/bin:/bin")),
            vec![PathBuf::from("/usr/bin")],
            None,
        );
        assert_eq!(
            paths,
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
    }

    #[test]
    fn an_unknown_extension_has_no_server() {
        let config = Config::builtin();
        assert!(config.candidates(Path::new("notes.xyz")).is_empty());
    }

    #[test]
    fn a_configured_entry_replaces_the_builtin_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lsp.json");
        std::fs::write(
            &path,
            r#"{"servers":{"rust-analyzer":{"command":"/bin/false","fileTypes":[".rs"]}}}"#,
        )
        .expect("write");
        let mut merged = Config::builtin();
        for (name, entry) in Config::load(&path).expect("load").servers {
            merged.servers.insert(name, entry);
        }
        let (name, entry) = merged
            .candidates(Path::new("src/main.rs"))
            .into_iter()
            .next()
            .expect("candidate");
        assert_eq!(name, "rust-analyzer");
        assert_eq!(entry.command, "/bin/false");
    }

    #[test]
    fn the_root_is_the_nearest_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(dir.path().join("a/Cargo.toml"), "").expect("write");
        let file = nested.join("main.rs");
        let root = root_for(&file, &["Cargo.toml".to_owned()], dir.path());
        assert_eq!(root, dir.path().join("a"));
    }

    #[test]
    fn without_a_marker_the_root_is_the_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("x/y.rs");
        let root = root_for(&file, &["Cargo.toml".to_owned()], dir.path());
        assert_eq!(root, dir.path());
    }

    #[test]
    fn findings_render_worst_first_and_are_capped() {
        let mut findings = Vec::new();
        for line in 0..(MAX_FINDINGS as u32 + 2) {
            findings.push(Finding {
                path: "a.rs".to_owned(),
                line,
                column: 0,
                severity: Severity::Warning,
                message: "w".to_owned(),
                source: "s".to_owned(),
            });
        }
        findings.push(Finding {
            path: "a.rs".to_owned(),
            line: 99,
            column: 0,
            severity: Severity::Error,
            message: "e".to_owned(),
            source: "s".to_owned(),
        });
        let rendered = render(&findings);
        assert!(rendered.starts_with("a.rs:99:0: error: e [s]"));
        assert!(rendered.contains("… 3 more"));
    }

    #[test]
    fn a_position_counts_utf16_units() {
        let text = "let s = \"é😀\";\nnext";
        // the emoji is two utf-16 units, the e-acute one.
        assert_eq!(offset_of(text, Position::new(0, 0)), Some(0));
        assert_eq!(offset_of(text, Position::new(0, 9)), Some(9));
        assert_eq!(offset_of(text, Position::new(1, 0)), Some(18));
    }
}
