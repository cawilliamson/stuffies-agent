# turns

## Purpose

One turn: the unit of work a session runs. Starting, steering, cancelling, timing out and
resuming it, and the queue that holds what the operator types while it runs.

## Requirements

### Requirement: One turn at a time

A session SHALL run at most one turn at a time. `turn.start` on a session with a turn in
flight SHALL be refused with `-32004`, and the check and the slot reservation SHALL happen
under one lock so two callers cannot both find the session idle.

#### Scenario: A second turn is refused, not queued

- **WHEN** `turn.start` is called while a turn is running
- **THEN** it is refused with `-32004` and the running turn is unaffected

#### Scenario: The slot is released when the turn ends

- **WHEN** a turn completes, is cancelled or times out
- **THEN** a later `turn.start` on that session is accepted

### Requirement: Turn start records what will run

`turn.start` SHALL accept a prompt, an optional `detach`, an optional `continue_final` and
optional images, and SHALL append `turn.started` carrying `turn_id`, the prompt, the model
and effort the turn will actually use, `detach`, the requesting client, `resumed_from` when
it resumes, and `continue_final` when set. Fields that do not apply SHALL be absent rather
than null.

#### Scenario: The log names the model that produced the message

- **WHEN** a turn is started
- **THEN** `turn.started` carries the model and effort the turn uses, not what the session
  is set to at replay time

#### Scenario: An image is carried in the log

- **WHEN** a turn is started with an image attachment
- **THEN** `turn.started` carries the media type, the base64 data and the file name

#### Scenario: An interrupted answer is continued rather than restarted

- **WHEN** a turn is started with `continue_final`
- **THEN** the transcript ends on the assistant message and no new user prompt is appended

### Requirement: Turn completion and context metering

A completed turn SHALL append `turn.completed` carrying the finish reason, the usage the
endpoint reported — prompt, completion and cached tokens — and the context figure against
the model's advertised limit. Each round-trip that reports a prompt count SHALL append
`turn.context`.

#### Scenario: Completion carries usage and context

- **WHEN** a turn runs to its own end
- **THEN** `turn.completed` carries the usage and a context block with the tokens used and
  the limit, absent when the endpoint advertised no limit

#### Scenario: The meter follows the conversation

- **WHEN** a turn makes several round-trips
- **THEN** each appends `turn.context` with the prompt count the endpoint reported

#### Scenario: A turn with no tool calls ends immediately

- **WHEN** the model answers without calling a tool
- **THEN** the turn completes and no further request is made

### Requirement: Cancellation

`turn.cancel` SHALL set a flag the turn observes within 25 milliseconds, drop any in-flight
tool future, kill a running command's whole process group, and append `turn.interrupted`
with cause `cancelled`. A cancelled stream SHALL still record the text it produced.

#### Scenario: Escape stops a running command

- **WHEN** a turn running a long shell command is cancelled
- **THEN** the process group is killed, including children the command backgrounded

#### Scenario: Cancelling records what was produced

- **WHEN** a turn is cancelled after the model streamed text
- **THEN** that text is in the log as `llm.completed` with stop reason `cancelled`

#### Scenario: Cancelling an idle session is not an error

- **WHEN** `turn.cancel` is called with no turn running
- **THEN** it succeeds and nothing is appended

### Requirement: Turn timeout

A turn that produces no terminal event within `SA_TURN_TIMEOUT` seconds, default 1800, SHALL
be interrupted with cause naming the timeout, and the session SHALL remain usable.

#### Scenario: A stalled stream is interrupted

- **WHEN** the provider never completes a turn
- **THEN** the turn is interrupted at the timeout and the cause records the duration

#### Scenario: A stalled auxiliary call does not block the turn

- **WHEN** a title or compaction call stalls
- **THEN** it is abandoned after `SA_AUX_BUDGET` seconds, default 20, and the turn proceeds

### Requirement: Turn status

`turn.status` SHALL report `running` with the turn id and the time it started, or `idle` with
the current time and no turn id.

#### Scenario: Status names the running turn

- **WHEN** a turn is running and `turn.status` is asked
- **THEN** the result carries that turn id, state `running` and its start time

### Requirement: Turn resume

`turn.resume` SHALL re-drive the most recent interrupted turn, appending a new `turn.started`
with `resumed_from` set to the interrupted turn's id and no client. It SHALL refuse when the
last turn event is not an interruption.

#### Scenario: An interrupted turn is re-driven explicitly

- **WHEN** `turn.resume` is called on a session whose last turn event is `turn.interrupted`
- **THEN** a new turn starts, `resumed_from` names the interrupted turn, and `detach` is true

#### Scenario: Nothing resumes on its own

- **WHEN** a session is interrupted and no client asks
- **THEN** no turn starts

#### Scenario: A completed turn is not resumed

- **WHEN** `turn.resume` is called on a session whose last turn completed
- **THEN** it is refused with `-32602`

### Requirement: Steering queue

`turn.steer` SHALL queue a message for delivery between model round-trips, one message per
round-trip, and SHALL answer `disposition: queued`. Delivery SHALL append `turn.steered` so
the transcript records the message where the model saw it. Steering SHALL require a running
turn.

#### Scenario: A redirect lands between round-trips

- **WHEN** a message is steered during a turn that is making tool calls
- **THEN** the next request carries it as a user message and `turn.steered` records it

#### Scenario: One message is delivered at a time

- **WHEN** two messages are steered before the next round-trip
- **THEN** only the first is delivered on that round-trip

#### Scenario: Steering an idle session is refused

- **WHEN** `turn.steer` is called with no turn running
- **THEN** it is refused with `-32005`

### Requirement: Follow-up queue

`turn.follow_up` SHALL queue a message that starts a new turn after the running one ends,
and SHALL outlive the turn it was typed in: a cancelled or timed-out turn SHALL NOT discard
it.

#### Scenario: A follow-up becomes the next turn

- **WHEN** a follow-up is queued and the running turn completes
- **THEN** a new turn starts with that prompt in the same session

#### Scenario: A cancelled turn does not discard the follow-up

- **WHEN** a follow-up is queued and the running turn is cancelled
- **THEN** the follow-up still starts a turn

### Requirement: Queue visibility

Every change to a queue SHALL append `turn.queued` carrying the complete current steering and
follow-up lists, which is the only way a view learns what is pending.

#### Scenario: A client that attached mid-turn can render the queue

- **WHEN** a message is queued while a turn runs
- **THEN** a replay of the session contains a `turn.queued` event with both lists in full

#### Scenario: A delivery empties the entry on screen

- **WHEN** a queued message is delivered to the model
- **THEN** a `turn.queued` event follows recording the queue without it

### Requirement: Queue removal

`turn.dequeue` SHALL empty both queues and return their text. `turn.queue_drop` SHALL remove
one message by queue name and index. An index past the end SHALL NOT be an error; an unknown
queue name SHALL be refused with `-32602`.

#### Scenario: Escape takes the queue back

- **WHEN** `turn.dequeue` is called
- **THEN** both lists are returned and both are now empty

#### Scenario: Editing a queued message

- **WHEN** `turn.queue_drop` removes index zero of `steering`
- **THEN** the result is the remaining queue and the dropped text is returned to the caller

#### Scenario: A stale index drops nothing

- **WHEN** `turn.queue_drop` names an index past the end of the queue
- **THEN** the call succeeds and the queue is unchanged

#### Scenario: An unknown queue is a client bug

- **WHEN** `turn.queue_drop` names a queue other than `steering` or `follow_up`
- **THEN** it is refused with `-32602`

### Requirement: Sequential tool calls

Tool calls within a turn SHALL run sequentially, and each SHALL append `tool.requested`
before it runs and exactly one of `tool.completed` or `tool.failed` afterwards.

#### Scenario: Calls do not overlap

- **WHEN** the model asks for three tool calls in one round
- **THEN** each is announced, run and terminated before the next is announced

#### Scenario: A failing tool does not end the turn

- **WHEN** a tool fails
- **THEN** `tool.failed` records the error and class, the model is given the error as the
  result, and the turn continues
