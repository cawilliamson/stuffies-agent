# Design

## Context

The daemon is the only writer of session state, and sqlite is authoritative at rest. A
handoff has to move that state to a host that has never seen it, without the source and the
target both believing they own the session.

Two constraints shape the design. The SSH channel is disposable, so the daemon must never be
a child of it. And the bundle is data, never code, so import cannot execute anything it
carries.

## Goals / Non-Goals

**Goals:**

- Move a session between hosts with its history intact and no possibility of two writers.
- Refuse, by name, anything the transfer cannot represent, rather than transferring it
  approximately.
- Keep secrets out of the bundle by construction, not by promise.
- Make a rejected import leave the target exactly as it was.

**Non-Goals:**

- Merging divergent histories. A fork is deliberate divergence with no merge.
- Transferring knowledge. A bundle carries a session, not the whole brain.
- A remote build environment. A pre-provisioned host is the contract.
- Submodules, and multi-repository trees.

## Decisions

**Move is the default; fork must be asked for.** Move quiesces at a turn boundary, writes a
checkpoint, exports, increments the epoch and becomes `relocated`, which refuses further
turns. Fork requires an explicit target workspace path that does not exist or is empty.
Alternative considered: a single mode with a `--fork` flag. Rejected because the failure mode
of an accidental fork is a silent divergence, and the failure mode of an accidental move is a
refusal.

**Fencing is an integer epoch, never a timestamp.** `owner = {host_fingerprint, epoch}`;
`host_fingerprint` is sha256 of `/etc/machine-id` when readable, otherwise a UUID generated
once and stored under the state directory at mode `0600`. A write is accepted only when the
daemon's epoch equals the session's, and staleness is integer ordering. Alternative
considered: lease timestamps. Rejected because clock skew between a laptop and a server must
not be load-bearing.

**The bundle is one file, written atomically.** `manifest.json`, `events.jsonl`,
`workspace.patch` and `untracked/`, with `content_hash` over the event log and the patch in
that order and the untracked tree hashed as a sorted list of `path\0sha256`. Temp file plus
rename, so a partial bundle is impossible. Alternative considered: a tar stream over the RPC
channel. Rejected because the channel is newline-delimited JSON, so one binary message type
would need base64 and a framing exception.

**Verification precedes mutation.** Import runs every check in a scratch directory:
supported `format_version`, matching `content_hash`, contiguous `seq` within `seq_range`,
known event kinds, known payload versions, no locally-newer epoch, and an applicable
workspace descriptor. The database is opened for write only after all of that passes, and the
import is one transaction.

**Blobs are flattened on export and re-split on import.** The bundle format must not expose
the storage detail, so a reader that has never seen the sqlite schema can read it.

**Export refuses what it cannot capture.** Submodules, a file over 8 MiB, a total over 64 MiB,
an ignored file referenced by the patch, and conflict markers in the index are all refusals
with a specific message. Silent divergence costs a day; a refusal costs a minute.

**The workspace descriptor is not an image.** It carries `repo_id`, `head_commit`, `remote`,
`dirty_patch`, `untracked` and `toolchain_hint`. `repo_id` is sha256 of the git common
directory path and the root commit, and is a local identity: it is used to apply a patch in
place, never as proof that two hosts hold "the same" repository.

**A patch that does not apply cleanly is a refusal, not a fuzz.** Three-way merge is disabled
on purpose: a fuzzy application is a silent difference the operator did not ask for.

**Secrets are refused by pattern.** `*.pem`, `*.key`, `*.p12`, `id_*`, `.env`, `.env.*`,
`*credentials*`, `*secret*` in the transfer set. `--allow-secret-paths` downgrades the refusal
to a warning recorded in `handoff.exported`, so the override is visible in history.

**The transport is a second process.** `ssh <host> sa handoff --receive` streams raw bytes on
stdin. It keeps the RPC channel clean and costs nothing.

## Risks / Trade-offs

- **A bundle is a large file.** 64 MiB of untracked content is the cap, and a repository over
  that is refused rather than slowly transferred.
- **Refusal is common.** A dirty tree with submodules, a secret-shaped file, or an ignored
  file the patch touches all refuse. Each refusal names the reason, so the fix is obvious.
- **A fork is not a branch.** Two writable trees with no merge is the deliberate choice; an
  operator expecting a merge will be surprised once.
- **The epoch is only as good as the store.** A host that loses its database can be handed a
  stale bundle; the import check on epoch is the only guard.
