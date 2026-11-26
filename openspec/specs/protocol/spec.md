# protocol

## Purpose

The wire contract between a client and the daemon: framing, the mandatory handshake, the
method table, notification delivery and the error vocabulary. Every transport carries this
contract and none of them reinterprets it.

## Requirements

### Requirement: Error vocabulary

The daemon SHALL answer a failed request with a JSON-RPC `error` member carrying a code from
one of two ranges: the reserved JSON-RPC codes `-32700` parse, `-32600` invalid request,
`-32601` method not found, `-32602` invalid params and `-32603` internal; or the application
range from `-32000` for version mismatch, peer uid mismatch, line too long, handshake
required, session busy, session not found, stale owner, workspace diverged, workspace
unreachable, context exhausted, bundle invalid, knowledge denied, knowledge unavailable and
not implemented.

The `data` member SHALL be omitted rather than sent as null.

#### Scenario: An unhandled method is named in the error

- **WHEN** a client calls a method the dispatcher does not know
- **THEN** the response carries code `-32601` and the message names the method

#### Scenario: Application codes are distinct and in range

- **WHEN** the application codes are compared
- **THEN** no two share a value, and each lies in `-32099..=-32000`

#### Scenario: A version mismatch names both versions

- **WHEN** a `hello` carries a protocol version the daemon does not speak
- **THEN** the error carries code `-32000`, its message names both versions, and its data
  carries `daemon` and `client`

### Requirement: Framing

The daemon SHALL read and write newline-delimited UTF-8 JSON, one JSON-RPC message per line,
with a maximum line length of 1 MiB.

#### Scenario: An oversized line is a protocol error

- **WHEN** a client writes a line longer than 1 MiB
- **THEN** the daemon answers with code `-32002` and closes the connection

#### Scenario: Malformed JSON is a parse error

- **WHEN** a client writes a line that is not valid JSON
- **THEN** the daemon answers with code `-32700`

#### Scenario: The jsonrpc member is mandatory

- **WHEN** a request omits `jsonrpc`, or carries anything but `"2.0"`
- **THEN** it is rejected as an invalid request

### Requirement: Handshake gate

The first message on every connection SHALL be `hello`, carrying `protocol`, `client` and
`client_version`. Any other method first SHALL be refused with `-32003` before its own
arguments are read. A connection that has not completed `hello` within 5 seconds SHALL be
closed.

`hello` SHALL answer with the daemon's protocol version, its package version and the commit
it was built from. There is no negotiation and no best-effort compatibility.

#### Scenario: Ping before hello is refused

- **WHEN** a client sends `ping` as its first frame
- **THEN** the daemon answers `-32003` and the message says hello must come first

#### Scenario: A completed handshake answers with build identity

- **WHEN** a client sends a valid `hello`
- **THEN** the result carries `protocol`, `version` and `build_rev`

#### Scenario: A silent connection is closed

- **WHEN** a connection sends nothing for 5 seconds after being accepted
- **THEN** the daemon closes it

### Requirement: Method table

The dispatcher SHALL implement the following methods, and SHALL answer every other method
with `-32601`.

```text
checkpoint.list      checkpoint.rewind   events.replay        events.subscribe
events.unsubscribe   goal.set            goal.status          goal.stop
hello                lsp.list            mcp.list             model.effort
model.list           model.select        openspec.run         ping
session.attach       session.create      session.cwd          session.delete
session.fork         session.list        session.rename       session.restore
session.usage        turn.cancel         turn.dequeue         turn.follow_up
turn.queue_drop      turn.resume         turn.start           turn.status
turn.steer
```

`handoff.accept`, `handoff.export` and `handoff.import` SHALL answer `-32014` rather than
`-32601`, so a client can tell "not yet" from "no such method". `session.fork` SHALL answer
`-32014` for the same reason.

#### Scenario: An unimplemented milestone is distinguished from an unknown method

- **WHEN** a client calls `handoff.export`
- **THEN** the error code is `-32014`, not `-32601`

#### Scenario: A notification is never answered

- **WHEN** a request arrives without an `id`
- **THEN** the dispatcher performs it and writes no response frame

### Requirement: Notifications

The daemon SHALL push exactly two notification methods to connected clients: `event`, whose
params carry `session_id`, `seq`, `kind` and `payload`; and `sessions.changed`, whose params
are empty and which is sent to every connection when a session is created, deleted or
restored.

#### Scenario: An appended event reaches every subscriber of that session

- **WHEN** a client subscribes to a session and an event is appended to it
- **THEN** the subscriber receives an `event` notification carrying the same `seq` and payload

#### Scenario: A session change reaches connections that are not subscribed

- **WHEN** a session is created
- **THEN** every connected client receives `sessions.changed`

### Requirement: Subscription filtering

`events.subscribe` SHALL accept a filter whose only honoured member is `exclude_kinds`, a
list of event kind names matched exactly. A filter can only remove kinds from delivery; it
cannot add them. Filtering SHALL apply at send time and SHALL NOT affect what is persisted.

#### Scenario: An excluded kind is not delivered

- **WHEN** a subscription excludes `llm.delta` and the turn streams deltas
- **THEN** that subscriber receives no `llm.delta` notification while every other kind arrives

#### Scenario: The log keeps what the filter removed

- **WHEN** a replayed session is read after a filtered subscription
- **THEN** the excluded events are present in the replay

### Requirement: Pagination

`session.attach` and `events.replay` SHALL accept `limit`, default 500, clamped to the range
1 to 5000. When more events remain the result SHALL set `more` true and name the `next_seq`
to continue from. `next_seq` SHALL be the sequence of the last event returned, or the
requested `from_seq` when nothing was returned.

#### Scenario: A limit above the maximum is clamped

- **WHEN** a caller asks for 9000 events
- **THEN** at most 5000 are returned and `more` says whether any remain

#### Scenario: Continuing a replay resumes where it stopped

- **WHEN** a caller replays with `from_seq` set to the previous `next_seq`
- **THEN** the events returned continue from that sequence with no gap and no repeat

### Requirement: Subscribe does not replay

`events.subscribe` SHALL register delivery only, and SHALL answer with a `subscription_id`
and the `from_seq` it was given. A client that needs history SHALL call `session.attach` or
`events.replay` itself.

#### Scenario: Subscribing delivers future events only

- **WHEN** a client subscribes to a session that already has history
- **THEN** it receives nothing until the next event is appended

### Requirement: Protocol versioning

The protocol version SHALL be a single integer, `1`, carried by `hello` and by `ping`. A
breaking change to framing, the method table, an error code or a payload shape SHALL bump it.
Adding an event kind SHALL NOT.

#### Scenario: Ping reports the protocol version

- **WHEN** a handshaken client calls `ping`
- **THEN** the result carries `version`, `build_rev`, `uptime_seconds` and the session count
