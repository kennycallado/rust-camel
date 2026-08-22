# Proposal: lint-query-parameters-overlap

## Why

The YAML/JSON DSL lowering fails closed when one endpoint declares the same
option key in two sources: the URI query string and a `parameters:` map
(`EndpointUriError::DuplicateKey`, `camel-api/src/endpoint_uri.rs`), or
`config.parameters` and step-level `parameters` (`combine_params`,
`camel-dsl/src/yaml.rs`). camel-lint sees both occurrences as plain
`LintOption`s on the same endpoint. Rules fire per occurrence
(unknown-option, secret, deprecated) with no conflict flag. The user learns
about the conflict only when the route is lowered, not while editing.
bd: rc-j9v8 (discovered in the task 3.1 review of rc-6vni).

## What Changes

- `LintOption` gains a source origin (`OptionOrigin`: query string / step
  parameters / config parameters). LintRoute construction tags origins at
  the collection points (`parse_from_query`, `collect_parameters` through
  the walk's inherited/local split).
- `UriKnownSubCode` gains `DuplicateKey` (stable string
  `R-URI-known:duplicate-key`). R-URI-known flags, per endpoint, any raw key
  that appears in more than one source: query ∩ parameters, or
  config.parameters ∩ step parameters. This mirrors the two lowering
  fail-closes exactly. Repeated keys inside the raw query string alone stay
  legal (the lowering preserves them in order).
- The check runs before the catalog early-returns (scheme lookup): the
  collision is a lowering failure independent of catalog knowledge.
- Delta spec: route-lint (MODIFIED requirement on option capture with
  origin; ADDED requirement for the duplicate-key diagnostic).
- Affected crates: `camel-lint` (view + diagnostic + rule), `camel-cli`
  (corpus baseline only if a fixture legitimately collides).
- Out of scope: RouteBuilder Rust DSL, LSP code changes (camel-lsp consumes
  diagnostics generically and inherits the new one unchanged), any change
  to lowering or `EndpointUri` semantics.

## Acceptance criteria

- A step `to: timer:foo?period=1000` with sibling
  `parameters: {period: "2500"}` emits `R-URI-known:duplicate-key` (Error)
  with a byte-exact span on the redundant occurrence, with no lowering run.
- The same key in `config.parameters` and step-level `parameters` emits the
  same diagnostic on the step occurrence.
- The same key repeated inside the query string alone emits no
  duplicate-key diagnostic.
- An unregistered scheme with a query/parameters overlap is still flagged.
- The lint corpus baseline stays unchanged unless a fixture has a true
  collision.
- camel-lint keeps no dependency on camel-core or camel-dsl.

## Risk budget

Low. One additive enum variant and one new field on `LintOption`
(workspace-internal consumers only). Accepted risk: the documented
`R-URI-known:<sub>` stable-contract extension by one sub-code. Out of
bounds: lowering, `EndpointUri`, LSP code, diagnostic-code renames.
