# Proposal: llm-chat-tool-turn-body

## Why

bd rc-a3u9 (agentic-lab E3, lab finding 14): in a multi-turn tool-calling
loop, the `llm:chat` producer in materialized mode (`stream=false`) returns
an EMPTY body for the final assistant reply when the turn carries tool calls
alongside text — the text lands only in the `CamelLlmText` header. Live
reproduction against local ollama `qwen3.5:4b` (2026-08-26) confirmed the
mechanism and an aggravator:

- `apply_materialized_result` (producer.rs) decides body-vs-header purely on
  `tool_calls.is_empty()`: any tool call in the turn forces
  `Body::Empty` + `CamelLlmText` header + `CamelLlmToolCalls` header. Small
  tool-calling models intermittently emit spurious/repeated calls on the
  final turn together with the answer text, so the final answer is not on
  the body.
- Aggravator confirmed live (lab finding 15): `qwen3.5:4b` emits the SAME
  tool call twice (identical id `call_0` twice in one assistant message).
  The producer performs no dedup, so the duplicated call re-enters the
  conversation history (`ChatRole::Assistant { tool_calls }`) on the next
  turn, which increases spurious re-emission by the model.

## What Changes

- `camel-component-llm` producer (`src/producer.rs`) only:
  - Materialized tool-turn body rule: when a materialized turn carries tool
    calls AND non-empty text, the exchange body SHALL be `Body::Text` with
    that text (the `CamelLlmToolCalls` and `CamelLlmText` headers keep their
    current meaning; agent loops keep dispatching on the header). Turns with
    tool calls and no text keep `Body::Empty`.
- Tool-call dedup by id (materialized turns): duplicate `EmittedToolCall`
  ids collected in one turn keep only the first occurrence (first-wins,
  also when the duplicate carries conflicting payload), with
  observability-level log notes (`debug!` for verbatim repeats, `warn!`
  for conflicting payloads).
- Contract doc: crate `CONTEXT.md` materialized-mode entry updated with the
  body/header matrix (text→Text body; tool turn with text→Text body +
  both headers; tool turn without text→Empty body + ToolCalls header).
- Excluded: streaming mode semantics, provider adapters (siumai/ollama),
  cache behavior, `llm:embed`, header names, the reshape/tool-dispatch
  contract of routes.

## Acceptance criteria

- Materialized turn with tool calls + text → body is `Body::Text(text)` AND
  `CamelLlmToolCalls` header present AND `CamelLlmText` header present
  (unchanged compatibility).
- Materialized turn with tool calls and empty text → body `Body::Empty`
  (unchanged).
- Materialized turn with text and no tool calls → `Body::Text` (unchanged).
- Duplicate tool-call ids in one turn → `CamelLlmToolCalls` carries each id
  once (first-wins).
- Existing crate tests stay green UNCHANGED — the multi-turn test's turn-1
  fixture emits a tool call with EMPTY text, so its `Body::Empty`
  assertion already matches the new contract; a new deterministic test
  covers the text-plus-tools turn.
- `cargo fmt --check`, `cargo clippy -p camel-component-llm --all-targets
  -- -D warnings` clean; crate test suite green.

## Risk budget

Low-moderate. Single crate, single function + collection loop; behavior
change is additive on the body (headers unchanged), so header-driven
consumers (agent loops, lab reshape) are unaffected. The turn-1 body
assertion change is an intentional contract amendment recorded in the spec
delta. Out of bounds: touching provider adapters, streaming, or the
route-owned tool-dispatch contract.
