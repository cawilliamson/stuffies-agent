# Tasks

## 1. Crate and carrier

- [ ] 1.1 Create `sa-web` with `sa-core` and `sa-proto` as its only workspace dependencies
- [ ] 1.2 Bind loopback only, from `web.toml`, and refuse to start when the file is absent
- [ ] 1.3 Serve `POST /rpc` as one JSON-RPC request to one response through the dispatcher
- [ ] 1.4 Serve `GET /events` as server-sent events, mapping `Last-Event-ID` onto `from_seq`
- [ ] 1.5 Apply the same subscription filter shape the socket transport accepts
- [ ] 1.6 Honour `X-Forwarded-For` only when the immediate peer is loopback

## 2. Authentication

- [ ] 2.1 Generate `web.token` at mode `0600` on first start when the listener is enabled
- [ ] 2.2 Implement `POST /login` with a constant-time comparison and a `sa_session` cookie
- [ ] 2.3 Require `X-SA-Token` on every mutating request and reject its absence with 403
- [ ] 2.4 Check `Origin` and `Referer` against the configured origins
- [ ] 2.5 Rate limit login attempts to five per minute per peer address
- [ ] 2.6 Invalidate every `web_sessions` row when the token rotates
- [ ] 2.7 Implement `POST /logout`

## 3. Assets

- [ ] 3.1 Embed the UI with `rust-embed` and serve it from `GET /` and `GET /assets/*`
- [ ] 3.2 Write the application shell and its ES modules with no build step
- [ ] 3.3 Serve with a content type per asset and no directory traversal

## 4. UI

- [ ] 4.1 List sessions and create one
- [ ] 4.2 Render a live transcript from the event stream
- [ ] 4.3 Render usage and context figures for the session
- [ ] 4.4 Cancel a running turn
- [ ] 4.5 Offer manual compaction and show the resulting event
- [ ] 4.6 Reconnect after a dropped stream and replay from the last event id

## 5. Manual compaction

- [ ] 5.1 Accept a manual compaction request through the dispatcher
- [ ] 5.2 Record `compaction.applied` with `trigger: manual` on the same event shape as the
      automatic path

## 6. Verification

- [ ] 6.1 Assert a wrong token is rejected and rate limited
- [ ] 6.2 Assert a cross-origin POST without `X-SA-Token` is 403 and changes nothing
- [ ] 6.3 Assert an SSE reconnect with `Last-Event-ID` replays exactly the missed events
- [ ] 6.4 Assert the UI renders a running session and cancels a turn
- [ ] 6.5 Assert the daemon serves the UI with no network egress
- [ ] 6.6 Assert the dependency gate fails if `sa-web` reaches `sa-daemon` or a terminal crate
