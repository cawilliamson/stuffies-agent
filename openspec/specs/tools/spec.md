# tools

## Purpose

The built-in tools a turn may call, what each does, and the bounds every one of them keeps:
paths inside the workspace, output that is capped and marked, and commands that cannot
outlive their turn.

## Requirements

### Requirement: Built-in tool set

The daemon SHALL offer exactly these built-in tools: `bash`, `edit`, `find`, `goal_done`,
`grep`, `lsp`, `read`, `todo`, `web_fetch`, `web_search` and `write`. A connected MCP
server's tools SHALL be offered alongside them under a `mcp__` prefix.

#### Scenario: The tool list matches the callable set

- **WHEN** the tool specifications are read
- **THEN** they are the built-in set plus one entry per connected server tool, and the system
  prompt is generated from the same list

#### Scenario: An unknown tool is a failure, not a crash

- **WHEN** the model calls a tool that is not registered
- **THEN** the call fails and the turn continues

### Requirement: Paths stay inside the workspace

Every path argument SHALL be resolved against the session's working directory, lexically
normalised, and refused when it does not resolve inside the workspace. Symlinks SHALL be
resolved before the check. An escape SHALL be a tool failure, not a clamp.

#### Scenario: A parent-directory path is refused

- **WHEN** a tool is called with `../outside`
- **THEN** it fails naming the resolved path and the turn continues

#### Scenario: An absolute path outside is refused

- **WHEN** a tool is called with `/etc/passwd`
- **THEN** it fails as an escape

### Requirement: Output is bounded and truncation is recorded

Tool output SHALL be capped at 256 KiB. When the cap is applied, the `tool.completed` event
SHALL set `truncated: true` so a client knows it saw a partial result.

#### Scenario: A large output is marked

- **WHEN** a tool produces more than 256 KiB
- **THEN** the event carries the capped output and `truncated: true`

### Requirement: Bash runs in a killable group

`bash` SHALL run `sh -c <command>` in the workspace with the daemon's environment inherited,
stdin closed, and a 600 second timeout. It SHALL run in its own process group so a cancelled
turn kills the whole tree, including children the command backgrounded. Output SHALL be
reported as it arrives.

A non-zero exit SHALL be `tool.completed` carrying the exit code; `tool.failed` SHALL mean the
tool itself could not run.

#### Scenario: A backgrounded child dies with the turn

- **WHEN** a command backgrounds a child and the turn is cancelled
- **THEN** the child is killed with the group

#### Scenario: A failing command is not a tool failure

- **WHEN** a command exits non-zero
- **THEN** the event is `tool.completed` with that exit code

#### Scenario: A hanging command is timed out

- **WHEN** a command runs past 600 seconds
- **THEN** it is terminated and the failure names the timeout

### Requirement: Read returns one window

`read` SHALL return at most 2000 lines by default, accept a `limit` up to 5000, and cap the
window at 50 KiB without splitting a line. A window that is not the whole file SHALL end with
a footer naming the range, the total and the offset to continue from. A read past the end
SHALL say so and name the file's length.

#### Scenario: A truncated read is visible

- **WHEN** a file longer than the window is read
- **THEN** the last line names the range, the total and the next offset

#### Scenario: A read past the end says so

- **WHEN** the offset is beyond the last line
- **THEN** the result says there are no lines there and gives the file's length

#### Scenario: A line is never split

- **WHEN** the next line would exceed the byte budget
- **THEN** the window ends before it rather than returning half of it

### Requirement: Write reports its change

`write` SHALL create parent directories as needed and write the file. When the file already
existed, the result SHALL include a unified diff of what changed, with three lines of
context, and the findings of the language server for that file.

#### Scenario: Replacing a file shows the change

- **WHEN** `write` replaces an existing file
- **THEN** the result carries a unified diff of the before and after text

#### Scenario: A new file has nothing to diff

- **WHEN** `write` creates a file that did not exist
- **THEN** the result reports the byte count and no diff

### Requirement: Edit reports its change and fresh hashes

`edit` SHALL report the change as a unified diff with three lines of context and SHALL return
the hashes of the lines it wrote, so the next edit needs no read. Findings for the changed
file SHALL follow.

#### Scenario: An edit is reviewable in place

- **WHEN** a file is edited
- **THEN** the result carries the diff and the hashes as written

### Requirement: Find and grep honour the project's ignore files

`find` and `grep` SHALL walk what the project's own ignore files include, SHALL search
dotfiles, SHALL never descend into `.git`, and SHALL bound the walk at 5000 entries and the
result at 500 matches.

#### Scenario: Build output is not searched

- **WHEN** `grep` searches a project whose `.gitignore` excludes a build directory
- **THEN** no match from that directory is returned

#### Scenario: A dotfile is still source

- **WHEN** `grep` searches a project with `.github/workflows/ci.yml`
- **THEN** a match in that file is returned

#### Scenario: The git directory is skipped

- **WHEN** `find` walks a repository
- **THEN** nothing under `.git` is returned

### Requirement: Todo is transcript-only

`todo` SHALL render the whole list it is given as a checklist and store nothing. `status`
SHALL be one of `completed`, `in_progress` or `pending`; anything else, an item without
content, or more than 50 items SHALL be an argument error.

#### Scenario: The plan is not stored

- **WHEN** `todo` is called twice with different lists
- **THEN** the second result reflects only the second list, and nothing is persisted

#### Scenario: An invalid status is refused

- **WHEN** an item carries a status other than the three known values
- **THEN** the call fails naming the value and the accepted set

#### Scenario: The list is bounded

- **WHEN** 51 items are sent
- **THEN** the call fails naming the maximum

### Requirement: Web tools are the only egress

`web_fetch` and `web_search` SHALL be the only tools that leave the machine. They SHALL speak
through a browser-emulated client so a connection-fingerprinting site sees a browser.
`web_fetch` SHALL fetch only `http` and `https`, cap the body at 4 MiB and mark it when cut,
and SHALL report the status it was served. `web_search` SHALL return a numbered list of title,
url and snippet, five by default and twenty at most.

#### Scenario: A non-http scheme is an argument error

- **WHEN** `web_fetch` is called with `file:///etc/passwd`
- **THEN** it fails as an argument error and nothing is fetched

#### Scenario: A large page is cut and marked

- **WHEN** a page exceeds 4 MiB
- **THEN** the body is truncated and the result says so

#### Scenario: An empty search says so

- **WHEN** a search finds nothing
- **THEN** the result says that, rather than failing

### Requirement: Goal completion is a tool

`goal_done {summary}` SHALL append `goal.done` with the summary and return. An empty summary
SHALL be an argument error.

#### Scenario: The tool records the event the loop reads

- **WHEN** the model calls `goal_done`
- **THEN** `goal.done` is in the log and the loop reads it from there
