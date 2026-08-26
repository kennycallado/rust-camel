## ADDED Requirements

### Requirement: Materialized tool-turn body carries accompanying text

The `llm:chat` producer in materialized mode (`stream=false`) SHALL place
the assistant text on the exchange body (`Body::Text`) whenever the turn
produces non-empty text, INCLUDING turns that also produce tool calls.
Tool-call turns SHALL still set the `CamelLlmToolCalls` header (the
route-owned dispatch signal) and the `CamelLlmText` header (compatibility).
A tool-call turn with no text SHALL leave the body `Body::Empty`.

#### Scenario: final turn with spurious tool call plus text sets the body

- **GIVEN** a materialized `llm:chat` producer and a provider that emits a
  tool call AND non-empty text in the same turn
- **WHEN** the producer call completes
- **THEN** the body is `Body::Text` containing the emitted text
- **AND** the `CamelLlmToolCalls` header lists the tool call
- **AND** the `CamelLlmText` header carries the same text

#### Scenario: tool turn without text keeps empty body

- **GIVEN** a materialized `llm:chat` producer and a provider that emits a
  tool call and no text
- **WHEN** the producer call completes
- **THEN** the body is `Body::Empty`
- **AND** the `CamelLlmToolCalls` header lists the tool call
- **AND** no `CamelLlmText` header is set

#### Scenario: text-only turn unchanged

- **GIVEN** a materialized `llm:chat` producer and a provider that emits
  text and no tool calls
- **WHEN** the producer call completes
- **THEN** the body is `Body::Text` with the text and no
  `CamelLlmToolCalls` header

### Requirement: Duplicate tool-call ids are deduplicated first-wins

In MATERIALIZED mode, the `llm:chat` producer SHALL deduplicate collected
tool calls by id within one turn, keeping the first occurrence and dropping
later duplicates (some models repeat the same call id verbatim — observed
with ollama `qwen3.5:4b`). This applies whether the duplicate carries
identical or conflicting name/arguments; the first occurrence always wins.
The `CamelLlmToolCalls` header SHALL list each id at most once.

#### Scenario: duplicated id with conflicting payload collapses to the first call

- **GIVEN** a materialized `llm:chat` producer and a provider that emits
  two `ChatEvent::ToolCall` events with the same id but different
  arguments (`{"city":"London"}` then `{"city":"Paris"}`)
- **WHEN** the producer call completes
- **THEN** the `CamelLlmToolCalls` header contains exactly one entry with
  that id, carrying the FIRST payload (`{"city":"London"}`)
