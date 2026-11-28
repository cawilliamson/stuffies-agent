# session-lifecycle
#
# Creating, listing, attaching to, renaming, moving, deleting and restoring a session, and the
# states a session may be in. Sessions are records in the log, not leases held by a client.

Feature: session-lifecycle
  Creating, listing, attaching to, renaming, moving, deleting and restoring a session, and the
  states a session may be in. Sessions are records in the log, not leases held by a client.

  # Requirement: Session creation
  # `session.create` SHALL create a session row, append `session.created` with the title, the
  # working directory, the client that asked, whether the title was chosen by the operator, the
  # owner `{host_fingerprint, epoch}` and the knowledge namespace set, then append `context.loaded`
  # describing what the directory provides, and notify every connection with `sessions.changed`. The
  # working directory SHALL be canonicalised. An explicit title SHALL never be overwritten; an
  # absent one SHALL default to the last component of the directory.
  @req:session-creation
  Scenario: Creation records the owner and the directory
    When a client creates a session for a directory
    Then `session.created` carries that canonical directory, the host fingerprint, epoch zero, and a title defaulting to the directory's last component
  @req:session-creation
  Scenario: An explicit title is preserved
    When a session is created with a title
    Then the stored title is exactly that, and `titled` is true in the event
  @req:session-creation
  Scenario: Creation is announced to every window
    When a session is created while two clients are connected
    Then both receive `sessions.changed`

  # Requirement: Session states
  # A session SHALL be in one of `active`, `quiescing`, `relocating`, `relocated`, `interrupted` or
  # `deleted`. Only `active` is writable, and `interrupted` SHALL be accepted by `turn.start`, which
  # moves it to `active` and records the transition.
  @req:session-states
  Scenario: Only an active session takes a turn
    When `turn.start` is called on a `relocated` session
    Then it is refused with `-32006` and the message names the state
  @req:session-states
  Scenario: An interrupted session accepts a new turn
    When `turn.start` is called on an `interrupted` session
    Then the turn starts and a `session.state` event records the move to `active`

  # Requirement: Session listing
  # `session.list` SHALL return each session's id, title, state, working directory, owner epoch,
  # model, effort, creation time and update time, most recently updated first. Deleted sessions
  # SHALL be omitted unless `include_deleted` is true. A session with no stored effort SHALL report
  # the effort a turn would actually use.
  @req:session-listing
  Scenario: A deleted session is hidden by default
    When a session has been deleted and the list is requested without `include_deleted`
    Then it is absent from the result
  @req:session-listing
  Scenario: Tabs can be ordered without shuffling
    When two sessions are listed
    Then each carries `created_at` and `updated_at`

  # Requirement: Attach is not ownership
  # `session.attach` SHALL return the session summary and a window of its events from `from_seq`,
  # and SHALL NOT tie the session to the connection. Any number of clients SHALL hold the same
  # session at once, and no client SHALL evict another.
  @req:attach-is-not-ownership
  Scenario: Two clients hold one session
    When two clients attach to the same session
    Then both receive the history and neither is disconnected
  @req:attach-is-not-ownership
  Scenario: A turn started by one client is visible to the other
    When one client starts a turn and the other is subscribed
    Then the second receives the turn's events

  # Requirement: Rename and working directory
  # `session.rename` and `session.cwd` SHALL update the session, append `session.renamed` or
  # `session.cwd`, and push the event to subscribers. `session.cwd` SHALL refuse a path that is not
  # an existing directory, and SHALL record the new directory's context.
  @req:rename-and-working-directory
  Scenario: A rename is pushed to subscribers
    When a subscribed client renames a session
    Then it receives a `session.renamed` event carrying the new title
  @req:rename-and-working-directory
  Scenario: A missing directory is refused
    When `session.cwd` names a path that does not exist
    Then it is refused with `-32602` and the session's directory is unchanged
  @req:rename-and-working-directory
  Scenario: Moving a session records what applies there
    When a session is moved to another directory
    Then a `context.loaded` event follows, describing the new directory

  # Requirement: Deletion is a tombstone
  # `session.delete` SHALL move the session to `deleted`, append `session.state` with the reason,
  # and retain the event log. It SHALL be refused with `-32004` while a turn is in flight. There
  # SHALL be no purge.
  @req:deletion-is-a-tombstone
  Scenario: Deleting keeps the history
    When a session is deleted and listed with `include_deleted`
    Then it appears as `deleted` and its events still replay
  @req:deletion-is-a-tombstone
  Scenario: A busy session is not deleted
    When `session.delete` is called during a turn
    Then it is refused with `-32004` and the session stays active

  # Requirement: Restore
  # `session.restore` SHALL bring a deleted session back to `active`, appending `session.state` with
  # the reason and announcing the change. Restoring a session that is not deleted SHALL succeed
  # without appending anything.
  @req:restore
  Scenario: A deleted session is brought back
    When `session.restore` is called on a deleted session
    Then the state becomes `active`, the transition is recorded, and every window is told
  @req:restore
  Scenario: Restoring a live session is not an error
    When `session.restore` is called on an active session
    Then it succeeds and no event is appended

  # Requirement: Fork is not yet implemented
  # `session.fork` SHALL answer `-32014` with a message naming the milestone, so a client can
  # distinguish an unbuilt method from an unknown one.
  @req:fork-is-not-yet-implemented
  Scenario: Fork is refused as not yet
    When `session.fork` is called
    Then the code is `-32014` and the message says which milestone brings it

  # Requirement: Usage accounting
  # `session.usage` SHALL sum the prompt, completion and cached token counts of every
  # `llm.completed` event in the session. `cost_micros` SHALL be absent when the endpoint prices
  # nothing.
  @req:usage-accounting
  Scenario: Usage sums the turn's reported figures
    When a session has completed turns with reported usage
    Then `session.usage` reports the totals, with cached tokens counted separately
  @req:usage-accounting
  Scenario: An unpriced endpoint claims no cost
    When no pricing is available for the model
    Then `cost_micros` is absent rather than zero
