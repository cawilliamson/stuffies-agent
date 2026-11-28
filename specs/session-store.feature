# session-store
#
# The sqlite database that is the sole source of truth for live sessions: the append-only event
# log, session rows, and the blob table oversized payloads spill into.

Feature: session-store
  The sqlite database that is the sole source of truth for live sessions: the append-only
  event log, session rows, and the blob table oversized payloads spill into.

  # Requirement: Sole source of truth
  # The daemon SHALL keep live sessions in a sqlite database at `$XDG_STATE_HOME/stuffies-
  # agent/sa.db`, with `SA_STATE_DIR` overriding the directory. It SHALL open it in WAL mode with
  # synchronous commits. There SHALL be no second mutable store of session state.
  @req:sole-source-of-truth
  Scenario: The database lives outside the workspace
    When a daemon starts against a workspace
    Then `sa.db` and its WAL files are under the state directory and the workspace gains no file
  @req:sole-source-of-truth
  Scenario: Standalone never touches the on-disk database
    When a command runs under `--standalone`
    Then it opens an in-memory database and the on-disk one is not migrated or written

  # Requirement: Schema versioning
  # The store SHALL record its schema version and SHALL refuse to open a database whose version
  # differs from the one this build expects, naming both.
  @req:schema-versioning
  Scenario: A future schema is refused rather than guessed
    When the database records a schema version this build does not know
    Then opening fails with both versions named and no migration is attempted

  # Requirement: Event append is atomic and ordered
  # Appending an event SHALL allocate the session's next `seq` inside one immediate transaction,
  # store the payload as JSON, and update the session's `updated_at` in the same transaction. `seq`
  # SHALL start at 1 and increase by one per event.
  @req:event-append-is-atomic-and-ordered
  Scenario: Concurrent appends do not collide
    When two appends for one session race
    Then they receive consecutive `seq` values and both rows exist
  @req:event-append-is-atomic-and-ordered
  Scenario: Sequence numbers are contiguous
    When a session's events are read in order
    Then the sequence starts at 1 and has no gap

  # Requirement: Payload versioning
  # Every stored payload SHALL carry a `v` field naming its payload version, written by the one
  # append path rather than by each caller. A payload version this build does not know SHALL be
  # refused rather than decoded best-effort.
  @req:payload-versioning
  Scenario: An appended payload always carries its version
    When any event is appended
    Then its stored payload contains `v` even when the caller did not set it

  # Requirement: Oversized payloads spill to blobs
  # Any string payload field above 64 KiB SHALL be written once to the blob table and replaced in
  # the event payload with `{"$blob": "<hex sha256>", "bytes": N}`, at any depth of the payload.
  # Reading an event SHALL expand the reference back to the original string.
  @req:oversized-payloads-spill-to-blobs
  Scenario: A large tool output is stored once
    When a tool output above 64 KiB is appended
    Then the event payload carries a blob reference and the blob table holds one row whose hash matches the content
  @req:oversized-payloads-spill-to-blobs
  Scenario: A spilled payload reads back whole
    When the event is replayed
    Then the caller receives the original string, not the reference

  # Requirement: Event window reads
  # The store SHALL return events after a given `seq`, optionally bounded above by `to_seq`, in
  # ascending order, with a `more` flag when the requested limit was exceeded. A read SHALL NOT
  # return a partial event.
  @req:event-window-reads
  Scenario: A bounded window stops at its upper bound
    When events are read with `to_seq` set
    Then no event with a higher `seq` is returned

  # Requirement: Open turn detection
  # The store SHALL report every session whose last turn event is `turn.started` rather than a
  # terminal kind, together with the tool call ids that never received a terminal event, so recovery
  # can close them.
  @req:open-turn-detection
  Scenario: An interrupted call is reported as pending
    When a turn appended `tool.requested` and then the daemon died
    Then the store reports that call id as pending for that turn
