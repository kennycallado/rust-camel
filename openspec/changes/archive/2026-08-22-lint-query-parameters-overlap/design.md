# Design: lint-query-parameters-overlap

## Approach

Mirror the lowering's two fail-closed duplicate-key paths as a pre-lowering
lint diagnostic.

1. **Origin-tagged options** (`route_view.rs`): new
   `#[non_exhaustive] pub enum OptionOrigin { Query, StepParameters,
   ConfigParameters }` and a `LintOption.origin` field.
   `LintOption::parse_from_query` tags `Query`. `collect_parameters` takes
   the origin as an argument; `document.rs::walk` passes `StepParameters`
   for a mapping's own `parameters:` map (sibling of `to`/`from`/`uri`,
   including the route-level `from`) and `ConfigParameters` for the
   `parameters:` map collected inside the object-form URI-key recursion —
   the same vocabulary the lowering uses (`merge_endpoint_uri` params vs
   `combine_params(config, step)`). Inherited entries keep their tags
   through the `inherited ++ local` chain, so a nested object-form endpoint
   sees `[StepParameters.., ConfigParameters..]` after its query options.
2. **New sub-code** (`diagnostic.rs`): `UriKnownSubCode::DuplicateKey`
   renders as the stable string `R-URI-known:duplicate-key`; the
   stable-contract doc comment on the `Display` impl is extended.
   The enum is a plain `pub enum`; adding a variant only extends the
   in-crate `Display` match, no external exhaustive match breaks
   (`lint_production_catalog.rs` imports it but only constructs known
   variants).
3. **Rule logic** (`ruriknown.rs`): in `analyze_endpoint`, before the
   scheme lookup and its early returns, group `ep.options` by raw key
   string (no alias resolution — the lowering compares raw keys). For each
   key present in two or more distinct origins, emit ONE error with span
   on the redundant occurrence: the first parameters-side key occurrence
   in options order (step-level before config-local for nested
   object-form endpoints; the lowering errors naming the parameter key,
   and `combine_params` names the step key). The message names the key
   and the distinct sources in the fixed vocabulary `the URI query
   string` / `step parameters` / `config parameters`, e.g.
   ``duplicate option key `period`: declared in the URI query string and step parameters``.
   Repeated keys within the raw query string alone stay legal (the
   lowering preserves them in order). The check is catalog-independent:
   the collision fails lowering regardless of scheme knowledge, so it
   fires even for unverified schemes.

## Affected crates

- `camel-lint`: `route_view.rs` (origin enum + field), `document.rs`
  (tagging at the three collection points + existing `LintOption` test
  literals updated), `diagnostic.rs` (sub-code + Display + contract
  comment), `rules/ruriknown.rs` (the check + tests).
- `camel-cli`: expected NO change. Corpus baseline verified stable
  (`parameters-secret.yaml` carries no query strings; `examples/` use no
  `parameters:` maps). Touched only if the corpus gate shows drift.

## Architecture boundaries

Control-plane only, per ADR-0001 (Tower data plane / control plane split):
camel-lint stays runtime-free — no `camel-core`/`camel-dsl` dependencies
(route-lint spec; enforced by the hexagonal-architecture test). The change
reads the lowering semantics (`camel-dsl/src/yaml.rs::combine_params`,
`camel-api::EndpointUri::try_from_uri_and_params`) as the behavior to
mirror, without a code dependency. The DSL surface (ADR-0017, ADR-0026) is
untouched. camel-lsp consumes `Diagnostic` generically and inherits the
new diagnostic with zero LSP code changes.

## Alternatives considered

- **Two-way origin (Query | Parameters) + "same key twice on the
  parameters side" heuristic** — rejected: a CST duplicate key inside one
  `parameters:` map (which deserialization rejects or last-wins) would
  false-positive, and messages/spans cannot distinguish the two lowering
  fail-closes.
- **Positional inference** (query options always precede parameters
  entries in `Endpoint.options`) — rejected: an implicit ordering
  invariant, fragile under future view changes, and unable to separate
  step from config parameters.
- **New top-level code (e.g. `R-DUP`)** — rejected: unnecessary extension
  of the stable code taxonomy; bd rc-j9v8 asks for a `UriKnownSubCode`.
- **Auto-fix removing the redundant entry** — deferred: no rule emits a
  `Fix` today; v1 stays diagnostic-only.
