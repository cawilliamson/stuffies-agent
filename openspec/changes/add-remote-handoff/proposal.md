# Proposal

## Why

The workflow this project exists to serve is: start work on a laptop, hand the session to a
remote daemon before shutting the laptop down, let it run unattended overnight, and hand it
back in the morning. Everything needed for that is built except the transfer itself.

`handoff.export`, `handoff.import` and `handoff.accept` currently answer `-32014`, and
`session.fork` does the same. A session cannot yet leave the host that made it.

## What Changes

- `handoff.export` writes a canonical bundle: a single container holding the manifest, the
  event log, the workspace patch and the untracked files, written atomically.
- `handoff.import` verifies a bundle completely before mutating anything, then applies it in
  one transaction. A rejected bundle leaves the target daemon unchanged.
- `handoff.accept` completes a move by confirming the target's ownership epoch.
- `session.fork` copies a session into a new workspace, with its own epoch lineage.
- Epoch fencing: a session records `{host_fingerprint, epoch}`, and a write from a stale
  epoch is refused rather than merged. Fencing uses the counter, never the clock.
- A workspace descriptor travels in the bundle and is applied on import, with the three
  documented cases for a matching repository, a clonable remote, and an unreachable tree.
- Secrets are refused at export, with an explicit override that is recorded.
- Bundles move over a dedicated `ssh <host> sa handoff --receive` process streaming raw
  bytes, not over the JSON-RPC channel.

Out of scope: automatic merge of divergent histories (a fork diverges deliberately, there is
no merge); knowledge transfer (see `add-knowledge-store`); `nix develop` integration on the
remote host; submodules; and physical purge of a moved-away session.

## Capabilities

### New Capabilities

- `handoff`: bundle export, verified import, epoch fencing, workspace descriptor and the
  transport that carries a bundle between hosts.

### Modified Capabilities

- `protocol`: `handoff.accept`, `handoff.export`, `handoff.import` and `session.fork` stop
  answering `-32014` and gain their real results.
- `session-lifecycle`: a session gains a `parent_id`, moves through `quiescing`,
  `relocating` and `relocated`, and `session.fork` creates a distinct writable session.
- `workspace-handling`: the workspace lock also covers export, and a rewind and a checkpoint
  become the recovery point a handoff relies on.

## Impact

- New module in `sa-core` for the bundle format and its verification.
- New subcommand `sa handoff` with `--receive` and `--send`.
- `sessions` table gains `parent_id` and epoch columns already present but unused.
- New error paths surfaced for the first time: `-32006` stale owner, `-32007` workspace
  diverged, `-32008` workspace unreachable, `-32011` bundle invalid.
- `sa-client` gains no new API: a bundle never travels over the RPC channel.
