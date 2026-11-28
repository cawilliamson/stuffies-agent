# Spec Delta

## Purpose

The HTTP carrier and the browser surface the daemon serves: authentication, the event stream,
the embedded assets and the operations a browser can perform.

## ADDED Requirements

### Requirement: The listener is off unless configured

The daemon SHALL serve HTTP only when `$XDG_CONFIG_HOME/stuffies-agent/web.toml` exists. The
default bind SHALL be loopback `127.0.0.1:7420`, overridable in that file. TLS SHALL be
terminated by a reverse proxy, never by the daemon.

#### Scenario: No config means no listener

- **WHEN** `web.toml` is absent
- **THEN** no port is bound and nothing else changes

#### Scenario: Only loopback is bound by default

- **WHEN** the listener starts with no address configured
- **THEN** it binds `127.0.0.1:7420` and no other interface

### Requirement: HTTP carries the same protocol

`POST /rpc` SHALL accept one JSON-RPC request and return one JSON-RPC response through the
same dispatcher the socket transport uses. The HTTP carrier SHALL contain no protocol logic
of its own.

#### Scenario: A method is not implemented twice

- **WHEN** a method is called over HTTP
- **THEN** the dispatcher handles it and the carrier only frames the answer

#### Scenario: An unknown method behaves as it does on the socket

- **WHEN** an unknown method is called over HTTP
- **THEN** the response carries `-32601`, exactly as the socket would answer

### Requirement: Event stream

`GET /events` SHALL serve server-sent events, one stream per subscription, and SHALL map a
reconnect's `Last-Event-ID` onto `from_seq` so exactly the missed events are replayed. The
subscription filter SHALL be the same shape the socket transport accepts.

#### Scenario: A reconnect replays exactly what was missed

- **WHEN** a browser reconnects with `Last-Event-ID` set to the last event it saw
- **THEN** it receives the events after that one, with no gap and no repeat

#### Scenario: A closed browser does not stop a turn

- **WHEN** the browser disconnects mid-turn
- **THEN** the turn continues and the reconnect replays what it missed

### Requirement: Token exchange

On first start with the listener enabled the daemon SHALL generate
`$XDG_STATE_HOME/stuffies-agent/web.token`, 32 bytes hex, mode `0600`. `POST /login {token}`
SHALL compare in constant time and, on success, set `sa_session`: 32 bytes of randomness,
`HttpOnly`, `SameSite=Strict`, `Secure`, 30-day expiry, held server-side in `web_sessions`.

The URL token SHALL be exchanged once and SHALL NOT be a bearer credential for `/rpc`.

#### Scenario: A wrong token is rejected

- **WHEN** `POST /login` carries a token that does not match
- **THEN** no cookie is set and the response says so

#### Scenario: A leaked URL is not a leak

- **WHEN** a URL carrying the token is shared after login
- **THEN** requests authenticated by the cookie continue to work and the token alone does not

#### Scenario: Login is rate limited

- **WHEN** a peer makes more than five login attempts in a minute
- **THEN** further attempts are rejected until the minute passes

#### Scenario: Rotation invalidates sessions

- **WHEN** `web.token` is regenerated
- **THEN** every `web_sessions` row is invalidated and browsers must log in again

### Requirement: CSRF defence

Every mutating request SHALL carry `X-SA-Token` with the session value, and its absence SHALL
be answered 403 with no state change. `Origin` and `Referer`, when present, SHALL match the
configured origin list.

#### Scenario: A cross-origin post changes nothing

- **WHEN** a form post arrives without `X-SA-Token`
- **THEN** the response is 403 and nothing is written

#### Scenario: A mismatched origin is rejected

- **WHEN** `Origin` names a host not in the configured list
- **THEN** the request is rejected

### Requirement: Forwarded headers are only trusted from loopback

`X-Forwarded-For` SHALL be honoured only when the immediate peer is loopback. Otherwise it
SHALL be ignored, a warning SHALL be logged, and the peer address SHALL be used.

#### Scenario: A remote peer cannot spoof its address

- **WHEN** a non-loopback peer sends `X-Forwarded-For`
- **THEN** the header is ignored and the real peer address is used

### Requirement: Embedded assets

The application shell and its assets SHALL be hand-written ES modules and CSS embedded with
`rust-embed` and served from the binary. There SHALL be no npm and no JavaScript build step.

#### Scenario: The daemon serves the UI with no toolchain

- **WHEN** the binary is run on a machine with no node installed
- **THEN** the UI is served and works

### Requirement: Browser operations

The UI SHALL list and create sessions, render a live transcript, show usage, cancel a running
turn, and offer manual compaction. A manual compaction SHALL record
`compaction.applied` with `trigger: manual` on the same event shape as the automatic path.

#### Scenario: A running session is visible

- **WHEN** a turn runs
- **THEN** its events appear in the browser's transcript as they are appended

#### Scenario: A turn can be cancelled from the browser

- **WHEN** the cancel control is used
- **THEN** the turn is interrupted and the interruption is recorded

#### Scenario: Manual compaction is labelled

- **WHEN** compaction is requested from the UI
- **THEN** the event carries `trigger: manual`

### Requirement: A browser holds no authority

A browser SHALL be an untrusted client with exactly the authority any other client has, which
is none. Authorisation SHALL be decided in the dispatcher, never in the UI.

#### Scenario: The UI cannot widen access

- **WHEN** a browser requests something the session is not allowed
- **THEN** the dispatcher refuses it and the UI reports the refusal
