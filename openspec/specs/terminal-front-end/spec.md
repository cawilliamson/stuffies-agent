# terminal-front-end

## Purpose

The full-screen client: one window holding every session as a tab, a framed transcript, a
prompt and a status bar. It is a view — it renders events and sends prompts, and holds no
state the daemon does not have.

## Requirements

### Requirement: TTY detection with a line-mode fallback

`sa attach` SHALL open the full-screen client when stdin and stdout are terminals, and the
line-oriented client otherwise. `sa attach --line` SHALL force line mode.

#### Scenario: A pipe gets the line client

- **WHEN** `sa attach` runs with stdout redirected
- **THEN** the line-oriented renderer runs and no terminal control sequences are emitted

#### Scenario: The flag forces line mode

- **WHEN** `sa attach --line` runs on a terminal
- **THEN** the line client is used

### Requirement: One window shows every session

The window SHALL present every session the daemon knows about as a tab, ordered by creation
time so they never shuffle. `sa` run in a directory SHALL land on the tab already working
there, or open one for it. A session created by another client SHALL appear as a tab without
a restart.

#### Scenario: A session made elsewhere becomes a tab

- **WHEN** another client creates a session
- **THEN** `sessions.changed` brings the tab strip back in line without a restart

#### Scenario: Tabs do not reorder under the operator

- **WHEN** the tab strip is rendered twice with no session change
- **THEN** the order is identical

### Requirement: Tab controls

The client SHALL support opening a tab, closing one, cycling between them and selecting one
directly. The last remaining tab SHALL NOT be closable.

#### Scenario: The last tab survives

- **WHEN** the close key is pressed with one tab open
- **THEN** the tab remains and a new session is not created silently

#### Scenario: Cycling wraps

- **WHEN** the cycle key is pressed on the last tab
- **THEN** the first tab is selected

### Requirement: Per-tab state

Each tab SHALL keep its own transcript, input, counters and scroll position, and daemon
events SHALL be routed only to the tab that owns the session.

#### Scenario: Events reach only their own tab

- **WHEN** an event arrives for a session that is not the visible tab
- **THEN** it is applied to that session's tab and the visible tab's transcript is unchanged

### Requirement: Transcript rendering

The transcript SHALL render assistant text as it streams, tool calls with their arguments,
tool results truncated to a readable window with an explicit truncation marker, reasoning
deltas dimmed, and each turn's token usage. Each entry SHALL be drawn as its own titled frame
with no raw role gutter.

#### Scenario: A tool call shows what it was pointed at

- **WHEN** a tool call is rendered
- **THEN** its arguments are visible and its result is marked when truncated

#### Scenario: Reasoning is dimmed

- **WHEN** reasoning deltas arrive
- **THEN** they are rendered dimmed and distinguishable from the answer

### Requirement: Cancellation and quit

`esc` SHALL interrupt a running turn and leave the client alive. The quit key SHALL exit the
client and leave the daemon and any detached turn running.

#### Scenario: Escape stops the turn, not the client

- **WHEN** `esc` is pressed during a turn
- **THEN** the turn is cancelled and the prompt returns

#### Scenario: Quitting does not stop the daemon

- **WHEN** the quit key is pressed
- **THEN** the client exits and a detached turn continues

### Requirement: Queued messages are visible and editable

Both queues SHALL be shown above the composer under their own headings with the pending count
on the status bar. Moving to the queue, selecting a message, taking it back into the composer,
dropping it, and taking the whole queue back SHALL all be possible from the keyboard.

#### Scenario: A queued message can be edited

- **WHEN** a queued message is selected and taken back
- **THEN** its text is in the composer and the queue no longer holds it

#### Scenario: The pending count is visible

- **WHEN** messages are queued
- **THEN** the status bar reports how many are pending

### Requirement: Slash commands

The client SHALL handle these commands without sending them as prompts: `/cancel`,
`/checkpoints`, `/cwd <dir>` (alias `/cd`), `/effort [level]`, `/exit`, `/help`, `/lsp`,
`/mcp`, `/model <name>`, `/new`, `/openspec <args>`, `/pwd`, `/resume` and `/rewind <id>`.
`/help` SHALL list every command, and a bare `/openspec` SHALL list the calls worth knowing.

#### Scenario: A command is not a prompt

- **WHEN** a slash command is submitted
- **THEN** no `turn.start` is sent for it

#### Scenario: Every command is documented

- **WHEN** `/help` is rendered
- **THEN** every handled command appears in it

#### Scenario: Directory completion

- **WHEN** a partial directory is typed after `/cwd`
- **THEN** completion offers the matching directories

#### Scenario: The arguments reach the program

- **WHEN** `/openspec list --specs` is submitted
- **THEN** the daemon runs the program with those arguments and its output is one entry

#### Scenario: A failure reads as one

- **WHEN** the program exits non-zero
- **THEN** its output is shown as an error rather than a notice

#### Scenario: A bare command teaches its calls

- **WHEN** `/openspec` is submitted with no argument
- **THEN** the calls worth knowing are listed and no request is sent

### Requirement: Status bar

The status bar SHALL be powerline-style: solid coloured areas joined by arrow separators, one
area per field — working directory, state, branch and short commit, model, throughput and
tokens — with nerdfont glyphs and no bracketed text. The branch area SHALL be absent rather
than empty when there is no branch to name.

#### Scenario: The bar carries the session's identity

- **WHEN** a tab is rendered
- **THEN** the bar shows its working directory, state, model and token counts

#### Scenario: The bar names the commit alongside the branch

- **WHEN** a tab whose directory is a repository is rendered
- **THEN** the branch area reads `main:6213c57`, and a detached HEAD shows the commit alone

#### Scenario: A machine without git has no branch area

- **WHEN** no git binary is on the path, or the directory is not a repository
- **THEN** the branch area is absent rather than empty

#### Scenario: Throughput reflects generation only

- **WHEN** a turn stalls between deltas
- **THEN** the stalled time is excluded from the rate rather than lowering it

### Requirement: Single-line editing is delegated

Cursor movement, word deletion and history in the prompt SHALL be delegated to the input
library rather than hand-rolled.

#### Scenario: Editing keys behave as the input library defines

- **WHEN** cursor and deletion keys are pressed in the prompt
- **THEN** the input library's behaviour applies

### Requirement: Dirty-gated rendering

A frame SHALL be built only when something visible changed, so an idle session costs
nothing.

#### Scenario: An idle window does not redraw

- **WHEN** no event arrives and no key is pressed
- **THEN** no frame is built

### Requirement: No theming layer

Layout SHALL be code, with a single deliberate visual and no registry of themes. Adding a
second visual SHALL mean adding a second render, not a configuration key.

#### Scenario: There is no theme setting

- **WHEN** the configuration is read
- **THEN** no theme selection is consulted to draw the window

### Requirement: Scope limits

The client SHALL NOT implement mouse input, text selection, markdown rendering, syntax
highlighting, panes or a second transcript. One transcript, one input, one status bar.

#### Scenario: Mouse input changes nothing

- **WHEN** a mouse event arrives
- **THEN** it does not alter the selection or the layout

### Requirement: History replay

Attaching to a session with history SHALL replay it into that tab, so each tab's transcript
is continuous across reconnects and restarts.

#### Scenario: A restarted daemon returns the tabs

- **WHEN** the daemon restarts and the client reattaches
- **THEN** each tab's transcript is rebuilt from the log

### Requirement: The layout survives small terminals

The layout SHALL render without panicking from 20 columns by 6 rows up to 200 by 60.

#### Scenario: A tiny terminal does not panic

- **WHEN** the window is resized to 20x6
- **THEN** the frame is drawn and nothing panics
