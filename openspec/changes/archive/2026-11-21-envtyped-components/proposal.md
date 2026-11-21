# Proposal: envtyped-components

## Why

`[components.*]` typed leaves cannot be set from `${env:}` placeholders today.
`[components.redis] port = "${env:REDIS_PORT:-6379}"` resolves to the STRING
`"6379"`; `RedisBundle::from_toml` then runs `toml::Value::try_into::<RedisConfig>()`
and fails `invalid type: string, expected u16`. The same failure class applies to
the numeric/boolean leaves of every component config (`http`, `ws`, `mail`,
`opensearch`, `mqtt`, `jms`, `cxf`, `surrealdb`, … — 88 numeric leaves under
`crates/components/**/config.rs`). The root cause is seam placement:
`env-int-placeholder-typing` records provenance for the WHOLE resolved tree
(including `components.*`), but `deserialize_with_probe` runs only when
`CamelConfig` deserialization itself fails, and `ComponentsConfig.raw:
HashMap<String, toml::Value>` accepts any string — so the first pass always
succeeds for component-only documents and the probe never sees them. The bundle's
own `try_into` fails later with no provenance left.

## What Changes

- camel-config exposes the already-recorded components-subtree provenance:
  `ComponentsConfig` gains a serde-skipped, per-block provenance store and a
  `provenance_for(key)` accessor. `raw` keeps its type; values stay resolved
  strings.
- camel-config generalizes the probe's candidate-subset search into
  `deserialize_block_with_probe`, a public helper generic over the target type
  and error, with the same clean-integer/clean-boolean gates, the same
  smallest-first subset order, the same cap of 8, and the same
  original-error-on-no-success rule. The root-field TOML arm becomes a thin
  wrapper — its semantics and cap are unchanged.
- camel-bundles calls the helper at `bundle_from_config`: first-pass
  `from_toml(raw)`; on error, provenance-backed candidate leaves under
  `components.<config_key>` are coerced in a copy and retried through
  `from_toml`; the first-pass error is returned verbatim when no subset parses.
- End-to-end tests (config load → block provenance; bundle_from_config for
  redis, http, ws), docs (`env-interpolation.md` "Component configuration"),
  and the missing `camel-config` lane row in AGENTS.md.

NOT included: any `ComponentBundle::from_toml` change (48 impls untouched), any
camel-dsl surface (route-document seams are rc-3c67o.2), per-type schemas for
component blocks, a camel-config dependency on component crates, or a second
placeholder resolution pass.

## Acceptance criteria

- `bundle_from_config::<RedisBundle>` turns `port = "${env:REDIS_PORT:-6379}"`
  into `RedisConfig.port == 6379` (env value and default both verified).
- An `Option<u16>` leaf and a boolean leaf in a component block coerce; a
  numeric-looking string leaf stays a string; a literal quoted numeric still
  fails; an unrelated bundle error surfaces verbatim; two blocks each with one
  candidate both coerce.
- `components.raw` values remain resolved strings after a successful coercion.
- Workspace build, clippy registry, `gates-local.sh` (with the new camel-config
  lane) green; `openspec validate envtyped-components --type change` clean.

## Risk budget

Coercion runs only after a real deserialization failure, so documents that load
today are unaffected; the oracle is the bundle's own `try_into`, never a
heuristic. Accepted: the components-subtree provenance travels inside
`ComponentsConfig` (serde-skipped field, equality ignores it). Out of bounds:
changing probe gates/cap, resolving placeholders twice, touching camel-dsl.
