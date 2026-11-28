# goal-mode
#
# A loop that keeps a session working until the model reports the goal achieved or impossible. The
# loop is derived from the event log, so a restart, a second client or a replayed log picks it up
# where it was.

Feature: goal-mode
  A loop that keeps a session working until the model reports the goal achieved or impossible.
  The loop is derived from the event log, so a restart, a second client or a replayed log
  picks it up where it was.

  # Requirement: Goal state lives in the log
  # `goal.set`, `goal.nudged`, `goal.done` and `goal.stopped` SHALL record everything the loop
  # knows. Its state SHALL be derived by reading the last of each, and nothing about a goal SHALL be
  # held beside the log or written into the workspace.
  @req:goal-state-lives-in-the-log
  Scenario: A restarted daemon resumes the loop
    When the daemon restarts with an open goal
    Then `goal.status` reports the same text, nudge count and cap as before
  @req:goal-state-lives-in-the-log
  Scenario: A second client sees the same loop
    When two clients ask for `goal.status`
    Then both are answered from the same log-derived state

  # Requirement: Setting a goal
  # `goal.set` SHALL accept the goal text and an optional nudge cap, default 20, and SHALL record
  # them. Empty text SHALL be refused with `-32602`. Setting a goal SHALL NOT itself start a turn.
  @req:setting-a-goal
  Scenario: An empty goal is refused
    When `goal.set` is called with whitespace only
    Then it is refused with `-32602`
  @req:setting-a-goal
  Scenario: A goal does not start work
    When a goal is set and no turn is started
    Then no turn runs

  # Requirement: Nudges follow a finished turn only
  # A nudge SHALL be queued only after a turn that ran to its own end. A cancel, a timeout, a failed
  # checkpoint or an endpoint error SHALL leave the goal open without spending a nudge.
  @req:nudges-follow-a-finished-turn-only
  Scenario: A cancelled turn spends no nudge
    When a turn under a goal is cancelled
    Then no `goal.nudged` is appended and the goal stays open
  @req:nudges-follow-a-finished-turn-only
  Scenario: A completed turn earns a nudge
    When a turn under an open goal completes without `goal_done`
    Then `goal.nudged` is appended and a follow-up carrying the nudge text is queued

  # Requirement: Nudge text names the failure mode
  # The first nudge SHALL state the goal and that exploratory progress is not completion. Later
  # nudges SHALL carry the iteration count and the cap, SHALL say that an approach that already
  # failed must change, and SHALL warn when only a few nudges remain. A blocker SHALL be presented
  # as a valid outcome when named.
  @req:nudge-text-names-the-failure-mode
  Scenario: The first nudge is plain
    When the first nudge is queued
    Then its text carries no iteration count
  @req:nudge-text-names-the-failure-mode
  Scenario: A repeated approach is called out
    When a second or later nudge is queued
    Then its text carries the iteration number and tells the model to change approach
  @req:nudge-text-names-the-failure-mode
  Scenario: The end is signposted
    When two or fewer nudges remain
    Then the nudge text says the loop is about to stop

  # Requirement: The loop is capped
  # When the nudge count reaches the cap the loop SHALL append `goal.stopped` with reason
  # `exhausted` and SHALL queue no further nudge.
  @req:the-loop-is-capped
  Scenario: An exhausted loop stops rather than circling
    When the nudge count reaches the cap
    Then `goal.stopped` with reason `exhausted` is appended and the next turn queues no nudge

  # Requirement: A new goal starts at zero
  # A nudge count SHALL belong to the goal it was spent on. A goal set after an earlier one ended
  # SHALL start with no nudges spent, and events from the earlier goal SHALL NOT count toward it.
  @req:a-new-goal-starts-at-zero
  Scenario: A second goal inherits nothing
    When a goal is set after a previous goal was stopped
    Then `goal.status` reports zero nudges spent

  # Requirement: Completion signal
  # `goal_done {summary}` SHALL append `goal.done` with the summary and end the loop. It SHALL be
  # the only thing that ends a loop early. An empty summary SHALL be an argument error.
  @req:completion-signal
  Scenario: The summary becomes the reported result
    When the model calls `goal_done` with a summary
    Then `goal.status` reports that summary and the loop is no longer active
  @req:completion-signal
  Scenario: A blocker is a result
    When the model calls `goal_done` naming what blocks it
    Then the loop ends and the blocker is the recorded summary
  @req:completion-signal
  Scenario: An empty summary is refused
    When `goal_done` is called with an empty summary
    Then it fails as an argument error and no event is appended

  # Requirement: Stopping a goal
  # `goal.stop` SHALL close the loop and append `goal.stopped` with the reason, so the log says who
  # ended it. `goal.status` SHALL report the text, the nudges spent, the cap and the result.
  @req:stopping-a-goal
  Scenario: A user stop is recorded as such
    When `goal.stop` is called
    Then `goal.stopped` carries reason `user` and the loop is no longer active
  @req:stopping-a-goal
  Scenario: Status reports the whole loop
    When `goal.status` is asked while a goal is open
    Then the result carries the text, the nudges spent and the cap
