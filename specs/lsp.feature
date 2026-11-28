# lsp
#
# The `lsp` tool and the hub behind it: which language server a file belongs to, when one is
# started, what a query returns, and what a turn is told about the file it just changed.

Feature: lsp
  The `lsp` tool and the hub behind it: which language server a file belongs to, when one is
  started, what a query returns, and what a turn is told about the file it just changed.

  # Requirement: Server table
  # The server table SHALL be read from `$XDG_CONFIG_HOME/stuffies-agent/lsp.json` in the shape `oh-
  # my-pi` ships in `defaults.json` — `fileTypes`, `rootMarkers`, `initOptions`, `settings` and
  # `isLinter` — merged over the built-in entries, so an entry of the same name corrects a command
  # or argument without restating the file types.
  @req:server-table
  Scenario: A configured entry overrides the built-in one
    When the config names an entry that already exists
    Then its command and arguments replace the built-in values and its file types are kept
  @req:server-table
  Scenario: An absent config leaves the built-ins
    When no `lsp.json` exists
    Then the built-in entries are used and nothing fails

  # Requirement: A server must be installed to be chosen
  # A server SHALL be chosen only when its binary is found on the search path, which is
  # `SA_LSP_PATH` when set, then the daemon's own path, then the path a login shell reports, then
  # `~/.local/bin`. No distribution's directories SHALL be named in the code.
  @req:a-server-must-be-installed-to-be-chosen
  Scenario: An uninstalled server is never started
    When a file's extension matches a configured server whose binary is not found
    Then no process is started and the tool reports nothing rather than an error
  @req:a-server-must-be-installed-to-be-chosen
  Scenario: The search path is reported
    When `lsp.list` is called
    Then the result names the directories searched, so a missing server says why

  # Requirement: Starting and reusing a server
  # A server SHALL start when a turn first asks about a file it covers, SHALL be rooted at the
  # nearest project marker above that file, and SHALL be keyed by server and root so one project's
  # server is never asked about another's files. A later turn SHALL reuse the running server.
  @req:starting-and-reusing-a-server
  Scenario: One project's server is not asked about another's files
    When two workspaces with different roots need the same server
    Then two instances are started, each rooted at its own marker
  @req:starting-and-reusing-a-server
  Scenario: A running server is reused
    When a second turn asks about a file covered by an already running server
    Then it is asked without paying for another start
  @req:starting-and-reusing-a-server
  Scenario: A start that never answers gives up
    When a server does not complete its handshake within 20 seconds
    Then the query fails with a named reason rather than hanging the turn

  # Requirement: Queries
  # `lsp {action}` SHALL support `definition`, `diagnostics`, `hover`, `references`, `rename`,
  # `status` and `symbols`. `line` and `character` SHALL be 0-based and default to the start of the
  # file. `status` SHALL need no path and SHALL report which servers are configured, installed and
  # running. An unknown action SHALL be an argument error.
  @req:queries
  Scenario: Status needs no path
    When `lsp` is called with `action: status`
    Then it reports each server's configured, installed and running state
  @req:queries
  Scenario: Rename applies the server's edit across files
    When `lsp` is called with `action: rename` and a new name
    Then every file the server names is changed and each change is reported as a diff
  @req:queries
  Scenario: A clean file says so
    When diagnostics are requested for a file with no findings
    Then the result says the file has no findings

  # Requirement: Findings follow an edit
  # `edit` and a `write` that replaces an existing file SHALL report the findings for the file they
  # changed, worst first, as `path:line:column: severity: message [source]`, bounded by an 8 second
  # budget. A budget that expires SHALL report nothing rather than fail the tool.
  @req:findings-follow-an-edit
  Scenario: An edit reports what it broke
    When an edit leaves a file that does not compile
    Then the result carries the findings for that file
  @req:findings-follow-an-edit
  Scenario: A slow indexer does not block the edit
    When the server has not indexed the file within the budget
    Then the edit still succeeds and reports no findings

  # Requirement: A pull is not believed on its first answer
  # A query SHALL wait for the server to stop reporting, up to 2.5 seconds, before calling a file
  # clean. Findings SHALL come from a pull where the server supports it and from a push where it
  # does not. A server that supports neither SHALL report nothing, which is not an error.
  @req:a-pull-is-not-believed-on-its-first-answer
  Scenario: A broken file is never called clean
    When a server returns an empty report for a file it has not finished indexing
    Then the query waits for the server to settle before accepting that answer
  @req:a-pull-is-not-believed-on-its-first-answer
  Scenario: A server with no diagnostics support is silent
    When a server neither pulls nor pushes diagnostics
    Then the query returns no findings and no error

  # Requirement: The server is told what the file now holds
  # Before a server is asked about a changed file, it SHALL be sent the current contents, so what it
  # reports is about the text on disk rather than the text it was opened with.
  @req:the-server-is-told-what-the-file-now-holds
  Scenario: A fixed finding disappears
    When an edit fixes a reported problem and the file is queried again
    Then the fixed finding is no longer reported

  # Requirement: The server runs with its own toolchain
  # A server SHALL be started with the search path it was found on, because it shells out to its own
  # toolchain.
  @req:the-server-runs-with-its-own-toolchain
  Scenario: The toolchain is reachable from the server
    When a server is started
    Then its environment carries the path it was discovered on, so its toolchain commands resolve

  # Requirement: Nothing leaves the machine
  # A language server SHALL be a local process started from the search path. No query SHALL require
  # the network.
  @req:nothing-leaves-the-machine
  Scenario: Findings work offline
    When the machine has no network
    Then diagnostics and navigation still answer
