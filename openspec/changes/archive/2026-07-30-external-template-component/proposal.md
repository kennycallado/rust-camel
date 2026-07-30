# Proposal: external-template-component

## Why

ADR-0047 lands template rendering in two stages. Stage 1 — the inline MiniJinja
Language (`crates/languages/camel-language-minijinja`) — shipped (squash
a42b5e69). It compiles template source embedded in route definitions only; it
cannot read files, resolve includes, or hot-reload. Full-page SSR and shared
template sets need those capabilities, but inline-only rendering cannot provide
them without crossing the Stage 1 trust boundary (no file/URI/network I/O).

This change delivers ADR-0047 Stage 2: an external-template Component
(`crates/components/camel-template`) that owns file loading, include resolution,
path policy, bounded acquisition, compiled-set caching, and atomic hot-reload.
The inline Language stays inline-only.

## What Changes

- New `crates/components/camel-template` Component with route-declared
  `template:file:///abs/path` URIs.
- Reuses Stage 1 MiniJinja via a newly extracted public `engine` module
  (`pub async fn render`), inheriting strict-undefined, explicit-autoescape, and
  the per-render bounds.
- Bounded acquisition (source/context/output/fuel/recursion/include dimensions),
  all fail-closed.
- Include/extends/import/from resolution with openat-relative path policy
  (statically discovered closure; no root escape; TOCTOU-safe).
- Atomic route-scoped hot-reload: stage all producers, commit only after every
  build succeeds; prior set retained on failure (TLS reload pattern).
- New control-plane `RuntimeCommand::ReloadTemplates` (non-lifecycle, mirrors
  `ReloadTlsCerts`).
- Extends the existing `StepLifecycle` trait in `camel-api` with an async
  `start()` hook (default no-op) for startup-time template compilation. Reload
  generation is handler-owned (on `ReloadHandler`), not threaded through
  `ProducerContext`.

**Excluded:** inline Language behavior changes, DSL `template:` route field,
`template:` Step (ADR-0047 non-goals).

## Acceptance criteria

1. Templates load from route-declared `template:file:///abs/path` URI.
2. Bare-path (`template:/abs/path`) and non-`file:` schemes rejected at endpoint
   construction with `CamelError::Config`.
3. Templates compile before activation (startup fail-closed; route → `Failed` on
   compile/path/bound error).
4. Requests reuse compiled state (compile-once; zero FS I/O on hot path).
5. Valid template change swaps atomically (`ArcSwap::store`); in-flight renders
   finish on the prior set.
6. Invalid reload preserves the prior set.
7. All bounded dimensions fail closed; exhaustion never truncates and reports
   success.
8. Includes/extends/import/from cannot escape the configured root.
9. No Exchange field overrides the resource URI (zero-override).
10. `ReloadTemplates` swaps the set without persisting lifecycle intent or
    mutating `RouteStatus`.

## Risk budget

Acceptable: a new Component crate, an additive `start()` hook on an existing
trait with a blanket no-op default, an additive `Endpoint::lifecycle()`
accessor with a default `None`, cfg-gated Unix/Windows path I/O.
Out of bounds: touching inline Language rendering semantics; template source
selected from Exchange data; silent truncation on bound exhaustion; changing
`create_producer`'s return type (additive accessor only).

**Affected crates:** `camel-api`, `camel-language-minijinja`,
`camel-component-api`, `camel-core`, `camel-template` (new), `camel-config`,
`camel-cli`.
**bd:** rc-64if.
