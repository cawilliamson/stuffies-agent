# daemon-lifecycle
#
# The daemon process: where it binds, who may connect, how a connection is served, what it
# recovers on start, and how it stops. It is the only component that owns session state.

Feature: daemon-lifecycle
  The daemon process: where it binds, who may connect, how a connection is served, what it
  recovers on start, and how it stops. It is the only component that owns session state.

  # Requirement: Socket binding and permissions
  # The daemon SHALL bind a unix socket at `$XDG_RUNTIME_DIR/stuffies-agent/daemon.sock`, with
  # `SA_SOCKET` overriding the path. The containing directory SHALL be created or corrected to mode
  # `0700` and the socket SHALL be mode `0600`.
  @req:socket-binding-and-permissions
  Scenario: The socket and its directory are private
    When the daemon has bound its socket
    Then the socket file is mode `0600` and the directory holding it is mode `0700`
  @req:socket-binding-and-permissions
  Scenario: A custom socket path still gets a private directory
    When `SA_SOCKET` names a path in a directory that does not exist
    Then the daemon creates it at mode `0700` and binds there
  @req:socket-binding-and-permissions
  Scenario: A missing runtime directory is created
    When the runtime directory does not exist at startup
    Then the daemon creates it rather than failing to bind

  # Requirement: Live daemon is never displaced
  # A second daemon SHALL refuse to bind when the socket is already served by a live daemon, and
  # SHALL replace a socket left behind by a dead one.
  @req:live-daemon-is-never-displaced
  Scenario: A second daemon refuses the socket
    When a daemon starts while another is listening on the same path
    Then it exits with `AddrInUse` and the running daemon keeps its clients
  @req:live-daemon-is-never-displaced
  Scenario: A stale socket is replaced
    When the socket file exists but nothing accepts a connection on it
    Then the daemon removes the file and binds

  # Requirement: Peer credentials
  # Every accepted connection SHALL have its peer uid read before a single frame is read, and SHALL
  # be rejected with `-32001` when that uid differs from the daemon's effective uid.
  @req:peer-credentials
  Scenario: A foreign uid is refused before its first frame
    When a connection arrives from a uid other than the daemon's
    Then the daemon writes the error and closes, without parsing anything the peer sent

  # Requirement: Startup recovery
  # On start the daemon SHALL scan for sessions with an open turn and, for each, append
  # `tool.failed` with class `interrupted` for every tool call that never got a terminal event, then
  # append `turn.interrupted` with cause `daemon_restart`, and set the session state to
  # `interrupted`. No tool call SHALL be retried.
  @req:startup-recovery
  Scenario: A restart closes every open turn
    When the daemon is killed mid-turn and started again
    Then the log holds a `turn.interrupted` for that turn and a `tool.failed` for each call that was in flight
  @req:startup-recovery
  Scenario: Recovery pairs every request with a terminal event
    When the recovered log is scanned for `tool.requested` events without a matching `tool.completed` or `tool.failed`
    Then there are none
  @req:startup-recovery
  Scenario: The recovered session can take a new turn
    When a turn is started on a session recovered as `interrupted`
    Then it is accepted, the state moves back to `active`, and the move is recorded

  # Requirement: Graceful shutdown
  # On shutdown the daemon SHALL stop accepting connections, drain or interrupt in-flight work, and
  # remove the socket file. It SHALL NOT truncate the log.
  @req:graceful-shutdown
  Scenario: Shutdown removes the socket
    When the daemon is asked to stop
    Then the socket file is gone and a restart can bind immediately

  # Requirement: The daemon outlives its clients
  # The daemon SHALL NOT exit because its last client disconnected, and SHALL keep a detached turn
  # running with no client attached.
  @req:the-daemon-outlives-its-clients
  Scenario: A detached turn finishes with nobody watching
    When a turn is started with `detach: true` and every client disconnects
    Then the turn runs to completion and the log holds its terminal event
  @req:the-daemon-outlives-its-clients
  Scenario: A closed connection leaves the session alone
    When a client disconnects mid-turn
    Then the turn continues and the daemon keeps serving other connections

  # Requirement: Standalone backend
  # `sa --standalone <command>` SHALL run the backend inside the client process over a `socketpair`.
  # It SHALL NOT bind a socket, contact a running daemon, or open the on-disk database: its store is
  # in memory and nothing outlives the run. Combining `--standalone` with `sa daemon` SHALL be
  # refused.
  @req:standalone-backend
  Scenario: Standalone writes no state
    When a command runs under `--standalone` in a temporary state directory
    Then the directory gains no database and no session survives the run
  @req:standalone-backend
  Scenario: Standalone ignores a running daemon
    When a daemon is already listening and a command runs under `--standalone`
    Then the command talks to its in-process backend and the daemon is undisturbed
  @req:standalone-backend
  Scenario: Standalone is not a daemon
    When `sa --standalone daemon` is run
    Then it is refused with an explanation rather than starting a second backend
