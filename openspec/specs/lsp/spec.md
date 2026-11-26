# lsp

## Purpose

The `lsp` tool and the hub behind it: which language server a file belongs to, when one is
started, what a query returns, and what a turn is told about the file it just changed.

## Requirements

### Requirement: Server table

The server table SHALL be read from `$XDG_CONFIG_HOME/stuffies-agent/lsp.json` in the shape
`oh-my-pi` ships in `defaults.json` — `fileTypes`, `rootMarkers`, `initOptions`, `settings`
and `isLinter` — merged over the built-in entries, so an entry of the same name corrects a
command or argument without restating the file types.

#### Scenario: A configured entry overrides the built-in one

- **WHEN** the config names an entry that already exists
- **THEN** its command and arguments replace the built-in values and its file types are kept

#### Scenario: An absent config leaves the built-ins

- **WHEN** no `lsp.json` exists
- **THEN** the built-in entries are used and nothing fails

### Requirement: A server must be installed to be chosen

A server SHALL be chosen only when its binary is found on the search path, which is
`SA_LSP_PATH` when set, then the daemon's own path, then the path a login shell reports, then
`~/.local/bin`. No distribution's directories SHALL be named in the code.

#### Scenario: An uninstalled server is never started

- **WHEN** a file's extension matches a configured server whose binary is not found
- **THEN** no process is started and the tool reports nothing rather than an error

#### Scenario: The search path is reported

- **WHEN** `lsp.list` is called
- **THEN** the result names the directories searched, so a missing server says why

### Requirement: Starting and reusing a server

A server SHALL start when a turn first asks about a file it covers, SHALL be rooted at the
nearest project marker above that file, and SHALL be keyed by server and root so one
project's server is never asked about another's files. A later turn SHALL reuse the running
server.

#### Scenario: One project's server is not asked about another's files

- **WHEN** two workspaces with different roots need the same server
- **THEN** two instances are started, each rooted at its own marker

#### Scenario: A running server is reused

- **WHEN** a second turn asks about a file covered by an already running server
- **THEN** it is asked without paying for another start

#### Scenario: A start that never answers gives up

- **WHEN** a server does not complete its handshake within 20 seconds
- **THEN** the query fails with a named reason rather than hanging the turn

### Requirement: Queries

`lsp {action}` SHALL support `definition`, `diagnostics`, `hover`, `references`, `rename`,
`status` and `symbols`. `line` and `character` SHALL be 0-based and default to the start of
the file. `status` SHALL need no path and SHALL report which servers are configured,
installed and running. An unknown action SHALL be an argument error.

#### Scenario: Status needs no path

- **WHEN** `lsp` is called with `action: status`
- **THEN** it reports each server's configured, installed and running state

#### Scenario: Rename applies the server's edit across files

- **WHEN** `lsp` is called with `action: rename` and a new name
- **THEN** every file the server names is changed and each change is reported as a diff

#### Scenario: A clean file says so

- **WHEN** diagnostics are requested for a file with no findings
- **THEN** the result says the file has no findings

### Requirement: Findings follow an edit

`edit` and a `write` that replaces an existing file SHALL report the findings for the file
they changed, worst first, as `path:line:column: severity: message [source]`, bounded by an
8 second budget. A budget that expires SHALL report nothing rather than fail the tool.

#### Scenario: An edit reports what it broke

- **WHEN** an edit leaves a file that does not compile
- **THEN** the result carries the findings for that file

#### Scenario: A slow indexer does not block the edit

- **WHEN** the server has not indexed the file within the budget
- **THEN** the edit still succeeds and reports no findings

### Requirement: A pull is not believed on its first answer

A query SHALL wait for the server to stop reporting, up to 2.5 seconds, before calling a file
clean. Findings SHALL come from a pull where the server supports it and from a push where it
does not. A server that supports neither SHALL report nothing, which is not an error.

#### Scenario: A broken file is never called clean

- **WHEN** a server returns an empty report for a file it has not finished indexing
- **THEN** the query waits for the server to settle before accepting that answer

#### Scenario: A server with no diagnostics support is silent

- **WHEN** a server neither pulls nor pushes diagnostics
- **THEN** the query returns no findings and no error

### Requirement: The server is told what the file now holds

Before a server is asked about a changed file, it SHALL be sent the current contents, so what
it reports is about the text on disk rather than the text it was opened with.

#### Scenario: A fixed finding disappears

- **WHEN** an edit fixes a reported problem and the file is queried again
- **THEN** the fixed finding is no longer reported

### Requirement: The server runs with its own toolchain

A server SHALL be started with the search path it was found on, because it shells out to its
own toolchain.

#### Scenario: The toolchain is reachable from the server

- **WHEN** a server is started
- **THEN** its environment carries the path it was discovered on, so its toolchain commands
  resolve

### Requirement: Nothing leaves the machine

A language server SHALL be a local process started from the search path. No query SHALL
require the network.

#### Scenario: Findings work offline

- **WHEN** the machine has no network
- **THEN** diagnostics and navigation still answer
