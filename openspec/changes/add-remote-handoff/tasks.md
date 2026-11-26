# Tasks

## 1. Ownership and fencing

- [ ] 1.1 Add `owner_epoch` comparison to `turn.start`, refusing a stale epoch with `-32006`
      and naming both epochs
- [ ] 1.2 Derive `host_fingerprint` from `/etc/machine-id`, falling back to a UUID stored at
      mode `0600` under the state directory
- [ ] 1.3 Add `session.state` transitions for `quiescing`, `relocating` and `relocated`, with
      `relocated` refusing turns
- [ ] 1.4 Record `parent_id` and epoch lineage on a forked session

## 2. Bundle format

- [ ] 2.1 Write the manifest with `format_version`, `session_id`, `owner_epoch`,
      `seq_range`, `content_hash`, `created_at`, `source_host`, `workspace` and `mode`
- [ ] 2.2 Export `events.jsonl` with blobs flattened back into the payloads
- [ ] 2.3 Export `workspace.patch` and `untracked/` under the size caps
- [ ] 2.4 Write the bundle as a temp file plus rename, so a partial bundle is impossible
- [ ] 2.5 Compute `content_hash` over the log and the patch in order, with the untracked tree
      as a sorted `path\0sha256` list

## 3. Export preconditions

- [ ] 3.1 Refuse submodules by name
- [ ] 3.2 Refuse a file over 8 MiB or a transfer set over 64 MiB
- [ ] 3.3 Refuse an ignored file the patch references
- [ ] 3.4 Refuse a dirty index that cannot be represented, naming the conflict markers
- [ ] 3.5 Refuse secret-shaped paths, and record the override when
      `--allow-secret-paths` is given

## 4. Import verification

- [ ] 4.1 Verify `format_version`, `content_hash` and contiguous `seq` in a scratch directory
- [ ] 4.2 Reject an unknown event kind or payload version outright
- [ ] 4.3 Reject a bundle whose epoch is not newer than a local session of the same id
- [ ] 4.4 Apply the whole import in one transaction, after every check passes
- [ ] 4.5 Prove a rejected import leaves the target unchanged

## 5. Workspace descriptor

- [ ] 5.1 Emit `repo_id`, `head_commit`, `remote`, `dirty_patch`, `untracked` and
      `toolchain_hint`
- [ ] 5.2 Apply case one: matching `repo_id`, patch in place, `head_commit` equal or an
      ancestor of `HEAD`
- [ ] 5.3 Apply case two: clone from `remote`, check out `head_commit`, apply the patch
- [ ] 5.4 Apply case three: refuse with `-32008` unless the caller supplies a workspace whose
      root commit matches
- [ ] 5.5 Refuse with `-32007` naming both commits when the tree has diverged
- [ ] 5.6 Apply every patch with three-way merge disabled

## 6. Move and fork

- [ ] 6.1 Implement `handoff.export` for mode `move`, quiescing at a turn boundary and
      writing a checkpoint first
- [ ] 6.2 Implement `handoff.import` for mode `move`, incrementing the epoch and taking
      ownership
- [ ] 6.3 Implement `handoff.accept`, completing the move and recording `handoff.imported`
- [ ] 6.4 Implement `session.fork`, requiring a target workspace that does not exist or is
      empty
- [ ] 6.5 Keep the source tree untouched by a fork
- [ ] 6.6 Refuse a second writable session on one tree, per the workspace lock

## 7. Transport

- [ ] 7.1 Add `sa handoff --send <session> --host <host>`
- [ ] 7.2 Add `sa handoff --receive`, streaming raw bytes on stdin
- [ ] 7.3 Confirm the SSH channel dropping mid-transfer does not affect a running turn

## 8. Verification

- [ ] 8.1 Round-trip a session between two in-process daemons
- [ ] 8.2 Assert the source refuses writes after a move
- [ ] 8.3 Assert a stale bundle is rejected without mutation
- [ ] 8.4 Assert a fork produces a distinct `parent_id` and workspace
- [ ] 8.5 Assert a dirty worktree transfers and applies cleanly
- [ ] 8.6 Assert a submodule and an `id_rsa` are each refused by name
- [ ] 8.7 Assert the bundle is readable by a process that has never seen the sqlite schema
