# Camel Webmaster Agent POC

This branch is a clean work branch for implementing the Camel Webmaster Agent on top of the AI POC.

## Base branch

`feature/ai-poc`

## Work branch

`feature/ai-poc-webmaster-agent`

## Goal

Implement a phase-0 observer/recommender agent that understands real rust-camel AI POC routes and emits structured maintenance proposals.

The agent must not execute autonomous changes and must not sit in the business hot path. It should observe route definitions, build a structured snapshot, and produce recommendations.

## Expected implementation

Suggested structure:

```text
crates/camel-agent/
  src/lib.rs
  src/types.rs
  src/snapshot.rs
  src/steward.rs

examples/webmaster-agent/
  Cargo.toml
  routes.yaml
  src/main.rs
  README.md
```

A runtime `agent:` component is optional and not required for this phase.

## Requirements

- Use the real `camel-dsl` parser and real declarative route model from this branch.
- Do not create a parallel fake YAML model with `steps: Vec<String>`.
- Do not target `main`; this work must be based on `feature/ai-poc`.
- Support AI POC route constructs explicitly:
  - `AiClassify`
  - `AiExtract`
  - `to: "llm:..."`
  - `to: "embedding:..."`
  - `to: "vector:..."`
- Walk nested route steps where available, such as `choice`, `split`, `multicast`, `load_balance`, `throttle`, and `loop`.
- Keep the basic mode offline: no OpenAI, Ollama, or Qdrant required.

## Snapshot types

The exact shape can evolve, but include at least:

```rust
pub struct SystemSnapshot {
    pub routes: Vec<RouteSnapshot>,
    pub components: Vec<ComponentSnapshot>,
    pub findings_context: serde_json::Value,
}

pub struct RouteSnapshot {
    pub id: String,
    pub from: String,
    pub steps: Vec<String>,
    pub components: Vec<String>,
    pub has_error_handler: bool,
    pub has_circuit_breaker: bool,
    pub uses_ai: bool,
}

pub struct MaintenanceProposal {
    pub kind: ProposalKind,
    pub severity: Severity,
    pub route_id: Option<String>,
    pub finding: String,
    pub recommendation: String,
    pub rationale: String,
    pub confidence: f32,
    pub requires_approval: bool,
}
```

Suggested `ProposalKind` values:

- `RouteReliability`
- `AiSafety`
- `Observability`
- `Documentation`
- `Testing`
- `Refactor`

## Heuristic rules

At minimum, implement rules for:

1. Route touches HTTP endpoint and lacks route-level error handling → propose retry/circuit breaker/dead-letter handling.
2. Route uses `ai_classify` or `ai_extract` → propose output validation and AI telemetry.
3. Route has RAG shape (`embedding` + `vector` + `llm`) → propose reusable `rag_answer` / `prompt_template` abstraction.
4. Route contains `api_key=` in endpoint/model URI → propose moving secrets to env/config/secrets.
5. Long route → propose documentation or template extraction.

## Required tests / fixtures

Use real YAML from this branch, not invented DSL:

- `examples/ai-ticket-router/routes.yaml`
- `examples/rag-basic/routes.yaml`
- `examples/rag-file/routes.yaml`

Tests should prove:

- `ai_classify` is detected as AI usage.
- `ai_extract` is detected as AI usage.
- model URI inside `ai_classify` / `ai_extract` contributes component `llm`.
- RAG routes detect `embedding`, `vector`, and `llm`.
- `api_key=` in model/endpoint URI triggers secret hardening proposal.

## Example

`cargo run -p webmaster-agent` should print a structured JSON snapshot and at least three maintenance proposals.

It should not require Ollama/OpenAI/Qdrant.

## Verification

Run at least:

```bash
cargo check -p camel-agent -p webmaster-agent
cargo test -p camel-agent
cargo run -p webmaster-agent
```

If possible, also run broader workspace checks for touched crates.
