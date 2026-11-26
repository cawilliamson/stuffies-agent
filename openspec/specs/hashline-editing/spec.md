# hashline-editing

## Purpose

Addressing a line by a hash of its own text, so an edit names the content it means rather
than a number that goes stale or a quote that is remembered slightly wrong.

## Requirements

### Requirement: Read renders addressable lines

`read` SHALL prefix every line with six hex characters of that line's own sha256 and a `|`
separator, so the model can copy the address back verbatim. `read {hashline: false}` SHALL
return the raw text.

#### Scenario: A line is addressable as read

- **WHEN** a file is read
- **THEN** each line is `hhhhhh|content` and the hash is derived from the content alone

#### Scenario: The raw form is one flag away

- **WHEN** `read` is called with `hashline: false`
- **THEN** the lines carry no prefix

### Requirement: Edit addresses by hash

`edit` SHALL accept a list of `{hash, content}` edits and replace the lines those hashes
name. A `line` hint MAY be supplied to choose between identical lines and SHALL NOT override
the hash.

#### Scenario: A moved line is still found

- **WHEN** an edit names a hash for a line that has shifted position since it was read
- **THEN** the line is found and replaced

#### Scenario: A changed line is refused

- **WHEN** an edit names a hash no line currently carries
- **THEN** the edit is refused and nothing is written

### Requirement: Refusal is actionable

A refusal SHALL name what went wrong and return the lines around the line that was meant,
hashed, so the retry needs no separate read. A file short enough to show whole SHALL be
returned whole when there is nothing to aim at.

#### Scenario: A stale hash is answered with the current text

- **WHEN** an edit is refused because the hash is stale
- **THEN** the result carries the hashed lines around the intended position

#### Scenario: A whole small file is returned

- **WHEN** a refusal has no line to aim at and the file is short
- **THEN** the whole file is returned hashed

### Requirement: Ambiguous and lenient shapes

A hash that several identical lines carry SHALL be refused naming each of them, unless a
`line` hint selects one. Text SHALL be accepted where a hash belongs, for a model that copied
the wrong half of the line. A line number that disagrees with the hash SHALL lose to the
hash.

#### Scenario: Identical lines need a hint

- **WHEN** an edit names a hash carried by three identical lines and gives no `line`
- **THEN** it is refused and all three positions are named

#### Scenario: Copied text is accepted as an address

- **WHEN** an edit supplies the line's text where a hash belongs
- **THEN** the line it names is found and replaced

### Requirement: Prefixes are addressing, not content

A `hhhhhh|` prefix copied back into `old` SHALL be stripped before matching, so a model that
pasted a hashed line still matches the file.

#### Scenario: A pasted hashed line matches

- **WHEN** `old` carries a hashline prefix
- **THEN** the prefix is removed and the text matches the file

### Requirement: Unmatched text points at the live file

When an `old`/`new` edit matches nothing, or matches more than one place, the result SHALL
name the count and SHALL show the hashed lines where the quoted text's first line still is,
so the model can switch to a hash edit instead of re-reading.

#### Scenario: A quote that moved is located

- **WHEN** `old` matches nothing but its first non-empty line exists in the file
- **THEN** the result shows the hashed lines around that line
