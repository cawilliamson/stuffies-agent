# engineering-constraints

## Purpose

The build's non-negotiable rules: crate boundaries, lints, the pre-commit gate, and the
prohibition on language-level package managers. These are enforced by CI, not by convention.

## Requirements

### Requirement: Crate boundaries

The workspace SHALL keep these dependency rules, enforced by `ci/check-deps.sh`:

- `sa-proto` SHALL depend on serde and serde_json only: no workspace crate, no tokio, no
  terminal crate, no HTTP stack.
- `sa-core` SHALL NOT depend on `ratatui` or `crossterm` and SHALL NOT write to stdout.
- `sa-daemon` SHALL NOT depend on a terminal crate.
- `sa-tui` SHALL NOT depend on `sa-daemon` or `sa-core`; it talks to `sa-client`.
- `sa-web`, when it exists, SHALL NOT depend on `sa-daemon`, `sa-client` or a terminal crate.

#### Scenario: A terminal crate in the engine fails the gate

- **WHEN** `sa-core` gains a dependency on `crossterm`
- **THEN** `ci/check-deps.sh` fails and names the crate

#### Scenario: A proto crate that reaches for tokio fails the gate

- **WHEN** `sa-proto` depends on tokio or another workspace crate
- **THEN** the dependency check fails and prints the offending tree

#### Scenario: The view does not hold authority

- **WHEN** `sa-tui` depends on `sa-daemon` or `sa-core`
- **THEN** the dependency check fails

### Requirement: Lints

The workspace SHALL deny `unsafe_code`, `unwrap_used`, `expect_used`, `panic`, `todo`,
`unimplemented`, `dbg_macro`, `mem_forget`, `print_stdout` and `print_stderr` outside
binaries, together with clippy's `all`, `cargo`, `nursery` and `pedantic` groups. Tests MAY
use `expect` with a message.

#### Scenario: An unwrap fails the build

- **WHEN** non-test code calls `unwrap`
- **THEN** clippy fails the build

#### Scenario: A binary may print

- **WHEN** the `sa` binary writes to stdout
- **THEN** it is allowed, because it is the user-facing program

### Requirement: Pre-commit gate

`ci/gate.sh` SHALL run, in order: `cargo fmt --check`; `cargo clippy --all-targets
--all-features --locked -- -D warnings`; `cargo test --workspace --locked`; the dependency
check; `cargo machete`; `cargo outdated --root-deps-only`; `ast-grep scan`; the complexity
check; and `cargo deny check`. A red gate SHALL mean do not commit.

#### Scenario: The gate refuses a dirty tree

- **WHEN** formatting differs or a lint fails
- **THEN** the gate exits non-zero before running the tests

#### Scenario: The gate is the commit hook

- **WHEN** the hook is installed
- **THEN** a commit that fails the gate does not land

### Requirement: No language-level package managers

Dependencies SHALL be declared in `Cargo.toml` and provided through nixpkgs. Nothing SHALL be
installed with cargo install, npm, pip or any other language-level tool. There SHALL be no
npm and no JavaScript build step: web assets are hand-written and embedded.

#### Scenario: A dependency arrives through nix

- **WHEN** a new crate is needed
- **THEN** it is declared in `Cargo.toml` and supplied by the devshell, not installed ad hoc

#### Scenario: The web surface needs no build step

- **WHEN** the web assets are changed
- **THEN** they are served as written, with no bundler in the path

### Requirement: Behaviour-named tests

Tests SHALL be named for the behaviour they check, in the form of a sentence about the
system, rather than numbered.

#### Scenario: A test name says what broke

- **WHEN** a recovery test fails
- **THEN** its name describes the recovery, not a sequence number

### Requirement: Nothing is kept for compatibility

The project is pre-release. There SHALL be no dual code paths, no deprecated aliases and no
migration shims: a change fixes the call sites.

#### Scenario: A changed signature is fixed everywhere

- **WHEN** a function's shape changes
- **THEN** every caller is updated and no wrapper preserves the old shape

### Requirement: Alphabetical ordering

Every collection written down SHALL be alphabetical — tool specifications, slash commands,
help text, enum variants, match arms over a fixed set, configuration keys, imports, test
fixtures and listings. Order SHALL carry meaning only for a wire sequence, a dependency's
required order, a ranking, a time series or a procedure the reader follows.

#### Scenario: A method table reads as a list

- **WHEN** the dispatcher's method arms are read
- **THEN** they are alphabetical, so a missing method is visible

#### Scenario: A procedure keeps its order

- **WHEN** the pre-commit gate runs its steps
- **THEN** the order is the order they must run in, not alphabetical
