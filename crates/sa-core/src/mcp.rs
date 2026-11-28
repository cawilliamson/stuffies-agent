//! model context protocol servers: configuration, connection and tool dispatch.
//!
//! the protocol itself is the official sdk's job (`rmcp`, through `rig-rmcp`). what lives
//! here is the part that is ours: where configuration is read from, how a server's tools are
//! named for the model, and how a call reaches the connection that owns it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rig_rmcp::McpConnection;
use rmcp::model::CallToolRequestParams;
use rmcp::service::ServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::transport::TokioChildProcess;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::llm::ToolSpec;

/// every mcp tool the model sees is named with this prefix, so a built-in can never be
/// shadowed by a server and a call can be routed without a lookup table of its own.
pub const TOOL_PREFIX: &str = "mcp__";
/// how long a server may take to start and list its tools.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// the deadline for one tool call when the configuration does not set one.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("config: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// one configured server, in the shape other mcp clients use.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerEntry {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

const fn default_enabled() -> bool {
    true
}

/// the file, in the format every other mcp client uses.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, ServerEntry>,
}

/// where the servers are configured. user level only: a workspace never holds configuration.
#[must_use]
pub fn config_path() -> PathBuf {
    crate::workspace::config_dir().join("mcp.json")
}

impl Config {
    /// reads the configuration. a missing file is not an error: it means no servers.
    pub fn load(path: &Path) -> Result<Self, McpError> {
        match std::fs::read_to_string(path) {
            Ok(body) => serde_json::from_str(&body)
                .map_err(|err| McpError::Config(format!("{}: {err}", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(McpError::Io(err)),
        }
    }

    /// the configuration this daemon should use.
    #[must_use]
    pub fn load_default() -> Self {
        Self::load(&config_path()).unwrap_or_default()
    }

    /// whether an entry has the one field that says how to reach it.
    #[must_use]
    pub const fn kind(entry: &ServerEntry) -> Option<Transport> {
        match (&entry.command, &entry.url) {
            (Some(_), None) => Some(Transport::Stdio),
            (None, Some(_)) => Some(Transport::Http),
            // both, or neither: the entry does not say how to reach it.
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
}

/// a tool name the model can call, and the server tool behind it.
#[must_use]
pub fn tool_name(server: &str, tool: &str) -> String {
    format!("{TOOL_PREFIX}{}__{}", sanitise(server), sanitise(tool))
}

/// tool names are plain identifiers, so anything else becomes an underscore.
fn sanitise(raw: &str) -> String {
    raw.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// expands `${VAR}` from the environment, which is how credentials are kept out of the file.
#[must_use]
pub fn expand(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(found) => out.push_str(&found),
            // an unset variable stays visible rather than silently emptying a credential.
            Err(_) => out.push_str(&rest[start..=start + 2 + end]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// one server as the operator sees it.
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub name: String,
    pub transport: &'static str,
    pub tools: Vec<String>,
    /// absent when the server connected.
    pub error: Option<String>,
}

struct Connected {
    name: String,
    connection: McpConnection,
    timeout: Duration,
    /// the model-facing name, the name the server knows it by, and what the model is told.
    tools: Vec<(String, String, ToolSpec)>,
}

/// every server the daemon could reach, and the tools they offer.
pub struct Hub {
    servers: Vec<Connected>,
    status: Vec<ServerStatus>,
}

impl Hub {
    /// a hub with no servers, for a daemon that has none configured.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            servers: Vec::new(),
            status: Vec::new(),
        }
    }

    /// connects every enabled server. one that fails is reported and skipped: a broken
    /// server must not stop the others, or the agent, from working.
    pub async fn connect(config: &Config) -> Self {
        let mut hub = Self::empty();
        for (name, entry) in &config.servers {
            if !entry.enabled {
                hub.status.push(ServerStatus {
                    name: name.clone(),
                    transport: transport_name(entry),
                    tools: Vec::new(),
                    error: None,
                });
                continue;
            }
            match connect_one(name, entry).await {
                Ok(connected) => {
                    hub.status.push(ServerStatus {
                        name: name.clone(),
                        transport: transport_name(entry),
                        tools: connected
                            .tools
                            .iter()
                            .map(|(exposed, _, _)| exposed.clone())
                            .collect(),
                        error: None,
                    });
                    hub.servers.push(connected);
                }
                Err(error) => hub.status.push(ServerStatus {
                    name: name.clone(),
                    transport: transport_name(entry),
                    tools: Vec::new(),
                    error: Some(error),
                }),
            }
        }
        hub
    }

    /// every tool the model may call, from every connected server.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().map(|(_, _, spec)| spec.clone()))
            .collect()
    }

    /// what to show the operator, including the servers that failed.
    #[must_use]
    pub fn status(&self) -> Vec<ServerStatus> {
        self.status.clone()
    }

    /// calls a tool by its model-facing name.
    pub async fn call(&self, tool: &str, args: &Value) -> Result<String, String> {
        let Some((server, real)) = self.servers.iter().find_map(|server| {
            server
                .tools
                .iter()
                .find(|(exposed, _, _)| exposed == tool)
                .map(|(_, real, _)| (server, real))
        }) else {
            return Err(format!("no mcp server offers {tool}"));
        };

        let arguments: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        let mut params = CallToolRequestParams::new(real.clone());
        params.arguments = Some(arguments);

        let call = server.connection.peer().call_tool(params);
        let result = match tokio::time::timeout(server.timeout, call).await {
            Ok(Ok(result)) => result,
            Ok(Err(err)) => return Err(format!("{}: {err}", server.name)),
            Err(_) => {
                return Err(format!(
                    "{}: no answer within {}s",
                    server.name,
                    server.timeout.as_secs()
                ));
            }
        };

        if result.is_error.unwrap_or(false) {
            return Err(render(&result));
        }
        Ok(render(&result))
    }
}

/// the model sees the text blocks of a result; anything else is carried as json.
fn render(result: &rmcp::model::CallToolResult) -> String {
    let mut out = String::new();
    for block in &result.content {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            rmcp::model::ContentBlock::Text(text) => out.push_str(&text.text),
            other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
    }
    if out.is_empty() {
        if let Some(structured) = &result.structured_content {
            out.push_str(&serde_json::to_string(structured).unwrap_or_default());
        }
    }
    out
}

const fn transport_name(entry: &ServerEntry) -> &'static str {
    if entry.url.is_some() {
        "http"
    } else {
        "stdio"
    }
}

async fn connect_one(name: &str, entry: &ServerEntry) -> Result<Connected, String> {
    let timeout = Duration::from_secs(entry.timeout.max(1));
    let connection = match Config::kind(entry) {
        Some(Transport::Stdio) => connect_stdio(entry).await?,
        Some(Transport::Http) => connect_http(entry).await?,
        None => return Err("needs a command or a url, not both or neither".to_owned()),
    };

    let listed = tokio::time::timeout(CONNECT_TIMEOUT, connection.tools())
        .await
        .map_err(|_| "timed out listing tools".to_owned())?
        .map_err(|err| err.to_string())?;

    let tools = listed
        .into_iter()
        .map(|tool| {
            let definition = tool.definition();
            let real = definition.name.to_string();
            let exposed = tool_name(name, &real);
            let spec = ToolSpec {
                name: exposed.clone(),
                description: format!(
                    "{} (mcp: {name})",
                    definition
                        .description
                        .as_deref()
                        .unwrap_or("An mcp server tool.")
                ),
                parameters: Value::Object((*definition.input_schema).clone()),
            };
            (exposed, real, spec)
        })
        .collect();

    Ok(Connected {
        name: name.to_owned(),
        connection,
        timeout,
        tools,
    })
}

async fn connect_stdio(entry: &ServerEntry) -> Result<McpConnection, String> {
    let command = entry.command.clone().ok_or("stdio needs a command")?;
    let mut cmd = tokio::process::Command::new(expand(&command));
    for arg in &entry.args {
        cmd.arg(expand(arg));
    }
    for (key, value) in &entry.env {
        cmd.env(key, expand(value));
    }
    if let Some(cwd) = &entry.cwd {
        cmd.current_dir(expand(cwd));
    }

    let transport = TokioChildProcess::new(cmd).map_err(|err| err.to_string())?;
    let service = ().serve(transport).await.map_err(|err| err.to_string())?;
    Ok(McpConnection::new(service))
}

async fn connect_http(entry: &ServerEntry) -> Result<McpConnection, String> {
    use rmcp_reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let url = entry.url.clone().ok_or("http needs a url")?;
    let mut headers = HeaderMap::new();
    for (key, value) in &entry.headers {
        let name = HeaderName::from_bytes(key.as_bytes()).map_err(|err| err.to_string())?;
        let value = HeaderValue::from_str(&expand(value)).map_err(|err| err.to_string())?;
        headers.insert(name, value);
    }
    let client = rmcp_reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|err| err.to_string())?;

    let mut config = StreamableHttpClientTransportConfig::default();
    config.uri = expand(&url).into();
    let worker = StreamableHttpClientWorker::new(client, config);
    let transport = StreamableHttpClientTransport::spawn(worker);
    let service = ().serve(transport).await.map_err(|err| err.to_string())?;
    Ok(McpConnection::new(service))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn config(body: &str) -> Config {
        serde_json::from_str(body).expect("config")
    }

    #[test]
    fn a_server_entry_says_how_to_reach_it() {
        let parsed = config(
            r#"{"mcpServers": {
                "local": {"command": "npx", "args": ["-y", "server"]},
                "remote": {"url": "https://example.test/mcp"},
                "broken": {"enabled": false},
                "both": {"command": "x", "url": "y"}
            }}"#,
        );
        assert_eq!(
            Config::kind(&parsed.servers["local"]),
            Some(Transport::Stdio)
        );
        assert_eq!(
            Config::kind(&parsed.servers["remote"]),
            Some(Transport::Http)
        );
        assert_eq!(Config::kind(&parsed.servers["broken"]), None);
        assert_eq!(Config::kind(&parsed.servers["both"]), None);
        assert!(!parsed.servers["broken"].enabled, "enabled defaults on");
        assert!(parsed.servers["local"].enabled);
    }

    #[test]
    fn tools_are_namespaced_so_a_server_cannot_shadow_a_built_in() {
        assert_eq!(
            tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
        assert_eq!(
            tool_name("my-server", "do.thing"),
            "mcp__my_server__do_thing",
            "anything that is not an identifier becomes an underscore"
        );
        assert!(tool_name("s", "t").starts_with(TOOL_PREFIX));
    }

    #[test]
    fn credentials_come_from_the_environment() {
        let home = std::env::var("HOME").expect("HOME is set");
        assert_eq!(expand("at ${HOME}"), format!("at {home}"));
        assert_eq!(expand("plain"), "plain");
        assert_eq!(
            expand("${SA_MCP_DEFINITELY_UNSET}"),
            "${SA_MCP_DEFINITELY_UNSET}",
            "an unset variable stays visible rather than emptying a credential"
        );
        assert_eq!(expand("${unterminated"), "${unterminated");
    }

    #[test]
    fn a_missing_config_is_no_servers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parsed = Config::load(&dir.path().join("mcp.json")).expect("load");
        assert!(parsed.servers.is_empty());
    }

    #[test]
    fn a_hub_with_nothing_configured_offers_nothing() {
        let hub = Hub::empty();
        assert!(hub.specs().is_empty());
        assert!(hub.status().is_empty());
    }
}
