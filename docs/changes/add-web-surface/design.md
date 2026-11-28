# Design

## Context

The daemon owns session state and the dispatcher implements every protocol method.
Transports are carriers around it and contain no protocol logic. A browser is one more
untrusted client speaking the same protocol over a different carrier.

HTTP is a passive carrier: it holds no outbound connection, no reconnect state and no
rate-limit budget of its own. That is what lets it satisfy the dispatcher rule without
widening the daemon's failure domain the way a chat gateway would.

## Goals / Non-Goals

**Goals:**

- Serve a browser with the same authority any other client has, which is none.
- Reconnect a dropped event stream without a second reconnect protocol.
- Ship the UI inside the binary with no build step and no npm.
- Keep the listener off unless it is configured.

**Non-Goals:**

- TLS. A reverse proxy terminates it.
- Websockets.
- Rendering markdown, highlighting code, or editing files in the browser.
- Authorisation. Knowledge namespaces are decided in the dispatcher, never in the UI.

## Decisions

**Loopback only, disabled unless configured.** `127.0.0.1:7420` by default, overridable in
`web.toml`; the listener does not start unless that file exists. `X-Forwarded-For` is honoured
only when the immediate peer is loopback, and the peer address is the truth otherwise.

**Server-sent events, not websockets.** The browser needs one direction for events and a
plain request for everything else, and SSE reconnects with `Last-Event-ID`, which maps
directly onto `from_seq`. A websocket would duplicate a reconnect protocol the daemon already
has. Alternative considered: websockets. Rejected for that duplication.

**A token is exchanged once and is not a bearer credential.** `POST /login {token}` compares
in constant time against `web.token` and sets `sa_session`: 32 bytes of randomness,
`HttpOnly`, `SameSite=Strict`, `Secure`, 30-day expiry, held server-side. A leaked URL is not
a leak.

**CSRF defence is a custom header.** Every mutating request must carry `X-SA-Token` with the
session value. A cross-origin form post cannot set a custom header, so no per-form token
machinery is needed. `Origin` and `Referer`, when present, must match the configured origins.

**Login is rate limited per peer.** Five attempts per minute. A wrong token is rejected, and
rotation of `web.token` invalidates every `web_sessions` row.

**Assets are embedded.** Hand-written ES modules and CSS served from the binary with
`rust-embed`. No npm, no bundler. If a build step is ever genuinely required it is a
`build.rs`, not a package manager.

**Manual compaction is a button.** `trigger: manual` on the same event shape the automatic
path uses, so the log has one compaction format.

## Risks / Trade-offs

- **A browser is a second surface to keep in step.** The UI is a client of the same protocol,
  so a protocol change breaks it the same way it breaks the TUI. That is the intended cost.
- **`Secure` cookies over loopback.** A browser treats loopback as a secure context, so the
  cookie is accepted; a plain-HTTP non-loopback deployment behind a proxy still needs the
  proxy to present HTTPS.
- **Rate limiting is in-process.** A restart resets the budget, which is acceptable for a
  single-user daemon.
- **The listener widens the attack surface to loopback.** Peer address plus token plus header
  is the boundary, and it is narrower than an SSH session's.
