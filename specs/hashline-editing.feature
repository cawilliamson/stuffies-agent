# hashline-editing
#
# Addressing a line by a hash of its own text, so an edit names the content it means rather than a
# number that goes stale or a quote that is remembered slightly wrong.

Feature: hashline-editing
  Addressing a line by a hash of its own text, so an edit names the content it means rather
  than a number that goes stale or a quote that is remembered slightly wrong.

  # Requirement: Read renders addressable lines
  # `read` SHALL prefix every line with six hex characters of that line's own sha256 and a `|`
  # separator, so the model can copy the address back verbatim. `read {hashline: false}` SHALL
  # return the raw text.
  @req:read-renders-addressable-lines
  Scenario: A line is addressable as read
    When a file is read
    Then each line is `hhhhhh|content` and the hash is derived from the content alone
  @req:read-renders-addressable-lines
  Scenario: The raw form is one flag away
    When `read` is called with `hashline: false`
    Then the lines carry no prefix

  # Requirement: Edit addresses by hash
  # `edit` SHALL accept a list of `{hash, content}` edits and replace the lines those hashes name. A
  # `line` hint MAY be supplied to choose between identical lines and SHALL NOT override the hash.
  @req:edit-addresses-by-hash
  Scenario: A moved line is still found
    When an edit names a hash for a line that has shifted position since it was read
    Then the line is found and replaced
  @req:edit-addresses-by-hash
  Scenario: A changed line is refused
    When an edit names a hash no line currently carries
    Then the edit is refused and nothing is written

  # Requirement: Refusal is actionable
  # A refusal SHALL name what went wrong and return the lines around the line that was meant,
  # hashed, so the retry needs no separate read. A file short enough to show whole SHALL be returned
  # whole when there is nothing to aim at.
  @req:refusal-is-actionable
  Scenario: A stale hash is answered with the current text
    When an edit is refused because the hash is stale
    Then the result carries the hashed lines around the intended position
  @req:refusal-is-actionable
  Scenario: A whole small file is returned
    When a refusal has no line to aim at and the file is short
    Then the whole file is returned hashed

  # Requirement: Ambiguous and lenient shapes
  # A hash that several identical lines carry SHALL be refused naming each of them, unless a `line`
  # hint selects one. Text SHALL be accepted where a hash belongs, for a model that copied the wrong
  # half of the line. A line number that disagrees with the hash SHALL lose to the hash.
  @req:ambiguous-and-lenient-shapes
  Scenario: Identical lines need a hint
    When an edit names a hash carried by three identical lines and gives no `line`
    Then it is refused and all three positions are named
  @req:ambiguous-and-lenient-shapes
  Scenario: Copied text is accepted as an address
    When an edit supplies the line's text where a hash belongs
    Then the line it names is found and replaced

  # Requirement: Prefixes are addressing, not content
  # A `hhhhhh|` prefix copied back into `old` SHALL be stripped before matching, so a model that
  # pasted a hashed line still matches the file.
  @req:prefixes-are-addressing-not-content
  Scenario: A pasted hashed line matches
    When `old` carries a hashline prefix
    Then the prefix is removed and the text matches the file

  # Requirement: Unmatched text points at the live file
  # When an `old`/`new` edit matches nothing, or matches more than one place, the result SHALL name
  # the count and SHALL show the hashed lines where the quoted text's first line still is, so the
  # model can switch to a hash edit instead of re-reading.
  @req:unmatched-text-points-at-the-live-file
  Scenario: A quote that moved is located
    When `old` matches nothing but its first non-empty line exists in the file
    Then the result shows the hashed lines around that line
