# client-library

## Purpose

`sa-client` is the only thing a client needs: connect to the daemon, handshake, call a
method, and read frames. It holds a cursor, never authority.

## Requirements

### Requirement: Connection and handshake

The library SHALL connect to a daemon socket, send `hello` with the client's own name, and
expose the daemon's `HelloResult`. It SHALL refuse to proceed when the handshake fails,
surfacing the daemon's error verbatim.

#### Scenario: A handshake reports the daemon's identity

- **WHEN** a client connects to a live daemon and handshakes
- **THEN** it receives the protocol version, the daemon version and the build commit

#### Scenario: A failed handshake is reported as the daemon said it

- **WHEN** the daemon refuses the handshake
- **THEN** the library returns the daemon's error message unchanged

#### Scenario: An absent daemon is a connection error

- **WHEN** nothing is listening on the socket path
- **THEN** connecting fails with the path named, rather than hanging

### Requirement: Calls and notifications

The library SHALL send a request and await the response with the matching `id`, and SHALL
expose frames that are not responses — notifications — separately so a caller can pump them
while a request is outstanding.

#### Scenario: A call returns its own response

- **WHEN** two calls are made with different ids and answered out of order
- **THEN** each caller receives the response carrying its own id

#### Scenario: A notification is not mistaken for a response

- **WHEN** an `event` notification arrives between a request and its response
- **THEN** the notification is available to the caller and the request still resolves

### Requirement: No authority

The library SHALL hold no session state of its own beyond a cursor. It SHALL NOT decide
session ownership, filter the log, or cache anything the daemon is the source of truth for.

#### Scenario: A reconnecting client resumes from its cursor

- **WHEN** a client reconnects after being away and replays from its last acknowledged `seq`
- **THEN** it receives exactly the events it missed, with no local reconstruction
