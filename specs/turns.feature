# turns
#
# One turn: the unit of work a session runs. Starting, steering, cancelling, timing out and
# resuming it, and the queue that holds what the operator types while it runs.

Feature: turns
  One turn: the unit of work a session runs. Starting, steering, cancelling, timing out and
  resuming it, and the queue that holds what the operator types while it runs.

  # Requirement: One turn at a time
  # A session SHALL run at most one turn at a time. `turn.start` on a session with a turn in flight
  # SHALL be refused with `-32004`, and the check and the slot reservation SHALL happen under one
  # lock so two callers cannot both find the session idle.
  @req:one-turn-at-a-time
  Scenario: A second turn is refused, not queued
    When `turn.start` is called while a turn is running
    Then it is refused with `-32004` and the running turn is unaffected
  @req:one-turn-at-a-time
  Scenario: The slot is released when the turn ends
    When a turn completes, is cancelled or times out
    Then a later `turn.start` on that session is accepted

  # Requirement: Turn start records what will run
  # `turn.start` SHALL accept a prompt, an optional `detach`, an optional `continue_final` and
  # optional images, and SHALL append `turn.started` carrying `turn_id`, the prompt, the model and
  # effort the turn will actually use, `detach`, the requesting client, `resumed_from` when it
  # resumes, and `continue_final` when set. Fields that do not apply SHALL be absent rather than
  # null.
  @req:turn-start-records-what-will-run
  Scenario: The log names the model that produced the message
    When a turn is started
    Then `turn.started` carries the model and effort the turn uses, not what the session is set to at replay time
  @req:turn-start-records-what-will-run
  Scenario: An image is carried in the log
    When a turn is started with an image attachment
    Then `turn.started` carries the media type, the base64 data and the file name
  @req:turn-start-records-what-will-run
  Scenario: An interrupted answer is continued rather than restarted
    When a turn is started with `continue_final`
    Then the transcript ends on the assistant message and no new user prompt is appended

  # Requirement: Turn completion and context metering
  # A completed turn SHALL append `turn.completed` carrying the finish reason, the usage the
  # endpoint reported — prompt, completion and cached tokens — and the context figure against the
  # model's advertised limit. Each round-trip that reports a prompt count SHALL append
  # `turn.context`.
  @req:turn-completion-and-context-metering
  Scenario: Completion carries usage and context
    When a turn runs to its own end
    Then `turn.completed` carries the usage and a context block with the tokens used and the limit, absent when the endpoint advertised no limit
  @req:turn-completion-and-context-metering
  Scenario: The meter follows the conversation
    When a turn makes several round-trips
    Then each appends `turn.context` with the prompt count the endpoint reported
  @req:turn-completion-and-context-metering
  Scenario: A turn with no tool calls ends immediately
    When the model answers without calling a tool
    Then the turn completes and no further request is made

  # Requirement: Cancellation
  # `turn.cancel` SHALL set a flag the turn observes within 25 milliseconds, drop any in-flight tool
  # future, kill a running command's whole process group, and append `turn.interrupted` with cause
  # `cancelled`. A cancelled stream SHALL still record the text it produced.
  @req:cancellation
  Scenario: Escape stops a running command
    When a turn running a long shell command is cancelled
    Then the process group is killed, including children the command backgrounded
  @req:cancellation
  Scenario: Cancelling records what was produced
    When a turn is cancelled after the model streamed text
    Then that text is in the log as `llm.completed` with stop reason `cancelled`
  @req:cancellation
  Scenario: Cancelling an idle session is not an error
    When `turn.cancel` is called with no turn running
    Then it succeeds and nothing is appended

  # Requirement: Turn timeout
  # A turn that produces no terminal event within `SA_TURN_TIMEOUT` seconds, default 1800, SHALL be
  # interrupted with cause naming the timeout, and the session SHALL remain usable.
  @req:turn-timeout
  Scenario: A stalled stream is interrupted
    When the provider never completes a turn
    Then the turn is interrupted at the timeout and the cause records the duration
  @req:turn-timeout
  Scenario: A stalled auxiliary call does not block the turn
    When a title or compaction call stalls
    Then it is abandoned after `SA_AUX_BUDGET` seconds, default 20, and the turn proceeds

  # Requirement: Turn status
  # `turn.status` SHALL report `running` with the turn id and the time it started, or `idle` with
  # the current time and no turn id.
  @req:turn-status
  Scenario: Status names the running turn
    When a turn is running and `turn.status` is asked
    Then the result carries that turn id, state `running` and its start time

  # Requirement: Turn resume
  # `turn.resume` SHALL re-drive the most recent interrupted turn, appending a new `turn.started`
  # with `resumed_from` set to the interrupted turn's id and no client. It SHALL refuse when the
  # last turn event is not an interruption.
  @req:turn-resume
  Scenario: An interrupted turn is re-driven explicitly
    When `turn.resume` is called on a session whose last turn event is `turn.interrupted`
    Then a new turn starts, `resumed_from` names the interrupted turn, and `detach` is true
  @req:turn-resume
  Scenario: Nothing resumes on its own
    When a session is interrupted and no client asks
    Then no turn starts
  @req:turn-resume
  Scenario: A completed turn is not resumed
    When `turn.resume` is called on a session whose last turn completed
    Then it is refused with `-32602`

  # Requirement: Steering queue
  # `turn.steer` SHALL queue a message for delivery between model round-trips, one message per
  # round-trip, and SHALL answer `disposition: queued`. Delivery SHALL append `turn.steered` so the
  # transcript records the message where the model saw it. Steering SHALL require a running turn.
  @req:steering-queue
  Scenario: A redirect lands between round-trips
    When a message is steered during a turn that is making tool calls
    Then the next request carries it as a user message and `turn.steered` records it
  @req:steering-queue
  Scenario: One message is delivered at a time
    When two messages are steered before the next round-trip
    Then only the first is delivered on that round-trip
  @req:steering-queue
  Scenario: Steering an idle session is refused
    When `turn.steer` is called with no turn running
    Then it is refused with `-32005`

  # Requirement: Follow-up queue
  # `turn.follow_up` SHALL queue a message that starts a new turn after the running one ends, and
  # SHALL outlive the turn it was typed in: a cancelled or timed-out turn SHALL NOT discard it.
  @req:follow-up-queue
  Scenario: A follow-up becomes the next turn
    When a follow-up is queued and the running turn completes
    Then a new turn starts with that prompt in the same session
  @req:follow-up-queue
  Scenario: A cancelled turn does not discard the follow-up
    When a follow-up is queued and the running turn is cancelled
    Then the follow-up still starts a turn

  # Requirement: Queue visibility
  # Every change to a queue SHALL append `turn.queued` carrying the complete current steering and
  # follow-up lists, which is the only way a view learns what is pending.
  @req:queue-visibility
  Scenario: A client that attached mid-turn can render the queue
    When a message is queued while a turn runs
    Then a replay of the session contains a `turn.queued` event with both lists in full
  @req:queue-visibility
  Scenario: A delivery empties the entry on screen
    When a queued message is delivered to the model
    Then a `turn.queued` event follows recording the queue without it

  # Requirement: Queue removal
  # `turn.dequeue` SHALL empty both queues and return their text. `turn.queue_drop` SHALL remove one
  # message by queue name and index. An index past the end SHALL NOT be an error; an unknown queue
  # name SHALL be refused with `-32602`.
  @req:queue-removal
  Scenario: Escape takes the queue back
    When `turn.dequeue` is called
    Then both lists are returned and both are now empty
  @req:queue-removal
  Scenario: Editing a queued message
    When `turn.queue_drop` removes index zero of `steering`
    Then the result is the remaining queue and the dropped text is returned to the caller
  @req:queue-removal
  Scenario: A stale index drops nothing
    When `turn.queue_drop` names an index past the end of the queue
    Then the call succeeds and the queue is unchanged
  @req:queue-removal
  Scenario: An unknown queue is a client bug
    When `turn.queue_drop` names a queue other than `steering` or `follow_up`
    Then it is refused with `-32602`

  # Requirement: Sequential tool calls
  # Tool calls within a turn SHALL run sequentially, and each SHALL append `tool.requested` before
  # it runs and exactly one of `tool.completed` or `tool.failed` afterwards.
  @req:sequential-tool-calls
  Scenario: Calls do not overlap
    When the model asks for three tool calls in one round
    Then each is announced, run and terminated before the next is announced
  @req:sequential-tool-calls
  Scenario: A failing tool does not end the turn
    When a tool fails
    Then `tool.failed` records the error and class, the model is given the error as the result, and the turn continues
