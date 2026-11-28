# client-library
#
# `sa-client` is the only thing a client needs: connect to the daemon, handshake, call a method,
# and read frames. It holds a cursor, never authority.

Feature: client-library
  `sa-client` is the only thing a client needs: connect to the daemon, handshake, call a
  method, and read frames. It holds a cursor, never authority.

  # Requirement: Connection and handshake
  # The library SHALL connect to a daemon socket, send `hello` with the client's own name, and
  # expose the daemon's `HelloResult`. It SHALL refuse to proceed when the handshake fails,
  # surfacing the daemon's error verbatim.
  @req:connection-and-handshake
  Scenario: A handshake reports the daemon's identity
    When a client connects to a live daemon and handshakes
    Then it receives the protocol version, the daemon version and the build commit
  @req:connection-and-handshake
  Scenario: A failed handshake is reported as the daemon said it
    When the daemon refuses the handshake
    Then the library returns the daemon's error message unchanged
  @req:connection-and-handshake
  Scenario: An absent daemon is a connection error
    When nothing is listening on the socket path
    Then connecting fails with the path named, rather than hanging

  # Requirement: Calls and notifications
  # The library SHALL send a request and await the response with the matching `id`, and SHALL expose
  # frames that are not responses — notifications — separately so a caller can pump them while a
  # request is outstanding.
  @req:calls-and-notifications
  Scenario: A call returns its own response
    When two calls are made with different ids and answered out of order
    Then each caller receives the response carrying its own id
  @req:calls-and-notifications
  Scenario: A notification is not mistaken for a response
    When an `event` notification arrives between a request and its response
    Then the notification is available to the caller and the request still resolves

  # Requirement: No authority
  # The library SHALL hold no session state of its own beyond a cursor. It SHALL NOT decide session
  # ownership, filter the log, or cache anything the daemon is the source of truth for.
  @req:no-authority
  Scenario: A reconnecting client resumes from its cursor
    When a client reconnects after being away and replays from its last acknowledged `seq`
    Then it receives exactly the events it missed, with no local reconstruction
