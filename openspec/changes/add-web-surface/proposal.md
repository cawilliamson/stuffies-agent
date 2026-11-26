# Proposal

## Why

`sa` serves two workloads. The coding one is built: a line-oriented CLI and a terminal front
end driving an agent against a working tree. The knowledge one is not, and it is the surface
that makes a long-running daemon worth having: durable memory, browsable from a browser,
answering questions without a terminal attached.

The web surface is the general one — sessions, live transcripts, cost and manual compaction —
and it is what a knowledge browser is built on. It is scheduled at v2.0 and nothing of it
exists: there is no `sa-web` crate and no HTTP listener.

## What Changes

- A loopback HTTP carrier served by the daemon itself: `POST /rpc`, `GET /events`,
  `GET /`, `GET /assets/*`, `POST /login`, `POST /logout`.
- Token authentication with a session cookie held server-side, a mandatory `X-SA-Token`
  header on every mutating request, origin checking, and rate limiting on login.
- Server-sent events with `Last-Event-ID` mapped onto `from_seq`, so a reconnect replays
  exactly what was missed.
- Hand-written ES modules and CSS embedded in the binary, with no npm and no build step.
- A browser UI: session list and creation, live transcript, usage display, turn cancellation
  and a manual compaction control.
- Manual compaction, recorded as `trigger: manual`.

Out of scope: TLS termination (a reverse proxy does it); websockets; a browser-based editor;
multi-tenant deployment (isolation stays a knowledge namespace concern); and any build step
beyond `rust-embed`.

## Capabilities

### New Capabilities

- `web-surface`: the HTTP carrier, its authentication, its event stream and the embedded UI
  it serves.

### Modified Capabilities

- `protocol`: `GET /events` carries the same events and filter semantics the socket carries;
  no method gains a second implementation.
- `daemon-lifecycle`: the daemon hosts a second listener, enabled only by configuration, and
  stopping it does not stop a turn.
- `compaction`: `trigger: manual` joins `trigger: auto`.
- `engineering-constraints`: the `sa-web` crate arrives and its dependency rules become live.

## Impact

- New crate `sa-web`, depended on by `sa-daemon` and by nothing else in the workspace.
- `web.toml` under the config directory, whose presence is what enables the listener.
- A `web.token` file under the state directory, mode `0600`, generated on first start.
- `web_sessions` table becomes used rather than reserved.
- The dependency-rule gate's `sa-web` branch becomes live instead of skipped.
