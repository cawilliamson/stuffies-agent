# compaction

## Purpose

Keeping a long session inside the model's context window by summarising the oldest turns,
without removing anything the user can see.

## Requirements

### Requirement: Compaction is automatic before a turn

The daemon SHALL check the session's context usage before a turn starts and SHALL compact
when it reaches 80% of the model's advertised context limit. When the endpoint advertises no
limit, compaction SHALL NOT fire on a guess.

#### Scenario: A long session is compacted before it overflows

- **WHEN** the session's reported context reaches the threshold and another turn starts
- **THEN** a `compaction.applied` event is appended before the turn's first request

#### Scenario: No advertised limit means no compaction

- **WHEN** the model's context limit is unknown
- **THEN** no compaction is attempted

### Requirement: What is retained

A compaction SHALL retain at least the last 4 complete turns verbatim and SHALL summarise
only events before them. A session with 4 or fewer turns SHALL NOT be compacted. It SHALL
fold enough turns that what remains fits the window: a compaction that leaves the retained
turns over the trigger fires again on the next turn and never gets anywhere. It SHALL fold
only events the previous compaction did not already cover, and SHALL NOT summarise the same
range twice.

#### Scenario: The recent turns stay verbatim

- **WHEN** a compaction runs
- **THEN** every event from the last four turns onward is outside the summarised range

#### Scenario: A short session is left alone

- **WHEN** the session has four or fewer turns
- **THEN** no compaction event is appended

#### Scenario: A compaction is not repeated

- **WHEN** a session is checked again with nothing new before the retained turns
- **THEN** no summarisation call is made and no event is appended

#### Scenario: The folded range advances

- **WHEN** a second compaction runs after new turns have pushed the view over the trigger
- **THEN** it summarises from where the last one reached, not from the top again

### Requirement: The summarisation is recorded

`compaction.applied` SHALL carry `from_seq`, `to_seq`, the summary, the token counts before
and after, `trigger: auto`, and the summarisation's own usage. Summarisation SHALL be a
normal model call whose cost is recorded separately from turn cost.

#### Scenario: The summary is an event

- **WHEN** a compaction runs
- **THEN** the event carries the summary text, the range it replaced and the usage of the
  call that produced it

#### Scenario: An empty summary changes nothing

- **WHEN** the summarisation returns no text
- **THEN** no event is appended and the log is unchanged

### Requirement: The log keeps everything

Compaction SHALL change only what the model sees. Every original event SHALL remain in the
log, and a replay or a re-attach SHALL show the full transcript.

#### Scenario: A user still sees the whole session

- **WHEN** a compacted session is replayed
- **THEN** every original event is present, including those inside the summarised range

#### Scenario: The model sees the summary and what followed

- **WHEN** a turn runs after a compaction
- **THEN** its request carries the summary and the events after `to_seq`, not the summarised
  events themselves

### Requirement: Compaction is crash-safe

Compaction SHALL be a normal event append, so a crash during one leaves a consistent log and
the next call retries.

#### Scenario: A crash during compaction loses nothing

- **WHEN** the daemon dies while a summarisation is in flight
- **THEN** the log holds no partial compaction and the next turn may compact again

### Requirement: An oversized single turn fails honestly

When one turn's own events exceed the context limit, compaction cannot help: the turn SHALL
fail with `context_exhausted` naming the turn.

#### Scenario: The turn that cannot fit is named

- **WHEN** a single turn exceeds the window on its own
- **THEN** it fails with `-32009` and the turn id, rather than being silently cut
