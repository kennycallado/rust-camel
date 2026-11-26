# Environment variable interpolation

Substitute environment variables into route files and `Camel.toml` with
`${env:VAR}` tokens. The tokens work in endpoint URIs, log messages,
header values, and any other string field.

The expansion point differs by surface. Route files expand in the parsed
YAML tree: the loader walks the tree and substitutes every string scalar
first (`interpolate_yaml_source`), and only an unparseable document falls
back to raw-text substitution. `Camel.toml` expands every string
leaf of the parsed, merged tree. The tree combines the main file,
include files, and `CAMEL_*` environment overrides. Expansion runs
before typed deserialization.

## Syntax

```yaml
{{#include ../../../examples/env-interpolation/routes/routes.yaml:env-routes}}
```

`${env:VAR}` reads the variable `VAR`. `${env:VAR:-default}` uses
`default` when `VAR` is unset. An unset variable with no default fails
load. The error names the variable. Set a default or export the variable
to avoid the failure.

The same syntax works in `Camel.toml`:

```toml
[security.keycloak]
client_secret = "${env:KC_SECRET}"

[observability.otel]
endpoint = "${env:OTEL_ENDPOINT:-http://localhost:4317}"
```

## Typed values

String fields keep the resolved text on every surface. Integer-typed fields
already coerce through the typed probe after a failed strict parse, as
[`camel-config` CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
records.

Boolean-typed fields coerce only when the resolved value is exactly `true`
or exactly `false`. The check is byte-exact and lowercase:

```toml
watch = "${env:WATCH:-true}"
```

```yaml
auto_startup: ${env:AUTO_START:-false}
```

Any other spelling fails typed deserialization — for example `TRUE`,
`yes`/`no`, `1`/`0`, or surrounding whitespace. On `Camel.toml`, the error
names the field. In route files, the load fails with the original parse
error. A literal quoted `"true"` with no placeholder never coerces, because
it carries no token.

## Component configuration

A `[components.*]` block holds untyped TOML that each bundle parses itself.
Environment tokens resolve in those leaves like any other `Camel.toml`
string:

```toml
[components.redis]
port = "${env:REDIS_PORT:-6379}"
```

String leaves keep the resolved text. Integer and boolean leaves follow the
rule in [Typed values](#typed-values) — the same clean-value gates — applied
at the bundle seam. The bundle parses its block first. Only when that parse
fails does `camel-config` coerce provenance-backed clean integer and boolean
leaves of that block and retry, with at most eight candidates per component
block. The root-field probe still counts the whole tree, so a `[components.*]`
block never changes root-field behavior. A block with more than eight
candidate leaves keeps the bundle's first-pass error, verbatim.

`ComponentsConfig.raw` keeps the resolved strings. Resolution runs at config
load, and coercion works on cloned trees, so `raw` never holds a coerced
type or a rewritten value.

**Stale provenance:** a caller that edits `ComponentsConfig.raw` after load
must rebuild the config — default-construct `ComponentsConfig`, then assign
the edited map. Rebuilding drops the metadata that records which leaves
carried a token, so no stale path coerces a token-free value. That metadata
is never a value and never affects config equality.

## Route documents

Route documents probe typed leaves after a failed first parse. The candidate
set spans every subtree except the two [stay-out subtrees](#stay-out-subtrees);
in practice it covers `routes:`, `rest:`, and `mcp:`. A candidate is a
whole-scalar `${env:}` substitution whose resolved text is a clean integer or
a clean boolean. The lexical gates match
[Typed values](#typed-values). Any other text is not a candidate, and the
document keeps its first-pass parse error.

```yaml route-sweep-skip
rest:
  - host: 0.0.0.0
    port: ${env:ORDERS_PORT:-8080}
    path: /orders
```

A `rest:` port is a number. The placeholder resolves to the string `8080`,
the strict parse fails, and the probe coerces the leaf and retries. The route
loads with the typed port. Probing runs only after a failed first parse, so a
document that parses today never probes and keeps its values.

The probe tries candidate subsets smallest-first, in document order, and
parses a fresh tree for each attempt. The first success wins, so coercion
never extends beyond a minimum-cardinality subset. The cap is eight candidate
leaves for the whole document, shared by every probed subtree.

Each `throttle` step can contribute two of those leaves: its two integer
fields, `max_requests` and `period_secs`. A document that puts
`${env:}` tokens on both fields of five or more `throttle` steps carries
ten or more candidates against the budget of eight and keeps its
first-pass parse error. Split the `throttle` steps across documents, or
reduce the tokenized fields until the document carries at most eight
candidates in total. Other tokenized integer or boolean leaves share the same
budget. The
[camel-dsl crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
records the probe mechanics.

Failures keep their usual shape: an unresolved variable without a default
fails and names the variable; a non-clean value, an over-cap document, and a
genuinely broken document all keep the first-pass error text.

### Stay-out subtrees

Two subtrees never contribute candidates:

- **`templated_routes.parameters`** stays string. These values are
  template-substitution inputs, and the map is `String` by contract. A
  numeric-looking parameter value loads as text and consumes no cap budget.
- **Template bodies (`templates[].routes[]`)** are untyped at document parse.
  Typing happens at materialization, which the probe never observes. A direct
  `${env:}` token in a template body keeps its materialization limitation: it
  substitutes to a string and does not coerce there.

For a numeric template input, use the typed-parameter machinery: declare a
`number` or `boolean` parameter. The materializer then substitutes the whole
node as a typed value.

### JSON route documents

A JSON route document repairs through the same probe. The legacy text splice
stays the first pass, so a document that loads today keeps today's values,
error text, and warnings. The probe runs only when that first pass fails.

Two JSON conditions disable the repair. A document with a duplicate object key
anywhere never repairs, because key-sorted maps cannot preserve the authored
overwrite order. A document whose splice substitution breaks JSON syntax, or
whose splice tree and walk tree differ, also never repairs. Both keep the
first-pass error.

### Collision and cap boundaries

An insertion collision drops the overwritten provenance path and its
descendants. A substituted
key that a later mapping key overwrites is not a candidate, even when the
values are equal: a surviving literal must never coerce on a token's
provenance. The document keeps the first-pass error when that lost path was
its only repair.

A document over the eight-candidate cap keeps the first-pass error. This
holds when candidates under `rest:` or `mcp:` push an otherwise repairable
`routes:` document past the cap.

### `camel openapi generate`

The REST extractor shares the probe. A `rest:` port placeholder therefore
coerces under `camel openapi generate` too, and the generated document carries
the coerced value in its server URL. Extraction keeps the default-only
lookup: ambient environment is never read. An unresolved no-default
placeholder still fails and names the variable.

## Escapes

| Input | Output |
|-------|--------|
| `${env:VAR}` | Value of `VAR`; fails closed if unset and no default |
| `${env:VAR:-default}` | Value of `VAR`, or `default` when unset |
| `$$` | A single `$` |
| `$${env:VAR}` | The literal text `${env:VAR}` |

The standalone `$$` escape works on every surface: route files and all
`Camel.toml` leaves. The full-form escape `$${env:VAR}` yields the
literal placeholder text on route files and plain `Camel.toml` leaves.

**Exception — credential leaves:** `Camel.toml` sections that hold
credentials or connection secrets reject the escaped full form. On
`security`, `datasources`, `idempotent_repo`, and `cache_repo` leaves, a
`$${env:VAR}` leaves a residual `${env:VAR}` marker, which fails load.
There is no legitimate reason for a credential field to hold the literal
text of a placeholder.

## Route-value interaction

Route-source substitution handles `${env:NAME}` only. It does not substitute
`${arg:NAME}`. An unescaped `${arg:NAME}` in a route source fails to load with
an unresolved name error. Job arguments are substituted only on the `camel-cli`
job path (`crates/camel-cli/src/commands/job/document.rs`).

Substitution runs before route parsing and is trusted (ADR-0032). A substituted
`${...}` token therefore becomes a Simple template slot. In a bare top-level
string value, the route loader treats every `${...}` as a slot through one
value gate. See [Basic steps](../yaml-dsl/step-verbs/basic.md). The escape
`$${env:VAR}` yields the literal text `${env:VAR}`, which the gate then reads
as a `lang:` slot. The route compiles, but the step fails at runtime with
`Language 'env' not found`.

`$$` is not a Simple-template escape. At the Simple source level after
substitution, `$${header.h}` is a literal `$` followed by the live
`${header.h}` slot. In a route file the environment pass first turns `$$` into
`$`, so `$${header.h}` reaches Simple as the whole slot `${header.h}` and keeps
its native value. Write `$$$${header.h}` in a route file to render `$` plus a
slot.

### Literal `${...}` workaround

Preserving an authored or substituted literal `${...}` is deferred to rc-luew6.
Until then, emit the literal through an explicit `language:` and `source:`
value with a quoted rhai or js string. Escape environment and argument tokens
as `$$`, because interpolation walks every string in the route. A non-env token
needs no escape.

```yaml
# '"$${env:X}"' -> literal ${env:X}
- set_body:
    language: rhai
    source: '"$${env:X}"'

# '"${header.h}"' -> literal ${header.h} (no env/arg token, no escape)
- set_body:
    language: rhai
    source: '"${header.h}"'
```

## Fail-closed

Both surfaces fail closed. An unset variable without a default aborts:

- Route discovery fails with an error naming the variable.
- `Camel.toml` load aborts with an error naming the field.

Use `${env:VAR:-default}` for optional values.

## Legacy `{{...}}` syntax

`Camel.toml` rejects the legacy `{{...}}` placeholder syntax. Any `{{` in
a string leaf fails load with an actionable message: placeholders use
`${env:NAME}` or `${env:NAME:-default}`. Route files never supported the
`{{...}}` form.

## How it works

The DSL loader (`camel_dsl::interpolate_yaml_source`) walks the parsed
route tree, with raw-text substitution as the fallback for unparseable
documents. `camel-config` walks the merged `Camel.toml` tree
after the builder merges the main file, include files, and `CAMEL_*`
environment overrides (`resolve_tree_placeholders`). The walk replaces
`${env:...}` patterns before typed deserialization. Substituted values
pass through `sanitize_env_value`, which strips control characters and
newlines. This blocks newline injection from a hostile or malformed
variable.

The `PropertiesResolver` type in `camel-config` retains the legacy
`{{...}}` API for compatibility. `Camel.toml` loading does not use it.

## Setup

```rust,ignore
{{#include ../../../examples/env-interpolation/src/main.rs:env-interpolation}}
```

The `Camel.toml` for this example is minimal. Route discovery and
component registration follow the standard pattern.

```toml
{{#include ../../../examples/env-interpolation/Camel.toml:env-config}}
```

## When to use

- **Twelve-factor apps**: inject configuration that varies per deploy
  through the environment, not through files in source control.
- **Secrets**: pass credentials and tokens from the environment. The
  route file never stores the secret value.
- **Per-environment endpoints**: point routes at different brokers, HTTP
  hosts, or databases without editing route files.

**Reference**: `PropertiesResolver` in the [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
