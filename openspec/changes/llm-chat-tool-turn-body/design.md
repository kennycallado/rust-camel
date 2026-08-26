# Design: llm-chat-tool-turn-body

## Approach

Two seams in `src/producer.rs`, both in the materialized chat path:

1. **Body rule in `apply_materialized_result`.** Today the branch is
   `if tool_calls.is_empty() { body = Text } else { body = Empty; text →
   CamelLlmText header }`. The fix: in the tool-calls branch, when
   `full_text` is non-empty, ALSO set `exchange.input.body =
   Body::Text(full_text)` (the header stays for compatibility). Empty text
   keeps `Body::Empty`. Rationale: agent loops dispatch on the
   `CamelLlmToolCalls` header (route-owned dispatch, ADR on tool
   vocabulary in CONTEXT.md "ToolDefinition / ToolChoice / ToolCall"), so
   putting the accompanying text on the body cannot hijack dispatch; it
   makes the final answer reachable on the standard surface when a model
   emits spurious calls with its answer (rc-a3u9 symptom). Text-only and
   no-text turns are unchanged.

2. **First-wins dedup in `run_provider_work`'s collector (materialized
   path only).** While folding `ChatEvent`s, keep a `HashSet<String>` of
   seen tool-call ids; a `ChatEvent::ToolCall` whose id was already
   collected is dropped: `tracing::debug!` when the repeat is verbatim
   (benign model quirk — qwen3.5:4b verified live), `tracing::warn!` when
   the same id carries a different name or arguments (conflicting payload
   the route author may want to know about). First occurrence always wins.
   This also keeps the re-sent conversation history clean (fewer spurious
   calls re-entering `ChatRole::Assistant { tool_calls }`), attacking the
   aggravator.

Design decision — why not fix it in the siumai adapter (drop duplicate
`ToolInputEnd` for the same id): the adapter's accumulator already keys
buffers by id and REMOVES on end; a duplicate end for the same id logs
"end without prior start". The observed duplication is two complete
start→delta→end sequences with the same id — provider-faithful transport of
a model quirk. Dedup belongs at the collection boundary (producer), one
place, provider-agnostic.

## Affected crates

- `camel-component-llm`: `src/producer.rs` — `apply_materialized_result`
  body rule (~6 lines), `run_provider_work` collector dedup (~10 lines);
  `src/producer_tools_tests.rs` — new deterministic mock tests
  (MockProvider emitting ToolCall + Delta + Finished in one turn;
  duplicate-id turns); `tests/multi_turn_tools.rs` — UNCHANGED (its turn-1
  fixture emits empty text, so the existing `Body::Empty` assertion already
  matches the new contract); `CONTEXT.md` — materialized-mode entry gains
  the body/header matrix.

## Architecture boundaries

- **Route-owned dispatch (CONTEXT.md tool vocabulary):** unchanged — the
  component never executes tools; `CamelLlmToolCalls` remains the dispatch
  signal. Body text is data-plane payload, not control.
- **Streaming/materialized split:** only materialized touched; streaming
  mode emits headers/Body::Stream as before.
- **No public API change:** `EmittedToolCall`, `ChatEvent`, headers all
  unchanged; behavior-only fix inside the producer.

## Phases

Single-phase: one coherent slice (body rule + dedup + tests), one crate,
no milestone-worthy ordering.
