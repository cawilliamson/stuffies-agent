# cli

## Purpose

The `sa` binary: the user-facing program and the one place allowed to write to the terminal.
Every subcommand is a disposable client of the daemon except `sa daemon`.

## Requirements

### Requirement: Command surface

The binary SHALL provide `sa attach`, `sa context`, `sa daemon`, `sa export`, `sa lsp`,
`sa mcp`, `sa ping`, `sa rpc --stdio`, `sa run` and `sa sessions`. `--standalone` SHALL be a
global flag accepted by every subcommand.

#### Scenario: Every subcommand is a client except daemon

- **WHEN** a subcommand other than `daemon` runs
- **THEN** it connects to the daemon rather than owning state

#### Scenario: An absent daemon is reported, not spawned

- **WHEN** a subcommand runs with no daemon listening
- **THEN** it fails naming the socket path it could not reach

### Requirement: Version identity

`sa --version` SHALL report the package version together with the commit the binary was built
from, so a client and a daemon from different code are visibly different programs.

#### Scenario: The build is identifiable

- **WHEN** `sa --version` is run
- **THEN** the output carries both the version and the build commit

### Requirement: Run one prompt

`sa run -p <prompt>` SHALL start a turn, stream its progress to the terminal and exit when the
turn ends, reporting a non-zero status when the turn failed.

#### Scenario: A run streams what the turn does

- **WHEN** `sa run -p` is given a prompt that calls tools
- **THEN** each tool call and its result is printed as it happens

#### Scenario: A failed turn exits non-zero

- **WHEN** the turn is interrupted or the endpoint fails
- **THEN** the process exits non-zero and the cause is printed

### Requirement: Attach and session selection

`sa attach` SHALL attach to the most recent session, `--new` SHALL start a fresh one, and
`--session <id>` SHALL pick a specific one. Attaching SHALL never silently pick a session the
operator did not choose.

#### Scenario: An unknown session id is refused

- **WHEN** `sa attach --session` names a session that does not exist
- **THEN** it fails naming the id rather than falling back to another session

#### Scenario: A new session is explicit

- **WHEN** `sa attach --new` is run
- **THEN** a session is created rather than the most recent one resumed

### Requirement: Session listing

`sa sessions` SHALL list the daemon's sessions with their id, title, state and age.

#### Scenario: Sessions are listed with their state

- **WHEN** `sa sessions` is run
- **THEN** each session is shown with its state and how long since it changed

### Requirement: Export

`sa export --format jsonl|markdown` SHALL write a derived, human-readable rendering of a
session to stdout. It SHALL be derived only, never read back as authoritative state.

#### Scenario: A transcript can be read outside the client

- **WHEN** `sa export --format markdown` is run for a session
- **THEN** the session's transcript is printed as markdown

#### Scenario: An unknown format is refused

- **WHEN** `--format` names something other than `jsonl` or `markdown`
- **THEN** the command fails naming the accepted values

### Requirement: Stdio proxy

`sa rpc --stdio` SHALL connect to the daemon's socket and relay frames between it and
stdin/stdout with no protocol awareness, so `ssh <host> sa rpc --stdio` is a disposable
carrier. It SHALL exit non-zero when the daemon is absent.

#### Scenario: The proxy relays without parsing

- **WHEN** a frame arrives on stdin
- **THEN** it is written to the socket unchanged and the reply is written back unchanged

#### Scenario: The proxy reports a missing daemon

- **WHEN** nothing is listening on the socket
- **THEN** the proxy exits non-zero with the path named

### Requirement: Inspection commands

`sa context` SHALL show the context and skills that would be injected for a directory,
`sa lsp` SHALL list the language servers with their installed and running state,
`sa mcp` SHALL list the configured servers with their tools and errors, and
`sa openspec <args>` SHALL run the `openspec` program in the current directory with those
arguments: no daemon and no session, and no second implementation of the tree's format.

#### Scenario: Context is inspectable before a session exists

- **WHEN** `sa context --dir <path>` is run
- **THEN** the files and skills that would be injected are listed with their providers

#### Scenario: A missing server says why

- **WHEN** `sa lsp` lists a server that is configured but not found
- **THEN** the output shows it as missing and names the path searched

#### Scenario: The tree is readable with no daemon

- **WHEN** `sa openspec list --specs` runs in a directory that carries an `openspec/` tree
- **THEN** the program's output is printed and no socket is contacted

#### Scenario: A failure is a failure a hook can see

- **WHEN** the program exits non-zero
- **THEN** `sa openspec` exits non-zero naming the code it exited with

#### Scenario: An absent tree is named

- **WHEN** `sa openspec` runs where there is no `openspec/` directory
- **THEN** it exits non-zero naming the directory it looked for

### Requirement: Environment overrides

The binary SHALL honour `SA_SOCKET`, `SA_STATE_DIR`, `SA_MODEL`, `SA_TURN_TIMEOUT` and
`SA_AUX_BUDGET` so a second daemon and its clients can run beside the first without touching
it.

#### Scenario: A second daemon runs beside the first

- **WHEN** `SA_SOCKET` and `SA_STATE_DIR` name fresh paths
- **THEN** a daemon and its clients use them and leave the default daemon alone

### Requirement: Nothing is initialised

There SHALL be no `sa init`. The daemon creates its own directories on first use, and
per-workspace configuration is optional.

#### Scenario: First run needs no setup

- **WHEN** the daemon starts on a machine that has never run it
- **THEN** it creates what it needs under the XDG roots and works
