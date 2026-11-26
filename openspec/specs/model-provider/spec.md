# model-provider

## Purpose

The daemon's model client: the OpenAI-compatible wire, where credentials come from, how the
endpoint's models and reasoning efforts are discovered, and what is recorded about usage.

## Requirements

### Requirement: One wire protocol

The daemon SHALL speak OpenAI-compatible chat completions, including streaming and tool
calls. Other vendors SHALL be reached through OpenAI-compatible endpoints. There SHALL be one
provider implementation behind a trait, so a scripted provider can drive the loop in tests
without a network.

#### Scenario: A turn streams through the one provider

- **WHEN** a turn runs
- **THEN** it uses the OpenAI-compatible client and no other wire format

#### Scenario: Tests drive the loop with no network

- **WHEN** a scripted provider is supplied
- **THEN** a turn completes with the scripted deltas and touches no socket

### Requirement: Credentials belong to the daemon

The daemon SHALL read the endpoint from `OPENAI_API_ENDPOINT` and the token from
`OPENAI_API_TOKEN`, or from the file named by `SA_TOKEN_FILE`. That file MAY hold the token
alone or an environment file with an `OPENAI_API_TOKEN` or `VLLM_API_KEY` line, quoted or
bare. Clients SHALL never see a key.

#### Scenario: A token file is read by the daemon

- **WHEN** `SA_TOKEN_FILE` names a file containing a token
- **THEN** the daemon takes the value out of it itself, with no external command

#### Scenario: A quoted value is unquoted

- **WHEN** the token file carries `OPENAI_API_TOKEN="sk-quoted"`
- **THEN** the token used is `sk-quoted`

#### Scenario: A missing token is an error at start

- **WHEN** neither the variable nor the file provides a token
- **THEN** the daemon fails to start, naming what it looked for

### Requirement: Models are discovered, never invented

`model.list` SHALL report what the endpoint says about each model: its id, its context
window, its capabilities and the reasoning effort values it accepts. Discovery SHALL happen
at daemon start and SHALL be retried in the background while the endpoint is unreachable.
A field the endpoint does not report SHALL be absent, and there SHALL be no compiled-in
default window.

#### Scenario: An unadvertised window is absent

- **WHEN** the endpoint does not report a context window for a model
- **THEN** `context_limit` is absent from `model.list` and nothing pretends otherwise

#### Scenario: Discovery recovers after an outage

- **WHEN** the endpoint is unreachable at start and reachable later
- **THEN** the model list fills in without restarting the daemon

### Requirement: Effort values are probed, not assumed

The daemon SHALL learn a model's reasoning effort values by probing, comparing each candidate
against a sentinel value no server knows, and caching the result per endpoint and model. It
SHALL never impose a value the endpoint did not accept.

#### Scenario: An ignored value is not reported as supported

- **WHEN** a candidate behaves exactly like the sentinel
- **THEN** it is not included in that model's effort levels

#### Scenario: No advertised levels means none is sent

- **WHEN** a model advertises no effort levels
- **THEN** requests carry no reasoning effort and the server's own default applies

### Requirement: Selecting a model or effort

`model.select` SHALL refuse a model the endpoint does not serve, naming the models it does.
`model.effort` SHALL refuse a level the model does not accept, naming the accepted levels,
and SHALL accept any value when the endpoint advertises none. Both SHALL record a
`model.changed` event.

#### Scenario: A typo is refused rather than stored

- **WHEN** `model.select` names a model the endpoint does not serve
- **THEN** it is refused with `-32602` and the offered models are named

#### Scenario: An unacceptable effort is refused

- **WHEN** `model.effort` names a level outside the model's advertised set
- **THEN** it is refused with `-32602` and the accepted levels are named

#### Scenario: A change is recorded

- **WHEN** a session's model or effort changes
- **THEN** `model.changed` carries the previous and the new value

### Requirement: Usage is recorded as reported

Every completed model round-trip SHALL append `llm.completed` carrying the usage the endpoint
reported, including the cached prompt tokens separately from the rest, and a `purpose` of
`turn` or `compaction`.

#### Scenario: The cached share is carried

- **WHEN** an endpoint reports cached prompt tokens
- **THEN** `llm.completed` carries `cached_tokens` alongside `prompt_tokens`

#### Scenario: Summarisation cost is separable

- **WHEN** a compaction summarises turns
- **THEN** its `llm.completed` carries `purpose: compaction`

### Requirement: No invented cost

`cost_micros` SHALL be present only when the daemon can price the model. An unknown model
SHALL omit it rather than guess, and `session.usage` SHALL leave it absent.

#### Scenario: An unpriced model claims nothing

- **WHEN** a turn completes on a model with no known price
- **THEN** no cost is reported for it

### Requirement: The model's view is rebuilt from the log

A turn's transcript SHALL be reconstructed from the session's events, tool calls and results
included, with rig's history adaptation applied so every call the wire sees is answered.
Cross-turn history SHALL carry prompts, steering messages, assistant text and tool results.

#### Scenario: A second turn sees the first turn's work

- **WHEN** a second turn starts on a session that already ran tools
- **THEN** its request carries the earlier calls and results rather than prose alone

#### Scenario: An unanswered call does not reach the wire

- **WHEN** a previous turn was interrupted mid-call
- **THEN** the history adaptation repairs the pairing before the request is sent
