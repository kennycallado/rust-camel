# Proposal: llm-decisions

Bd: rc-3lnv3

## Why

Routes often need a typed decision, not generated text. Examples: "is this ticket a
billing issue?", "which queue gets this order?", "how urgent is this alert?". Today a
route must call `llm:chat`, ask for JSON, and parse free text. That is fragile and
cannot expose calibrated probabilities.

Three backends now answer typed questions directly:

- OpenAI Decisions (`POST /v1/decisions`, generally available, model `gpt-6-luna`).
- TypeSafe.ai System One (`POST /v1/systemone`, model family Jev).
- Ollama local decision models (`POST /v1/systemone` on the Ollama server, for
  example `tev1:0.8b`). The wire format is the TypeSafe System One format.

The genai SDK (checked: 0.6.5, 0.7.0-rc.8, 0.7.2) has no decision support. The
component needs a project-owned decision seam now. A genai-backed adapter can
replace the direct HTTP adapters later without a change to routes or configuration.

## What Changes

Affected crate: `camel-component-llm` (plus `docs/src/components/llm.md`, the crate
`CONTEXT.md`, `CONTEXT-MAP.md`, and ADR-0020).

Included:

- A new `DecisionProvider` trait, separate from `LlmProvider`, with Camel-owned
  request, answer, usage, and capability types. Question kinds: Predicate, Choice,
  Score. Answers can be a per-question Refusal.
- Direct `reqwest` adapters for three provider types: `openai`, `typesafe`, and
  `ollama`. TypeSafe and Ollama share one System One wire codec. A deterministic
  `mock` decision provider.
- New configuration tables: `decision_providers`, `decision_question_sets`, and
  `default_decision_provider`. Operators declare question sets in trusted
  configuration.
- A new URI operation `llm:decide?questions=<set>[&provider=<name>][&model=<id>]`.
  The Exchange body is the decision state (text or JSON). The result is a JSON body
  that `choice`/`filter` EIPs read through existing languages.
- Optional explicit per-question thresholds (`threshold` on predicates,
  `min_confidence` on choice/score) that add `meets_threshold` /
  `meets_min_confidence` booleans to the answer. With no
  threshold, the component returns evidence only.
- Shared HTTP status mapping moves unchanged from `genai_adapter.rs` to a non-SDK
  module so that both the genai adapter and the decision adapters use it.

Excluded (deferred):

- Image inputs, TypeSafe `box` questions, conversation-history input, and OpenAI
  prompt-cache controls.
- Cost estimation for decisions (usage headers only).
- Question definitions from Exchange headers.
- A new EIP. Routes use the existing `choice` and `filter` EIPs.

## Acceptance criteria

- OpenAI, TypeSafe, Ollama, and mock providers implement one `DecisionProvider`
  contract for Predicate, Choice, and Score. Route logic has no vendor field names.
- Endpoint creation rejects unknown question sets, unknown decision providers, and
  question sets that exceed the provider capabilities. No network I/O occurs.
- Answers are correlated by question name. Missing, duplicate, unknown, wrongly
  typed, out-of-range, or non-finite answers cause `LlmError::Protocol`. They never
  become a false predicate.
- OpenAI refusals are a per-question `refusal` answer. They are not errors.
- Status mapping covers 401/403, 404, 400/413/415/422, 429 with Retry-After,
  `insufficient_quota`, and 5xx/529. Error text never contains the API key.
- Deterministic wiremock tests cover each question kind for each wire codec. A
  live Ollama test (`tev1:0.8b`) is `#[ignore]` and runs only through `--ignored`.
- Existing chat, embed, mock, and boundary tests stay green. The applicable
  AGENTS.md quality gates pass in the feature worktree.

## Risk budget

Acceptable: the two wire formats can change before they are stable. The codecs are
isolated in one file each, so a format change touches one file. Score scale is
0-based for all providers. That matches both wire formats today.

Out of bounds: changes to the `LlmProvider` trait or to chat/embed behavior, new
genai imports outside `genai_adapter.rs`, hidden conversion of probabilities to
booleans, and claims that typed answers are factually correct.
