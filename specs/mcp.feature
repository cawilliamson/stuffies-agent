# mcp
#
# Model Context Protocol servers: where their configuration lives, how their tools are named for
# the model, and how a call reaches the connection that owns it.

Feature: mcp
  Model Context Protocol servers: where their configuration lives, how their tools are named
  for the model, and how a call reaches the connection that owns it.

  # Requirement: Configuration
  # The daemon SHALL read `$XDG_CONFIG_HOME/stuffies-agent/mcp.json` in the format every other MCP
  # client uses: `mcpServers`, each entry carrying `command`, `args`, `env` and `cwd` for a stdio
  # transport or `url` and `headers` for a streamable HTTP transport, plus `enabled` and a timeout.
  # A missing file SHALL mean no servers.
  @req:configuration
  Scenario: An absent config offers no tools
    When no `mcp.json` exists
    Then no server is connected and the built-in tools are unaffected
  @req:configuration
  Scenario: A disabled entry is skipped
    When an entry sets `enabled` false
    Then it is not connected and its tools are not offered

  # Requirement: Credentials come from the environment
  # `${VAR}` in an `env` or header value SHALL be expanded from the daemon's environment, so
  # credentials stay out of the file. An unset variable SHALL be left as written rather than
  # replaced with an empty string.
  @req:credentials-come-from-the-environment
  Scenario: A credential is expanded
    When a header value carries `${HOME}`
    Then the daemon substitutes its own value of that variable
  @req:credentials-come-from-the-environment
  Scenario: An unset variable is left alone
    When a value names a variable that is not set
    Then the literal `${VAR}` text is sent

  # Requirement: Namespaced tool names
  # A server's tools SHALL be offered to the model as `mcp__<server>__<tool>`, so a server tool can
  # never shadow a built-in and a call routes on the name alone.
  @req:namespaced-tool-names
  Scenario: A server cannot shadow a built-in
    When a server exposes a tool named `read`
    Then the model sees `mcp__<server>__read` and `read` still reaches the built-in
  @req:namespaced-tool-names
  Scenario: A call routes on the name
    When the model calls `mcp__<server>__<tool>`
    Then the call reaches the connection for that server and no other

  # Requirement: One failure does not stop the others
  # A server that fails to connect SHALL be recorded with its error, skipped, and SHALL NOT stop the
  # other servers or the turn. `mcp.list` SHALL report each configured server's name, transport,
  # tool count and error.
  @req:one-failure-does-not-stop-the-others
  Scenario: A broken server is reported and skipped
    When a configured server cannot be reached
    Then `mcp.list` carries its error and every other server's tools still work
  @req:one-failure-does-not-stop-the-others
  Scenario: A broken server does not stop a turn
    When a turn runs while one server is unreachable
    Then the turn proceeds with the built-in tools and the servers that connected

  # Requirement: The protocol is the SDK's
  # Framing, handshake and call typing SHALL be `rmcp`'s through `rig-rmcp`. What belongs to this
  # project is where configuration is read from, how a tool is named, and when a server is reached.
  @req:the-protocol-is-the-sdk-s
  Scenario: Connections are established once
    When the daemon starts with configured servers
    Then each enabled server is connected once and reused for every turn

  # Requirement: A hung server does not hang the turn
  # A tool call to a server SHALL be bounded by the entry's timeout, default 60 seconds, and a
  # connection SHALL be bounded at 30 seconds.
  @req:a-hung-server-does-not-hang-the-turn
  Scenario: A silent server times out
    When a server accepts a call and never answers
    Then the call fails with the timeout named and the turn continues
