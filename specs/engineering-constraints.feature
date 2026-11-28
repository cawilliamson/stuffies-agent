# engineering-constraints
#
# The build's non-negotiable rules: crate boundaries, lints, the pre-commit gate, and the
# prohibition on language-level package managers. These are enforced by CI, not by convention.

Feature: engineering-constraints
  The build's non-negotiable rules: crate boundaries, lints, the pre-commit gate, and the
  prohibition on language-level package managers. These are enforced by CI, not by convention.

  # Requirement: Crate boundaries
  # The workspace SHALL keep these dependency rules, enforced by `ci/check-deps.sh`: - `sa-proto`
  # SHALL depend on serde and serde_json only: no workspace crate, no tokio, no terminal crate, no
  # HTTP stack. - `sa-core` SHALL NOT depend on `ratatui` or `crossterm` and SHALL NOT write to
  # stdout. - `sa-daemon` SHALL NOT depend on a terminal crate. - `sa-tui` SHALL NOT depend on `sa-
  # daemon` or `sa-core`; it talks to `sa-client`. - `sa-web`, when it exists, SHALL NOT depend on
  # `sa-daemon`, `sa-client` or a terminal crate.
  @req:crate-boundaries
  Scenario: A terminal crate in the engine fails the gate
    When `sa-core` gains a dependency on `crossterm`
    Then `ci/check-deps.sh` fails and names the crate
  @req:crate-boundaries
  Scenario: A proto crate that reaches for tokio fails the gate
    When `sa-proto` depends on tokio or another workspace crate
    Then the dependency check fails and prints the offending tree
  @req:crate-boundaries
  Scenario: The view does not hold authority
    When `sa-tui` depends on `sa-daemon` or `sa-core`
    Then the dependency check fails

  # Requirement: Lints
  # The workspace SHALL deny `unsafe_code`, `unwrap_used`, `expect_used`, `panic`, `todo`,
  # `unimplemented`, `dbg_macro`, `mem_forget`, `print_stdout` and `print_stderr` outside binaries,
  # together with clippy's `all`, `cargo`, `nursery` and `pedantic` groups. Tests MAY use `expect`
  # with a message.
  @req:lints
  Scenario: An unwrap fails the build
    When non-test code calls `unwrap`
    Then clippy fails the build
  @req:lints
  Scenario: A binary may print
    When the `sa` binary writes to stdout
    Then it is allowed, because it is the user-facing program

  # Requirement: Pre-commit gate
  # `ci/gate.sh` SHALL run, in order: `cargo fmt --check`; `cargo clippy --all-targets --all-
  # features --locked -- -D warnings`; `cargo test --workspace --locked`; the dependency check;
  # `cargo machete`; `cargo outdated --root-deps-only`; `ast-grep scan`; the complexity check; and
  # `cargo deny check`. A red gate SHALL mean do not commit.
  @req:pre-commit-gate
  Scenario: The gate refuses a dirty tree
    When formatting differs or a lint fails
    Then the gate exits non-zero before running the tests
  @req:pre-commit-gate
  Scenario: The gate is the commit hook
    When the hook is installed
    Then a commit that fails the gate does not land

  # Requirement: No language-level package managers
  # Dependencies SHALL be declared in `Cargo.toml` and provided through nixpkgs. Nothing SHALL be
  # installed with cargo install, npm, pip or any other language-level tool. There SHALL be no npm
  # and no JavaScript build step: web assets are hand-written and embedded.
  @req:no-language-level-package-managers
  Scenario: A dependency arrives through nix
    When a new crate is needed
    Then it is declared in `Cargo.toml` and supplied by the devshell, not installed ad hoc
  @req:no-language-level-package-managers
  Scenario: The web surface needs no build step
    When the web assets are changed
    Then they are served as written, with no bundler in the path

  # Requirement: Behaviour-named tests
  # Tests SHALL be named for the behaviour they check, in the form of a sentence about the system,
  # rather than numbered.
  @req:behaviour-named-tests
  Scenario: A test name says what broke
    When a recovery test fails
    Then its name describes the recovery, not a sequence number

  # Requirement: Nothing is kept for compatibility
  # The project is pre-release. There SHALL be no dual code paths, no deprecated aliases and no
  # migration shims: a change fixes the call sites.
  @req:nothing-is-kept-for-compatibility
  Scenario: A changed signature is fixed everywhere
    When a function's shape changes
    Then every caller is updated and no wrapper preserves the old shape

  # Requirement: Alphabetical ordering
  # Every collection written down SHALL be alphabetical — tool specifications, slash commands, help
  # text, enum variants, match arms over a fixed set, configuration keys, imports, test fixtures and
  # listings. Order SHALL carry meaning only for a wire sequence, a dependency's required order, a
  # ranking, a time series or a procedure the reader follows.
  @req:alphabetical-ordering
  Scenario: A method table reads as a list
    When the dispatcher's method arms are read
    Then they are alphabetical, so a missing method is visible
  @req:alphabetical-ordering
  Scenario: A procedure keeps its order
    When the pre-commit gate runs its steps
    Then the order is the order they must run in, not alphabetical
