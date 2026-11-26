# session-store

## Purpose

The sqlite database that is the sole source of truth for live sessions: the append-only
event log, session rows, and the blob table oversized payloads spill into.

## Requirements

### Requirement: Sole source of truth

The daemon SHALL keep live sessions in a sqlite database at
`$XDG_STATE_HOME/stuffies-agent/sa.db`, with `SA_STATE_DIR` overriding the directory. It
SHALL open it in WAL mode with synchronous commits. There SHALL be no second mutable store
of session state.

#### Scenario: The database lives outside the workspace

- **WHEN** a daemon starts against a workspace
- **THEN** `sa.db` and its WAL files are under the state directory and the workspace gains
  no file

#### Scenario: Standalone never touches the on-disk database

- **WHEN** a command runs under `--standalone`
- **THEN** it opens an in-memory database and the on-disk one is not migrated or written

### Requirement: Schema versioning

The store SHALL record its schema version and SHALL refuse to open a database whose version
differs from the one this build expects, naming both.

#### Scenario: A future schema is refused rather than guessed

- **WHEN** the database records a schema version this build does not know
- **THEN** opening fails with both versions named and no migration is attempted

### Requirement: Event append is atomic and ordered

Appending an event SHALL allocate the session's next `seq` inside one immediate
transaction, store the payload as JSON, and update the session's `updated_at` in the same
transaction. `seq` SHALL start at 1 and increase by one per event.

#### Scenario: Concurrent appends do not collide

- **WHEN** two appends for one session race
- **THEN** they receive consecutive `seq` values and both rows exist

#### Scenario: Sequence numbers are contiguous

- **WHEN** a session's events are read in order
- **THEN** the sequence starts at 1 and has no gap

### Requirement: Payload versioning

Every stored payload SHALL carry a `v` field naming its payload version, written by the one
append path rather than by each caller. A payload version this build does not know SHALL be
refused rather than decoded best-effort.

#### Scenario: An appended payload always carries its version

- **WHEN** any event is appended
- **THEN** its stored payload contains `v` even when the caller did not set it

### Requirement: Oversized payloads spill to blobs

Any string payload field above 64 KiB SHALL be written once to the blob table and replaced
in the event payload with `{"$blob": "<hex sha256>", "bytes": N}`, at any depth of the
payload. Reading an event SHALL expand the reference back to the original string.

#### Scenario: A large tool output is stored once

- **WHEN** a tool output above 64 KiB is appended
- **THEN** the event payload carries a blob reference and the blob table holds one row whose
  hash matches the content

#### Scenario: A spilled payload reads back whole

- **WHEN** the event is replayed
- **THEN** the caller receives the original string, not the reference

### Requirement: Event window reads

The store SHALL return events after a given `seq`, optionally bounded above by `to_seq`, in
ascending order, with a `more` flag when the requested limit was exceeded. A read SHALL NOT
return a partial event.

#### Scenario: A bounded window stops at its upper bound

- **WHEN** events are read with `to_seq` set
- **THEN** no event with a higher `seq` is returned

### Requirement: Open turn detection

The store SHALL report every session whose last turn event is `turn.started` rather than a
terminal kind, together with the tool call ids that never received a terminal event, so
recovery can close them.

#### Scenario: An interrupted call is reported as pending

- **WHEN** a turn appended `tool.requested` and then the daemon died
- **THEN** the store reports that call id as pending for that turn
