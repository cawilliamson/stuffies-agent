# session-lifecycle

## Purpose

Creating, listing, attaching to, renaming, moving, deleting and restoring a session, and the
states a session may be in. Sessions are records in the log, not leases held by a client.

## Requirements

### Requirement: Session creation

`session.create` SHALL create a session row, append `session.created` with the title, the
working directory, the client that asked, whether the title was chosen by the operator, the
owner `{host_fingerprint, epoch}` and the knowledge namespace set, then append
`context.loaded` describing what the directory provides, and notify every connection with
`sessions.changed`.

The working directory SHALL be canonicalised. An explicit title SHALL never be overwritten;
an absent one SHALL default to the last component of the directory.

#### Scenario: Creation records the owner and the directory

- **WHEN** a client creates a session for a directory
- **THEN** `session.created` carries that canonical directory, the host fingerprint, epoch
  zero, and a title defaulting to the directory's last component

#### Scenario: An explicit title is preserved

- **WHEN** a session is created with a title
- **THEN** the stored title is exactly that, and `titled` is true in the event

#### Scenario: Creation is announced to every window

- **WHEN** a session is created while two clients are connected
- **THEN** both receive `sessions.changed`

### Requirement: Session states

A session SHALL be in one of `active`, `quiescing`, `relocating`, `relocated`, `interrupted`
or `deleted`. Only `active` is writable, and `interrupted` SHALL be accepted by
`turn.start`, which moves it to `active` and records the transition.

#### Scenario: Only an active session takes a turn

- **WHEN** `turn.start` is called on a `relocated` session
- **THEN** it is refused with `-32006` and the message names the state

#### Scenario: An interrupted session accepts a new turn

- **WHEN** `turn.start` is called on an `interrupted` session
- **THEN** the turn starts and a `session.state` event records the move to `active`

### Requirement: Session listing

`session.list` SHALL return each session's id, title, state, working directory, owner epoch,
model, effort, creation time and update time, most recently updated first. Deleted sessions
SHALL be omitted unless `include_deleted` is true. A session with no stored effort SHALL
report the effort a turn would actually use.

#### Scenario: A deleted session is hidden by default

- **WHEN** a session has been deleted and the list is requested without `include_deleted`
- **THEN** it is absent from the result

#### Scenario: Tabs can be ordered without shuffling

- **WHEN** two sessions are listed
- **THEN** each carries `created_at` and `updated_at`

### Requirement: Attach is not ownership

`session.attach` SHALL return the session summary and a window of its events from `from_seq`,
and SHALL NOT tie the session to the connection. Any number of clients SHALL hold the same
session at once, and no client SHALL evict another.

#### Scenario: Two clients hold one session

- **WHEN** two clients attach to the same session
- **THEN** both receive the history and neither is disconnected

#### Scenario: A turn started by one client is visible to the other

- **WHEN** one client starts a turn and the other is subscribed
- **THEN** the second receives the turn's events

### Requirement: Rename and working directory

`session.rename` and `session.cwd` SHALL update the session, append `session.renamed` or
`session.cwd`, and push the event to subscribers. `session.cwd` SHALL refuse a path that is
not an existing directory, and SHALL record the new directory's context.

#### Scenario: A rename is pushed to subscribers

- **WHEN** a subscribed client renames a session
- **THEN** it receives a `session.renamed` event carrying the new title

#### Scenario: A missing directory is refused

- **WHEN** `session.cwd` names a path that does not exist
- **THEN** it is refused with `-32602` and the session's directory is unchanged

#### Scenario: Moving a session records what applies there

- **WHEN** a session is moved to another directory
- **THEN** a `context.loaded` event follows, describing the new directory

### Requirement: Deletion is a tombstone

`session.delete` SHALL move the session to `deleted`, append `session.state` with the reason,
and retain the event log. It SHALL be refused with `-32004` while a turn is in flight. There
SHALL be no purge.

#### Scenario: Deleting keeps the history

- **WHEN** a session is deleted and listed with `include_deleted`
- **THEN** it appears as `deleted` and its events still replay

#### Scenario: A busy session is not deleted

- **WHEN** `session.delete` is called during a turn
- **THEN** it is refused with `-32004` and the session stays active

### Requirement: Restore

`session.restore` SHALL bring a deleted session back to `active`, appending `session.state`
with the reason and announcing the change. Restoring a session that is not deleted SHALL
succeed without appending anything.

#### Scenario: A deleted session is brought back

- **WHEN** `session.restore` is called on a deleted session
- **THEN** the state becomes `active`, the transition is recorded, and every window is told

#### Scenario: Restoring a live session is not an error

- **WHEN** `session.restore` is called on an active session
- **THEN** it succeeds and no event is appended

### Requirement: Fork is not yet implemented

`session.fork` SHALL answer `-32014` with a message naming the milestone, so a client can
distinguish an unbuilt method from an unknown one.

#### Scenario: Fork is refused as not yet

- **WHEN** `session.fork` is called
- **THEN** the code is `-32014` and the message says which milestone brings it

### Requirement: Usage accounting

`session.usage` SHALL sum the prompt, completion and cached token counts of every
`llm.completed` event in the session. `cost_micros` SHALL be absent when the endpoint
prices nothing.

#### Scenario: Usage sums the turn's reported figures

- **WHEN** a session has completed turns with reported usage
- **THEN** `session.usage` reports the totals, with cached tokens counted separately

#### Scenario: An unpriced endpoint claims no cost

- **WHEN** no pricing is available for the model
- **THEN** `cost_micros` is absent rather than zero
