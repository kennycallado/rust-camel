# Tasks: llm-chat-tool-turn-body

## camel-component-llm (producer)

### Task 1.1: Materialized tool-turn body carries accompanying text

**Files:**
- `crates/components/camel-component-llm/src/provider/mock.rs` (modified)
- `crates/components/camel-component-llm/src/producer.rs` (modified)
- `crates/components/camel-component-llm/src/producer_tools_tests.rs` (modified)

**Steps:**
1. In `src/provider/mock.rs`, add a second tool-emission field beside the
   existing `tool_call: Option<(String, String, String)>` (line ~63):
   `tool_calls_with_text: Option<(String, Vec<(String, String, String)>)>`
   (text + ordered calls); initialize it to `None` in
   `MockProvider::new` (its initializer enumerates every field, ~line
   127-134). Add builder
   `pub fn with_tool_calls_and_text(mut self, text: impl Into<String>, calls: Vec<(&str, &str, &str)>) -> Self`
   that stores owned triples. In `chat_stream`, clone the new field into
   the `async_stream::stream!` closure like `tool_call` is cloned at
   ~line 272. Add a branch (before the existing `tool_call` branch at
   ~line 313): when
   `tool_calls_with_text` is Some, yield `Ok(ChatEvent::Delta { text })`
   first, then one `Ok(ChatEvent::ToolCall { id, name, arguments })` per
   stored call, then `Ok(ChatEvent::Finished { usage: Some(LlmUsage {
   prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }), model:
   Some(model), finish_reason: Some(FinishReason::Stop), metadata:
   serde_json::Map::new() })`, set `finished`, and return — mirroring the
   existing tool_call branch's structure.
2. In `src/producer.rs` `apply_materialized_result` (the `else` branch,
   ~line 770): keep the `CamelLlmText` header insert and the
   `Body::Empty` assignment, but change the body rule to: if `!full_text
   .is_empty()` → `exchange.input.body = Body::Text(full_text.clone())`
   (the header insert keeps using `full_text`; clone once, or restructure
   so the header borrows before the body takes ownership). Empty text →
   `Body::Empty` unchanged. Update the branch's comment to state the
   contract: dispatch rides the `CamelLlmToolCalls` header; the body
   carries accompanying text when present.
3. In `src/producer_tools_tests.rs`, add three tests following the file's
   own idiom: config (`stream: false`, `LlmOperation::Chat`) +
   `LlmProducer::new(config, provider, 32768, "test-route".into()).build()`
   + `producer.handle_chat(&mut exchange).await` (the pattern of the
   sibling tests at lines 61/106/146/176 — NOT tower Service::call;
   assertions read `exchange.input.body` / headers afterwards).
   `make_exchange` comes from `producer_test_helpers` (already imported).

**Tests:** (executable spec — name, arrange, act, assert)
- `tool_turn_with_text_sets_body_and_headers` (spec scenario "final turn
  with spurious tool call plus text sets the body"): MockProvider with
  `.with_tool_calls_and_text("The answer is 42.", vec![("call_1",
  "get_weather", r#"{"city":"London"}"#)])` → `producer.handle_chat(&mut exchange).await` → assert body
  is `Body::Text` whose string == "The answer is 42."; `CamelLlmToolCalls`
  header parses to one `EmittedToolCall` with id "call_1"; `CamelLlmText`
  header == "The answer is 42.". Command:
  `cargo test -p camel-component-llm --lib tool_turn_with_text_sets_body_and_headers`.
  Expected: FAIL before step 2 (body is Empty), PASS after.
- `tool_turn_without_text_keeps_empty_body` (spec scenario "tool turn
  without text keeps empty body"): MockProvider with existing
  `.with_tool_call("call_1", "get_weather", r#"{"city":"London"}"#)` →
  `producer.handle_chat(&mut exchange).await` → assert body is `Body::Empty`; `CamelLlmToolCalls`
  header lists the call; NO `CamelLlmText` header present. Command:
  `cargo test -p camel-component-llm --lib tool_turn_without_text_keeps_empty_body`.
  Expected: PASS before and after (regression guard).
- `text_only_turn_sets_body` (spec scenario "text-only turn unchanged"):
  MockProvider `MockMode::Fixed("plain answer")` (no tool calls) →
  `producer.handle_chat(&mut exchange).await` → assert body is `Body::Text("plain answer")` and no
  `CamelLlmToolCalls` header. Command:
  `cargo test -p camel-component-llm --lib text_only_turn_sets_body`.
  Expected: PASS before and after (regression guard).

**Acceptance:**
- `cargo check -p camel-component-llm --all-targets` exits 0
- `cargo test -p camel-component-llm` exits 0 (new + existing, including
  `tests/multi_turn_tools.rs` UNCHANGED — its turn-1 fixture emits empty
  text so `Body::Empty` still holds)
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-llm --all-targets -- -D warnings` exits 0

- [x] 1.1

### Task 1.2: Duplicate tool-call ids deduplicated first-wins (materialized)

**Files:**
- `crates/components/camel-component-llm/src/producer.rs` (modified)
- `crates/components/camel-component-llm/src/producer_tools_tests.rs` (modified)

**Steps:**
1. In `src/producer.rs` `run_provider_work`'s collector closure (the
   `while let Some(event) = stream.next().await` loop at ~line 694, inside
   the function starting at ~669; the `ChatEvent::ToolCall` arm is at
   ~703): declare ONCE, beside the `tool_calls` vec (same scope as
   `let mut tool_calls: Vec<EmittedToolCall> = Vec::new();`):
   `let mut seen_tool_ids: std::collections::HashSet<String> =
   std::collections::HashSet::new();` (fully qualified — no import edit).
   In the `ChatEvent::ToolCall { id, name, arguments }` arm, before
   pushing: if `!seen_tool_ids.insert(id.clone())` (id already present):
   compare against the first occurrence via
   `tool_calls.iter().find(|tc| tc.id == id)` — if name or arguments
   differ, `tracing::warn!` (mirroring the adjacent collector log's field
   syntax `route_id = %rid, id = %id`, message "duplicate tool call id
   with conflicting payload dropped; first wins"); else
   `tracing::debug!` (same fields, "duplicate tool call id dropped");
   then `continue` WITHOUT pushing.
2. In `src/producer_tools_tests.rs`, add the dedup test.

**Tests:**
- `duplicate_tool_call_ids_dedup_first_wins` (spec scenario "duplicated id
  with conflicting payload collapses to the first call"): MockProvider with
  `.with_tool_calls_and_text("Done.", vec![("call_1", "get_weather",
  r#"{"city":"London"}"#), ("call_1", "get_weather",
  r#"{"city":"Paris"}"#)])` (the mock extension from task 1.1 emits both,
  in order) → `producer.handle_chat(&mut exchange).await` → assert `CamelLlmToolCalls` header parses to
  EXACTLY one `EmittedToolCall` with id "call_1" and arguments
  `{"city":"London"}` (the FIRST payload). Command:
  `cargo test -p camel-component-llm --lib duplicate_tool_call_ids_dedup_first_wins`.
  Expected: FAIL before step 1 (two entries), PASS after.

**Acceptance:**
- `cargo test -p camel-component-llm` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-llm --all-targets -- -D warnings` exits 0

- [x] 1.2

### Task 1.3: CONTEXT.md body/header matrix

**Files:**
- `crates/components/camel-component-llm/CONTEXT.md` (modified)

**Steps:**
1. In the `## Glossary` section's `Materialized mode` entry (~line 29),
   replace the single sentence about `Body::Text` with a compact matrix in
   STE prose (3 short sentences, no new sections): text-only turn →
   `Body::Text`; tool-call turn with accompanying text → `Body::Text`
   (dispatch still rides the `CamelLlmToolCalls` header, which the route
   owns) + `CamelLlmText` header; tool-call turn without text →
   `Body::Empty`. Add one sentence: duplicate tool-call ids within one
   materialized turn are deduplicated first-wins (identical repeats at
   `debug`, conflicting payloads at `warn`).

**Tests:**
- `context_md_materialized_matrix_lint_clean` (free-form name): setup =
  crate CONTEXT.md edited as in step 1 → action = run
  `cargo xtask lint-context-citations` in the worktree → assert = exit
  code 0. Expected: PASS before and after (the lint accepts the edit;
  it is a guard, not a TDD transition).

**Acceptance:**
- Diff limited to `crates/components/camel-component-llm/CONTEXT.md`;
  only the `Materialized mode` entry changed
- `cargo xtask lint-context-citations` exits 0

- [x] 1.3
