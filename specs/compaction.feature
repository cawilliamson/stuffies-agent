# compaction
#
# Keeping a long session inside the model's context window by summarising the oldest turns,
# without removing anything the user can see.

Feature: compaction
  Keeping a long session inside the model's context window by summarising the oldest turns,
  without removing anything the user can see.

  # Requirement: Compaction is automatic before a turn
  # The daemon SHALL check the session's context usage before a turn starts and SHALL compact when
  # it reaches 80% of the model's advertised context limit. When the endpoint advertises no limit,
  # compaction SHALL NOT fire on a guess.
  @req:compaction-is-automatic-before-a-turn
  Scenario: A long session is compacted before it overflows
    When the session's reported context reaches the threshold and another turn starts
    Then a `compaction.applied` event is appended before the turn's first request
  @req:compaction-is-automatic-before-a-turn
  Scenario: No advertised limit means no compaction
    When the model's context limit is unknown
    Then no compaction is attempted

  # Requirement: What is retained
  # A compaction SHALL retain at least the last 4 complete turns verbatim and SHALL summarise only
  # events before them. A session with 4 or fewer turns SHALL NOT be compacted. It SHALL fold enough
  # turns that what remains fits the window: a compaction that leaves the retained turns over the
  # trigger fires again on the next turn and never gets anywhere. It SHALL fold only events the
  # previous compaction did not already cover, and SHALL NOT summarise the same range twice.
  @req:what-is-retained
  Scenario: The recent turns stay verbatim
    When a compaction runs
    Then every event from the last four turns onward is outside the summarised range
  @req:what-is-retained
  Scenario: A short session is left alone
    When the session has four or fewer turns
    Then no compaction event is appended
  @req:what-is-retained
  Scenario: A compaction is not repeated
    When a session is checked again with nothing new before the retained turns
    Then no summarisation call is made and no event is appended
  @req:what-is-retained
  Scenario: The folded range advances
    When a second compaction runs after new turns have pushed the view over the trigger
    Then it summarises from where the last one reached, not from the top again

  # Requirement: The summarisation is recorded
  # `compaction.applied` SHALL carry `from_seq`, `to_seq`, the summary, the token counts before and
  # after, `trigger: auto`, and the summarisation's own usage. Summarisation SHALL be a normal model
  # call whose cost is recorded separately from turn cost.
  @req:the-summarisation-is-recorded
  Scenario: The summary is an event
    When a compaction runs
    Then the event carries the summary text, the range it replaced and the usage of the call that produced it
  @req:the-summarisation-is-recorded
  Scenario: An empty summary changes nothing
    When the summarisation returns no text
    Then no event is appended and the log is unchanged

  # Requirement: The log keeps everything
  # Compaction SHALL change only what the model sees. Every original event SHALL remain in the log,
  # and a replay or a re-attach SHALL show the full transcript.
  @req:the-log-keeps-everything
  Scenario: A user still sees the whole session
    When a compacted session is replayed
    Then every original event is present, including those inside the summarised range
  @req:the-log-keeps-everything
  Scenario: The model sees the summary and what followed
    When a turn runs after a compaction
    Then its request carries the summary and the events after `to_seq`, not the summarised events themselves

  # Requirement: Compaction is crash-safe
  # Compaction SHALL be a normal event append, so a crash during one leaves a consistent log and the
  # next call retries.
  @req:compaction-is-crash-safe
  Scenario: A crash during compaction loses nothing
    When the daemon dies while a summarisation is in flight
    Then the log holds no partial compaction and the next turn may compact again

  # Requirement: An oversized single turn fails honestly
  # When one turn's own events exceed the context limit, compaction cannot help: the turn SHALL fail
  # with `context_exhausted` naming the turn.
  @req:an-oversized-single-turn-fails-honestly
  Scenario: The turn that cannot fit is named
    When a single turn exceeds the window on its own
    Then it fails with `-32009` and the turn id, rather than being silently cut
