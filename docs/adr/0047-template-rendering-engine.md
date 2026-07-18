# ADR-0047: MiniJinja Template Rendering Language

**Date:** 2026-07-18
**Status:** Accepted (implementation pending in bd rc-ao5)
**Issue:** bd rc-ao5
**Amends:** none
**Cross-refs:** ADR-0030 (Exchange-aware extension hooks), ADR-0033 (fail-closed security defaults), ADR-0039 (configurable resource caps), ADR-0046 (Apache Camel as design input, not conformance authority)

## Context

`rc-ao5` began on 2026-05-31 as an HTML/SSR processor for HTTP routes. The
2026-07-18 investigation found a broader structured-output gap: LLM prompts,
HTML, email/documents, HTTP producer bodies, and OpenSearch JSON DSL all need
conditional sections, loops, filters, and multiline output. LLM is a strong
existing consumer because the producer builds prompts and message history from
the Exchange (`crates/components/camel-component-llm/src/producer.rs:164-186,
228-252`) and the rendered prompt affects cache identity
(`crates/components/camel-component-llm/src/producer_cache.rs:266-325`).
OpenSearch is a weaker consumer because rendered JSON still requires an
explicit JSON unmarshal before its producer accepts the body
(`crates/components/camel-opensearch/src/producer/mod.rs:287-298`).

Inline source is sufficient for prompts, JSON snippets, and HTML fragments, but
not for the original full-page SSR case. A page containing shared layout,
styles, navigation, and footer must remain an external resource. Consequently,
the inline Language is one delivery slice of `rc-ao5`, not by itself completion
of the original use case.

This is not a missing Step or DSL shape. `transform:` is already an alias for
`set_body:`, and `SetBodyConfig` already accepts generic `language` and `source`
(`crates/camel-dsl/src/route_ast.rs:426-464`). The canonical API model is
`LanguageExpressionDef { language, source }`
(`crates/camel-api/src/declarative.rs:16-25`). Existing `simple:`, `rhai:`,
`jsonpath:`, and `xpath:` fields are compatibility conveniences, not a pattern
to extend.

Simple remains the right language for interpolation and predicates. It has
comparison plus binary `and`/`or`, but no unary `not` token or production
(`crates/languages/camel-language-simple/src/parser.rs:38-42,69-81,379-421`),
and its evaluator deliberately excludes arithmetic and string-concatenation
operators (`crates/languages/camel-language-simple/src/evaluator.rs:84-93`). A
rendering language fills a different role: producing structured text with
blocks, loops, filters, macros, and context-aware escaping.

The proposal is ADR-worthy: backend and crate boundaries are costly to reverse,
the Language/Component split is non-obvious, and viable engines and placements
have materially different security and lifecycle properties.

## Decision

Adopt **Option C, staged**.

### Phase 1: inline `minijinja` Language

Add `crates/languages/camel-language-minijinja`, backed by MiniJinja. It
implements the existing `Language`, `Expression`, and `Predicate` contracts
(`crates/languages/camel-language-api/src/lib.rs:21-67`). Route usage is:

```yaml
set_body:
  language: minijinja
  source: |-
    {% autoescape "html" %}
    <h1>Hello {{ headers.name }}</h1>
    {% endautoescape %}
```

No `template:` field and no `template:` Step are added. Runtime route
resolution calls `create_expression`/`create_predicate` once and stores the
compiled object (`crates/camel-core/src/lifecycle/adapters/step_resolution.rs:59-86`);
the MiniJinja AST/environment therefore belongs to that compiled object and is
reused for every Exchange.

Phase 1 accepts inline, configuration-authored source only. It does not install
a template loader and cannot read files, resolve URIs, or perform network I/O.
Rendering is CPU-local. MiniJinja 2.21 exposes a synchronous render API; that is
compatible with the async Language SPI because Phase 1 performs no blocking
I/O. Async resource acquisition must not be hidden inside `Expression::evaluate`.
Template source is never selected from an Exchange body, header, or property.

### Escape-mode contract

There is no global HTML default. Each rendering expression must explicitly
select its output context with a top-level MiniJinja `autoescape` block:
`"html"`, `"json"`, or `"none"`. Phase 1 rejects a rendering template without
an explicit top-level mode. `none` is an explicit operator decision, not an
implicit fallback. URL fragments use MiniJinja's `urlencode` filter; there is no
generic shell or SQL escaping mode.

This keeps the generic `language/source` DSL unchanged while preventing an HTML
default from corrupting JSON, URL, prompt, or plain-text output. It also avoids
extension-based inference: inline templates have no trustworthy filename.

### Security model

- Template source comes from route configuration only. Message headers cannot
  replace source or select a template resource. An external source URI declared
  on a template Component Endpoint is configuration; bytes arriving through an
  inbound Message are not. Apache Camel's FreeMarker, Mustache, and Velocity
  components use the same default
  (`allowTemplateFromHeader=false`) because header-selected templates cross an
  untrusted-data boundary.
- Undefined values are errors. The language configures MiniJinja strict
  undefined behavior; missing `body`, header, or property paths fail evaluation
  rather than silently rendering an empty string.
- Template context exposes only `body`, `headers`, and Exchange properties.
  CamelContext, registries, filesystem, network, environment variables, and
  arbitrary host objects are not exposed.
- Template source bytes, serialized context bytes, execution fuel, recursion
  depth, and output bytes all have non-zero finite defaults. Source/context
  limits apply before compilation or rendering; fuel does not substitute for
  either. `Body::Stream` context is rejected with guidance to add `stream_cache`,
  whose materialization is independently bounded
  (`crates/camel-processor/src/stream_cache.rs:37-54`). Limit exhaustion fails
  startup, reload, or evaluation; it never truncates and reports success.
  Limits are configurable with bounded defaults, following ADR-0033 and
  ADR-0039. MiniJinja's `fuel` feature supplies per-render instruction accounting
  and its environment supplies a recursion limit; rust-camel supplies bounded
  input accounting and an output writer.
- Loader, dynamic evaluation, and host-capability features remain disabled.
  This mirrors Rhai's separation of unconditional capability sandboxing from
  configurable DoS limits (`crates/languages/camel-language-rhai/src/lib.rs:6-43,
  45-76`).

### Phase 2: external-template Component

Full-page SSR confirms demand for external templates. Add
`crates/components/camel-template` under bd `rc-64if`, linked
`discovered-from:rc-ao5`; `rc-ao5` is not complete until this slice is delivered.
The Component owns route-declared URI/file loading, include resolution, path
policy, bounded source acquisition, compiled-template caching, invalidation, and
hot reload. It renders the current Exchange as context without replacing the
body with template bytes first.

Compiled state is bounded by entry count and total source bytes. Normal requests
reuse it without parsing; only a changed resource builds replacement state.
Compilation occurs off the active snapshot, and a successful complete build is
the sole swap point.

The Endpoint resolves its resource URI at route compilation/startup. Incoming
body, headers, and properties cannot override that URI. Reload reads a complete
bounded snapshot, compiles and validates it, then swaps only a fully compiled
template set; failure preserves the prior set, matching the TLS reload pattern
(`crates/components/camel-component-api/src/tls_source.rs:137-140`,
`crates/components/camel-component-grpc/src/tls_reload.rs:40-60`).

MiniJinja's current extension point is `Environment::set_loader`, a synchronous
loader callback with environment caching. It is useful inside the future
Component, but it is not an async `Source` trait and does not by itself solve
TOCTOU, traversal policy, cache invalidation, or lifecycle. Those concerns are
why external loading does not belong in the Language crate.

Composing `camel-file` with `pollEnrich` remains useful for ordinary content
enrichment but is not the template-loading contract. `camel-file` returns a
single-consumption `Body::Stream` (`crates/components/camel-file/src/poll_logic.rs:375-422`),
and `pollEnrich` serializes access to one mutable PollingConsumer
(`crates/camel-processor/src/content_enricher.rs:86-97`). The default strategy
replaces the original body while retaining its headers/properties
(`crates/camel-processor/src/enrichment_strategy.rs:24-36`); there is no
`USE_ORIGINAL` DSL strategy
(`crates/camel-core/src/lifecycle/adapters/step_resolution.rs:175-185`). This
composition would require body preservation, `stream_cache`, runtime template
compilation, and unverifiable data-provenance assertions. The Component avoids
all five concerns.

### Backend selection

MiniJinja is selected because it provides Jinja2-compatible syntax, dynamic
Serde-compatible values, strict undefined behavior, configurable autoescape,
per-render fuel, recursion limits, a loader extension point for Phase 2, and a
small dependency/compile-time goal. These directly match dynamic Exchange data
and the Rhai sandbox precedent.

Current evidence does **not** support two rationales from the initial review:
MiniJinja 2.21 is not async-native, and its public API does not expose a
`Source` trait. Neither is required by this decision: inline render is
synchronous and loader I/O belongs to the future Component. The previously
stated “same maintainer as Askama” claim is also not used as decision evidence.

## Non-goals

- **SQL text rendering.** `camel-sql` already binds named, expression,
  positional, and expanding-IN parameters
  (`crates/components/camel-sql/src/query.rs:37-52,248-265`). Rendering SQL text
  would replace parameter binding with injection-prone string construction.
- **Replacing Simple.** Simple remains the default lightweight interpolation
  and predicate language.
- **Replacing `camel-xslt` or `camel-xj`.** They retain their XML/XSLT and
  XML/JSON bridge-based transformation contracts
  (`crates/components/camel-xslt/src/producer.rs`,
  `crates/components/camel-xj/src/producer.rs`).
- **External includes, inheritance, or hot reload in the Language crate.** These
  require the Phase 2 Component. Inline macros remain available.
- **Treating shell command construction as a safe rendering target.** No
  context-free escaping rule can make arbitrary shell text safe.

## Consequences

### Positive

- One Language crate unblocks LLM prompts, HTML fragments, and inline
  email/document output; HTTP and OpenSearch can opt in without component
  coupling. The Component completes full-page SSR.
- Existing `set_body`/`transform` and `language/source` contracts remain
  canonical. No schema, AST, or Step proliferation occurs.
- Templates compile at route compilation, so syntax and explicit escape-mode
  failures prevent route startup rather than appearing on first traffic.
- Capability isolation and resource limits are explicit, testable contracts.

### Negative

- External templates require Phase 2, so users see a Language crate for inline
  rendering and a Component crate for resource lifecycle.
- Strict undefined values and mandatory escape declarations reject templates
  accepted by permissive Jinja deployments; migration is intentional.
- Output-byte accounting needs a rust-camel bounded writer in addition to
  MiniJinja's built-in fuel and recursion controls.

### Neutral

- `minijinja` becomes a workspace dependency and its selected features become
  part of dependency and MSRV governance.
- Rendered JSON remains text until an explicit unmarshal step converts it to a
  structured body.

## Alternatives Considered

- **Add `template:` to `SetBodyConfig`** — rejected. It duplicates canonical
  `language/source` and perpetuates legacy per-language convenience fields.
- **Add a `template:` Step** — rejected. Body-value transformation already
  belongs to `set_body`/`transform`; a second Step creates duplicate compiler,
  schema, and runtime paths.
- **Tera backend** — rejected. Tera's public render path is synchronous and it
  lacks MiniJinja's per-render fuel control. MiniJinja is also synchronous in
  current releases, so synchrony alone is not the discriminator; enforceable
  execution bounds and lean embedding are.
- **Handlebars backend** — rejected. Its logic-light model is useful for simple
  substitution but requires custom helpers for transformations naturally
  expressed by Jinja filters and expressions. That weakens portability for the
  LLM/document use cases.
- **Askama backend** — rejected. Askama derives a template implementation for a
  compile-time Rust struct or enum. Route-authored templates and dynamic
  Exchange maps require runtime compilation and runtime-shaped context.
- **Put rendering in `BeanProcessor`** — rejected. `BeanProcessor` dispatches a
  named method with parameters (`crates/camel-bean/src/processor.rs:5-22`); it is
  not the expression-language registry or compilation boundary.
- **Put rendering in WASM** — rejected. It adds serialization/ABI crossings and
  a broader capability policy to a deterministic in-process text operation.
  WASM remains available when isolation or non-Rust template code is itself a
  requirement.
- **Put external loading in the Language crate** — rejected. Filesystem and URI
  access add async I/O, traversal policy, TOCTOU behavior, cache invalidation,
  and reload lifecycle. A loader callback is not a lifecycle boundary.
- **Load template bytes with `camel-file` + `pollEnrich`, then render from the
  Exchange** — rejected as the template contract. `file:` Endpoint paths denote
  directories and exact selection uses `fileName`
  (`crates/components/camel-file/src/lib.rs:563-580`); the returned body is a
  single-consumption stream, default enrichment replaces application data, and
  current DSL exposes only `useEnrichedBody`/`throwOnNoPoll`. More importantly,
  bodies and properties have no intrinsic trust label. A route can copy
  adversarial Message data into either, so compile-time validation of
  `template_from: property.x` would assert provenance the runtime does not
  track (`crates/camel-api/src/exchange.rs:54-65,115-123`).

## Self-grill Record

**Questions generated:**

1. [glossary] Does “template” conflict with an existing domain term?
2. [sharpen] Are rendering and external resource loading one responsibility?
3. [scenario] What happens when one global HTML autoescape policy renders JSON,
   a prompt, or plain text?
4. [cross-ref] Does the current DSL already express this operation, and when is
   source compiled?
5. [cross-ref] Do the recorded MiniJinja API claims match the current public API?

**Answers (with citations):**

1. [glossary] Yes. The route AST already uses “templates” for route-definition
   expansion (`crates/camel-dsl/src/route_ast.rs:17-21`). This ADR uses
   **rendering Language** for inline evaluation and **external-template
   Component** for resource lifecycle, avoiding a third undifferentiated
   “template” concept (`CONTEXT-MAP.md:7-13,20-23`).
2. [sharpen] No. `Language` creates compiled expressions/predicates and evaluates
   them against an Exchange (`crates/languages/camel-language-api/src/lib.rs:21-67`);
   Components own URI-scheme integration and lifecycle (`CONTEXT-MAP.md:8,21-23`).
   The proposal is refined into two phases and two crate boundaries.
3. [scenario] HTML escaping changes quotes, angle brackets, and ampersands; that
   can invalidate JSON or alter prompt/plain-text semantics. Conversely,
   unescaped HTML permits markup injection. Mandatory per-expression
   `html`/`json`/`none` declaration resolves both cases; URL fragments require a
   context-specific filter. MiniJinja exposes `AutoEscape::Html`,
   `AutoEscape::Json`, and `AutoEscape::None` (MiniJinja 2.21 API, References).
4. [cross-ref] Yes. `SetBodyConfig.language/source` already exists
   (`crates/camel-dsl/src/route_ast.rs:450-464`), and route resolution invokes
   `create_expression`/`create_predicate` before execution
   (`crates/camel-core/src/lifecycle/adapters/step_resolution.rs:59-86`). New DSL
   fields and Steps are dropped.
5. [cross-ref] Partly. Current MiniJinja has synchronous `Template::render` and
   `Environment::set_loader`, not an async render API or public `Source` trait.
   Fuel, recursion, strict undefined, and explicit autoescape are present. The
   decision is refined to rely only on verified APIs and to keep loader I/O in
   Phase 2 (MiniJinja 2.21 API, References).

**Outcome:** refine — retain MiniJinja and the staged decision; make escape-mode
selection enforceable, separate rendering from resource lifecycle, and remove
unsupported async-native, `Source`-trait, and shared-maintainer rationales.

**Self-grill mode:** self-grill-proposals skill

### Revision self-grill record (2026-07-18)

**Questions generated:**

1. [glossary] Is an external template resource the same concept as a dynamic
   Exchange-sourced template?
2. [sharpen] Can acquisition, compilation, and rendering share the Language SPI
   without hiding I/O or provenance?
3. [scenario] Does `pollEnrich` preserve application data while supplying a
   reusable exact-file template under concurrent HTTP traffic?
4. [cross-ref] Can the DSL and Language SPI represent `template_from` and
   `context_from` today?
5. [cross-ref] Does `camel-file` provide template hot reload?

**Answers (with citations):**

1. [glossary] No. A route-declared Component URI is configuration; an Exchange
   body/header/property is runtime Message data. Exchange properties are merely
   processor-to-processor storage, not a trust domain
   (`crates/camel-api/src/exchange.rs:54-65,115-123`).
2. [sharpen] No. `Language::create_expression` accepts one static script and
   `Expression::evaluate` receives an Exchange
   (`crates/languages/camel-language-api/src/lib.rs:21-25,50-54`). Resource I/O,
   cache lifecycle, and atomic reload remain Component responsibilities.
3. [scenario] No. `UseEnrichedBody` replaces the original body
   (`crates/camel-processor/src/enrichment_strategy.rs:24-36`), file content is a
   single-consumption stream, and the shared poller is mutex-serialized
   (`crates/camel-processor/src/content_enricher.rs:86-97`). `USE_ORIGINAL` is
   not supported.
4. [cross-ref] No. `SetBodyConfig` and `LanguageExpressionDef` carry only the
   existing static `language/source` form
   (`crates/camel-dsl/src/route_ast.rs:450-464`,
   `crates/camel-api/src/declarative.rs:16-25`). Adding two MiniJinja-specific
   fields would reverse the no-DSL-proliferation decision.
5. [cross-ref] No. `FilePollingConsumer` rescans on `receive` and keeps a local
   seen set (`crates/components/camel-file/src/polling_consumer.rs:55-92`);
   `ModificationDetectingStream` only reports a file changed during one read
   (`crates/components/camel-file/src/poll_logic.rs:28-93`). It does not compile,
   atomically swap, or lifecycle-manage template sets.

**Outcome:** drop dynamic Exchange-sourced Phase 1; refine Phase 2 from optional
future work to the required external-template slice of `rc-ao5`.

**Self-grill mode:** self-grill-proposals skill

## References

- `crates/languages/camel-language-api/src/lib.rs:21-67`
- `crates/languages/camel-language-rhai/src/lib.rs:6-76`
- `crates/camel-dsl/src/route_ast.rs:426-464,500-514`
- `crates/camel-api/src/declarative.rs:16-25`
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs:59-86`
- `crates/components/camel-component-llm/src/producer.rs:164-186,228-252`
- `crates/components/camel-component-llm/src/producer_cache.rs:266-325`
- `crates/components/camel-opensearch/src/producer/mod.rs:287-298`
- `crates/components/camel-sql/src/query.rs:37-52,248-265`
- `crates/components/camel-file/src/lib.rs:563-580`
- `crates/components/camel-file/src/polling_consumer.rs:55-92`
- `crates/components/camel-file/src/poll_logic.rs:28-93,375-422`
- `crates/camel-processor/src/content_enricher.rs:86-97`
- `crates/camel-processor/src/enrichment_strategy.rs:24-36`
- `crates/camel-processor/src/stream_cache.rs:37-54`
- [MiniJinja 2.21 crate API](https://docs.rs/minijinja/2.21.0/minijinja/)
- [MiniJinja `Environment` API](https://docs.rs/minijinja/2.21.0/minijinja/struct.Environment.html)
- [MiniJinja `Template` API](https://docs.rs/minijinja/2.21.0/minijinja/struct.Template.html)
- [Tera 2.0 `Tera` API](https://docs.rs/tera/2.0.0/tera/struct.Tera.html)
- [Handlebars 6.4 `Handlebars` API](https://docs.rs/handlebars/6.4.3/handlebars/struct.Handlebars.html)
- [Askama 0.16 crate API](https://docs.rs/askama/0.16.0/askama/)
- [Apache Camel FreeMarker component](https://camel.apache.org/components/4.18.x/freemarker-component.html)
- [Apache Camel Mustache component](https://camel.apache.org/components/4.18.x/mustache-component.html)
- [Apache Camel Velocity component](https://camel.apache.org/components/4.18.x/velocity-component.html)
- `docs/adr/0030-exchange-aware-dataformat-hooks.md`
- `docs/adr/0033-security-defaults-fail-closed-startup-validation.md`
- `docs/adr/0039-configurable-loop-iteration-cap.md`
- `docs/adr/0046-apache-camel-inspiration-not-conformance.md`

## Revisions

- **2026-07-18:** Confirmed external full-page templates as required scope;
  rejected Exchange-sourced dynamic templates and promoted the external-template
  Component from optional follow-up to required `rc-ao5` delivery.
