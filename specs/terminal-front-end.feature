# terminal-front-end
#
# The full-screen client: one window holding every session as a tab, a framed transcript, a prompt
# and a status bar. It is a view — it renders events and sends prompts, and holds no state the
# daemon does not have.

Feature: terminal-front-end
  The full-screen client: one window holding every session as a tab, a framed transcript, a
  prompt and a status bar. It is a view — it renders events and sends prompts, and holds no
  state the daemon does not have.

  # Requirement: TTY detection with a line-mode fallback
  # `sa attach` SHALL open the full-screen client when stdin and stdout are terminals, and the line-
  # oriented client otherwise. `sa attach --line` SHALL force line mode.
  @req:tty-detection-with-a-line-mode-fallback
  Scenario: A pipe gets the line client
    When `sa attach` runs with stdout redirected
    Then the line-oriented renderer runs and no terminal control sequences are emitted
  @req:tty-detection-with-a-line-mode-fallback
  Scenario: The flag forces line mode
    When `sa attach --line` runs on a terminal
    Then the line client is used

  # Requirement: One window shows every session
  # The window SHALL present every session the daemon knows about as a tab, ordered by creation time
  # so they never shuffle. `sa` run in a directory SHALL land on the tab already working there, or
  # open one for it. A session created by another client SHALL appear as a tab without a restart.
  @req:one-window-shows-every-session
  Scenario: A session made elsewhere becomes a tab
    When another client creates a session
    Then `sessions.changed` brings the tab strip back in line without a restart
  @req:one-window-shows-every-session
  Scenario: Tabs do not reorder under the operator
    When the tab strip is rendered twice with no session change
    Then the order is identical

  # Requirement: Tab controls
  # The client SHALL support opening a tab, closing one, cycling between them and selecting one
  # directly. The last remaining tab SHALL NOT be closable.
  @req:tab-controls
  Scenario: The last tab survives
    When the close key is pressed with one tab open
    Then the tab remains and a new session is not created silently
  @req:tab-controls
  Scenario: Cycling wraps
    When the cycle key is pressed on the last tab
    Then the first tab is selected

  # Requirement: Per-tab state
  # Each tab SHALL keep its own transcript, input, counters and scroll position, and daemon events
  # SHALL be routed only to the tab that owns the session.
  @req:per-tab-state
  Scenario: Events reach only their own tab
    When an event arrives for a session that is not the visible tab
    Then it is applied to that session's tab and the visible tab's transcript is unchanged

  # Requirement: Transcript rendering
  # The transcript SHALL render assistant text as it streams, tool calls with their arguments, tool
  # results truncated to a readable window with an explicit truncation marker, reasoning deltas
  # dimmed, and each turn's token usage. Each entry SHALL be drawn as its own titled frame with no
  # raw role gutter.
  @req:transcript-rendering
  Scenario: A tool call shows what it was pointed at
    When a tool call is rendered
    Then its arguments are visible and its result is marked when truncated
  @req:transcript-rendering
  Scenario: Reasoning is dimmed
    When reasoning deltas arrive
    Then they are rendered dimmed and distinguishable from the answer

  # Requirement: Cancellation and quit
  # `esc` SHALL interrupt a running turn and leave the client alive. The quit key SHALL exit the
  # client and leave the daemon and any detached turn running.
  @req:cancellation-and-quit
  Scenario: Escape stops the turn, not the client
    When `esc` is pressed during a turn
    Then the turn is cancelled and the prompt returns
  @req:cancellation-and-quit
  Scenario: Quitting does not stop the daemon
    When the quit key is pressed
    Then the client exits and a detached turn continues

  # Requirement: Queued messages are visible and editable
  # Both queues SHALL be shown above the composer under their own headings with the pending count on
  # the status bar. Moving to the queue, selecting a message, taking it back into the composer,
  # dropping it, and taking the whole queue back SHALL all be possible from the keyboard.
  @req:queued-messages-are-visible-and-editable
  Scenario: A queued message can be edited
    When a queued message is selected and taken back
    Then its text is in the composer and the queue no longer holds it
  @req:queued-messages-are-visible-and-editable
  Scenario: The pending count is visible
    When messages are queued
    Then the status bar reports how many are pending

  # Requirement: Slash commands
  # The client SHALL handle these commands without sending them as prompts: `/cancel`,
  # `/checkpoints`, `/cwd <dir>` (alias `/cd`), `/effort [level]`, `/exit`, `/help`, `/lsp`, `/mcp`,
  # `/model <name>`, `/new`, `/pwd`, `/resume` and `/rewind <id>`. `/help` SHALL list every command.
  @req:slash-commands
  Scenario: A command is not a prompt
    When a slash command is submitted
    Then no `turn.start` is sent for it
  @req:slash-commands
  Scenario: Every command is documented
    When `/help` is rendered
    Then every handled command appears in it
  @req:slash-commands
  Scenario: Directory completion
    When a partial directory is typed after `/cwd`
    Then completion offers the matching directories

  # Requirement: Status bar
  # The status bar SHALL be powerline-style: solid coloured areas joined by arrow separators, one
  # area per field — working directory, state, branch and short commit, model, throughput and tokens
  # — with nerdfont glyphs and no bracketed text. The branch area SHALL be absent rather than empty
  # when there is no branch to name.
  @req:status-bar
  Scenario: The bar carries the session's identity
    When a tab is rendered
    Then the bar shows its working directory, state, model and token counts
  @req:status-bar
  Scenario: The bar names the commit alongside the branch
    When a tab whose directory is a repository is rendered
    Then the branch area reads `main:6213c57`, and a detached HEAD shows the commit alone
  @req:status-bar
  Scenario: A machine without git has no branch area
    When no git binary is on the path, or the directory is not a repository
    Then the branch area is absent rather than empty
  @req:status-bar
  Scenario: Throughput reflects generation only
    When a turn stalls between deltas
    Then the stalled time is excluded from the rate rather than lowering it

  # Requirement: Single-line editing is delegated
  # Cursor movement, word deletion and history in the prompt SHALL be delegated to the input library
  # rather than hand-rolled.
  @req:single-line-editing-is-delegated
  Scenario: Editing keys behave as the input library defines
    When cursor and deletion keys are pressed in the prompt
    Then the input library's behaviour applies

  # Requirement: Dirty-gated rendering
  # A frame SHALL be built only when something visible changed, so an idle session costs nothing.
  @req:dirty-gated-rendering
  Scenario: An idle window does not redraw
    When no event arrives and no key is pressed
    Then no frame is built

  # Requirement: No theming layer
  # Layout SHALL be code, with a single deliberate visual and no registry of themes. Adding a second
  # visual SHALL mean adding a second render, not a configuration key.
  @req:no-theming-layer
  Scenario: There is no theme setting
    When the configuration is read
    Then no theme selection is consulted to draw the window

  # Requirement: Scope limits
  # The client SHALL NOT implement mouse input, text selection, markdown rendering, syntax
  # highlighting, panes or a second transcript. One transcript, one input, one status bar.
  @req:scope-limits
  Scenario: Mouse input changes nothing
    When a mouse event arrives
    Then it does not alter the selection or the layout

  # Requirement: History replay
  # Attaching to a session with history SHALL replay it into that tab, so each tab's transcript is
  # continuous across reconnects and restarts.
  @req:history-replay
  Scenario: A restarted daemon returns the tabs
    When the daemon restarts and the client reattaches
    Then each tab's transcript is rebuilt from the log

  # Requirement: The layout survives small terminals
  # The layout SHALL render without panicking from 20 columns by 6 rows up to 200 by 60.
  @req:the-layout-survives-small-terminals
  Scenario: A tiny terminal does not panic
    When the window is resized to 20x6
    Then the frame is drawn and nothing panics
