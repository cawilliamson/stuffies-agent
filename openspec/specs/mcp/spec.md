# mcp

## Purpose

Model Context Protocol servers: where their configuration lives, how their tools are named
for the model, and how a call reaches the connection that owns it.

## Requirements

### Requirement: Configuration

The daemon SHALL read `$XDG_CONFIG_HOME/stuffies-agent/mcp.json` in the format every other MCP
client uses: `mcpServers`, each entry carrying `command`, `args`, `env` and `cwd` for a stdio
transport or `url` and `headers` for a streamable HTTP transport, plus `enabled` and a
timeout. A missing file SHALL mean no servers.

#### Scenario: An absent config offers no tools

- **WHEN** no `mcp.json` exists
- **THEN** no server is connected and the built-in tools are unaffected

#### Scenario: A disabled entry is skipped

- **WHEN** an entry sets `enabled` false
- **THEN** it is not connected and its tools are not offered

### Requirement: Credentials come from the environment

`${VAR}` in an `env` or header value SHALL be expanded from the daemon's environment, so
credentials stay out of the file. An unset variable SHALL be left as written rather than
replaced with an empty string.

#### Scenario: A credential is expanded

- **WHEN** a header value carries `${HOME}`
- **THEN** the daemon substitutes its own value of that variable

#### Scenario: An unset variable is left alone

- **WHEN** a value names a variable that is not set
- **THEN** the literal `${VAR}` text is sent

### Requirement: Namespaced tool names

A server's tools SHALL be offered to the model as `mcp__<server>__<tool>`, so a server tool
can never shadow a built-in and a call routes on the name alone.

#### Scenario: A server cannot shadow a built-in

- **WHEN** a server exposes a tool named `read`
- **THEN** the model sees `mcp__<server>__read` and `read` still reaches the built-in

#### Scenario: A call routes on the name

- **WHEN** the model calls `mcp__<server>__<tool>`
- **THEN** the call reaches the connection for that server and no other

### Requirement: One failure does not stop the others

A server that fails to connect SHALL be recorded with its error, skipped, and SHALL NOT stop
the other servers or the turn. `mcp.list` SHALL report each configured server's name,
transport, tool count and error.

#### Scenario: A broken server is reported and skipped

- **WHEN** a configured server cannot be reached
- **THEN** `mcp.list` carries its error and every other server's tools still work

#### Scenario: A broken server does not stop a turn

- **WHEN** a turn runs while one server is unreachable
- **THEN** the turn proceeds with the built-in tools and the servers that connected

### Requirement: The protocol is the SDK's

Framing, handshake and call typing SHALL be `rmcp`'s through `rig-rmcp`. What belongs to this
project is where configuration is read from, how a tool is named, and when a server is
reached.

#### Scenario: Connections are established once

- **WHEN** the daemon starts with configured servers
- **THEN** each enabled server is connected once and reused for every turn

### Requirement: A hung server does not hang the turn

A tool call to a server SHALL be bounded by the entry's timeout, default 60 seconds, and a
connection SHALL be bounded at 30 seconds.

#### Scenario: A silent server times out

- **WHEN** a server accepts a call and never answers
- **THEN** the call fails with the timeout named and the turn continues
