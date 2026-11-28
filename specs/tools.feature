# tools
#
# The built-in tools a turn may call, what each does, and the bounds every one of them keeps:
# paths inside the workspace, output that is capped and marked, and commands that cannot outlive
# their turn.

Feature: tools
  The built-in tools a turn may call, what each does, and the bounds every one of them keeps:
  paths inside the workspace, output that is capped and marked, and commands that cannot
  outlive their turn.

  # Requirement: Built-in tool set
  # The daemon SHALL offer exactly these built-in tools: `bash`, `edit`, `find`, `goal_done`,
  # `grep`, `lsp`, `read`, `todo`, `web_fetch`, `web_search` and `write`. A connected MCP server's
  # tools SHALL be offered alongside them under a `mcp__` prefix.
  @req:built-in-tool-set
  Scenario: The tool list matches the callable set
    When the tool specifications are read
    Then they are the built-in set plus one entry per connected server tool, and the system prompt is generated from the same list
  @req:built-in-tool-set
  Scenario: An unknown tool is a failure, not a crash
    When the model calls a tool that is not registered
    Then the call fails and the turn continues

  # Requirement: Paths stay inside the workspace
  # Every path argument SHALL be resolved against the session's working directory, lexically
  # normalised, and refused when it does not resolve inside the workspace. Symlinks SHALL be
  # resolved before the check. An escape SHALL be a tool failure, not a clamp.
  @req:paths-stay-inside-the-workspace
  Scenario: A parent-directory path is refused
    When a tool is called with `../outside`
    Then it fails naming the resolved path and the turn continues
  @req:paths-stay-inside-the-workspace
  Scenario: An absolute path outside is refused
    When a tool is called with `/etc/passwd`
    Then it fails as an escape
  @req:paths-stay-inside-the-workspace
  Scenario: A symlink pointing out is refused
    When a link inside the workspace resolves outside it
    Then the call fails naming the path it actually landed on

  # Requirement: Output is bounded and truncation is recorded
  # Tool output SHALL be capped at 256 KiB. When the cap is applied, the `tool.completed` event
  # SHALL set `truncated: true` so a client knows it saw a partial result.
  @req:output-is-bounded-and-truncation-is-recorded
  Scenario: A large output is marked
    When a tool produces more than 256 KiB
    Then the event carries the capped output and `truncated: true`

  # Requirement: Bash runs in a killable group
  # `bash` SHALL run `sh -c <command>` in the workspace with the daemon's environment inherited,
  # stdin closed, and a 600 second timeout. It SHALL run in its own process group so a cancelled
  # turn kills the whole tree, including children the command backgrounded. Output SHALL be reported
  # as it arrives. A non-zero exit SHALL be `tool.completed` carrying the exit code; `tool.failed`
  # SHALL mean the tool itself could not run.
  @req:bash-runs-in-a-killable-group
  Scenario: A backgrounded child dies with the turn
    When a command backgrounds a child and the turn is cancelled
    Then the child is killed with the group
  @req:bash-runs-in-a-killable-group
  Scenario: A failing command is not a tool failure
    When a command exits non-zero
    Then the event is `tool.completed` with that exit code
  @req:bash-runs-in-a-killable-group
  Scenario: A hanging command is timed out
    When a command runs past 600 seconds
    Then it is terminated and the failure names the timeout

  # Requirement: Read returns one window
  # `read` SHALL return at most 2000 lines by default, accept a `limit` up to 5000, and cap the
  # window at 50 KiB without splitting a line. A window that is not the whole file SHALL end with a
  # footer naming the range, the total and the offset to continue from. A read past the end SHALL
  # say so and name the file's length.
  @req:read-returns-one-window
  Scenario: A truncated read is visible
    When a file longer than the window is read
    Then the last line names the range, the total and the next offset
  @req:read-returns-one-window
  Scenario: A read past the end says so
    When the offset is beyond the last line
    Then the result says there are no lines there and gives the file's length
  @req:read-returns-one-window
  Scenario: A line is never split
    When the next line would exceed the byte budget
    Then the window ends before it rather than returning half of it

  # Requirement: An image is read as a picture
  # A file whose bytes are `png`, `jpeg`, `gif` or `webp` SHALL NOT be decoded as text. `read` SHALL
  # return it as an image content part beside a line naming the file, its type and its size, so the
  # model sees the picture rather than an error. The type SHALL be taken from the bytes, not from
  # the file name.
  @req:an-image-is-read-as-a-picture
  Scenario: A screenshot is looked at
    When `read` is called on a `png`
    Then the result carries the image and the model can describe it
  @req:an-image-is-read-as-a-picture
  Scenario: A binary that is neither
    When `read` is called on a file that is not text and not an image
    Then the error says it is neither, rather than reporting a utf-8 failure

  # Requirement: Write reports its change
  # `write` SHALL create parent directories as needed and write the file. When the file already
  # existed, the result SHALL include a unified diff of what changed, with three lines of context,
  # and the findings of the language server for that file.
  @req:write-reports-its-change
  Scenario: Replacing a file shows the change
    When `write` replaces an existing file
    Then the result carries a unified diff of the before and after text
  @req:write-reports-its-change
  Scenario: A new file has nothing to diff
    When `write` creates a file that did not exist
    Then the result reports the byte count and no diff

  # Requirement: Edit reports its change and fresh hashes
  # `edit` SHALL report the change as a unified diff with three lines of context and SHALL return
  # the hashes of the lines it wrote, so the next edit needs no read. Findings for the changed file
  # SHALL follow.
  @req:edit-reports-its-change-and-fresh-hashes
  Scenario: An edit is reviewable in place
    When a file is edited
    Then the result carries the diff and the hashes as written

  # Requirement: Find and grep honour the project's ignore files
  # `find` and `grep` SHALL walk what the project's own ignore files include, SHALL search dotfiles,
  # SHALL never descend into `.git`, and SHALL bound the walk at 5000 entries and the result at 500
  # matches.
  @req:find-and-grep-honour-the-project-s-ignore-files
  Scenario: Build output is not searched
    When `grep` searches a project whose `.gitignore` excludes a build directory
    Then no match from that directory is returned
  @req:find-and-grep-honour-the-project-s-ignore-files
  Scenario: A dotfile is still source
    When `grep` searches a project with `.github/workflows/ci.yml`
    Then a match in that file is returned
  @req:find-and-grep-honour-the-project-s-ignore-files
  Scenario: The git directory is skipped
    When `find` walks a repository
    Then nothing under `.git` is returned

  # Requirement: Todo is transcript-only
  # `todo` SHALL render the whole list it is given as a checklist and store nothing. `status` SHALL
  # be one of `completed`, `in_progress` or `pending`; anything else, an item without content, or
  # more than 50 items SHALL be an argument error.
  @req:todo-is-transcript-only
  Scenario: The plan is not stored
    When `todo` is called twice with different lists
    Then the second result reflects only the second list, and nothing is persisted
  @req:todo-is-transcript-only
  Scenario: An invalid status is refused
    When an item carries a status other than the three known values
    Then the call fails naming the value and the accepted set
  @req:todo-is-transcript-only
  Scenario: The list is bounded
    When 51 items are sent
    Then the call fails naming the maximum

  # Requirement: Web tools are the only egress
  # `web_fetch` and `web_search` SHALL be the only tools that leave the machine. They SHALL speak
  # through a browser-emulated client so a connection-fingerprinting site sees a browser.
  # `web_fetch` SHALL fetch only `http` and `https`, cap the body at 4 MiB and mark it when cut, and
  # SHALL report the status it was served. `web_search` SHALL return a numbered list of title, url
  # and snippet, five by default and twenty at most.
  @req:web-tools-are-the-only-egress
  Scenario: A non-http scheme is an argument error
    When `web_fetch` is called with `file:///etc/passwd`
    Then it fails as an argument error and nothing is fetched
  @req:web-tools-are-the-only-egress
  @req:web-tools-are-the-only-egress
  Scenario: A large page is cut and marked
    When a page exceeds 4 MiB
    Then the body is truncated and the result says so
  @req:web-tools-are-the-only-egress
  Scenario: An empty search says so
    When a search finds nothing
    Then the result says that, rather than failing

  # Requirement: Goal completion is a tool
  # `goal_done {summary}` SHALL append `goal.done` with the summary and return. An empty summary
  # SHALL be an argument error.
  @req:goal-completion-is-a-tool
  Scenario: The tool records the event the loop reads
    When the model calls `goal_done`
    Then `goal.done` is in the log and the loop reads it from there
