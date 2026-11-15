# Proposal: restpath

## Why

A REST path segment that mixes a literal with a placeholder (`/{id}.xml`,
`/report-{date}.json`) never matches: the route loads and every request 404s
with plain `Not Found`. Reproduced in 0.68.0, still present on main.

Both path-template parsers accept only a segment that is *entirely* `{name}`
(`seg.strip_prefix('{').and_then(|s| s.strip_suffix('}'))`):

- `crates/components/camel-http/src/rest_match.rs` `parse_path_template`:
  `{id}.xml` degrades to `Literal("{id}.xml")`, so `try_match` demands that
  literal text from the request.
- `crates/camel-dsl/src/rest.rs`: the same parser feeds
  `validate_path_template`, `template_specificity`, and the ambiguity check,
  so the placeholder is invisible at load time, never reaches
  `extract_param_names`, and the per-route `CamelHttpPath_<param>` header is
  never set.

A typo like a stray `{` degrades to a literal and becomes a silent 404
instead of a load error.

## What Changes

- `camel-component-http` (`rest_match.rs`): real segment-pattern parsing —
  `{name}` recognised anywhere in a segment (literal prefix, infix, suffix;
  multiple placeholders per segment), recursive leftmost-shortest capture,
  `(L, C)` specificity and deterministic tie-breaking.
- `camel-dsl` (`rest.rs`): the same grammar for load-time validation,
  param-name extraction (multi-param segments), specificity and ambiguity
  detection; fail-closed rejection of stray `{`/`}` naming the path.
- One grammar, two implementations that cannot drift: a cross-crate corpus
  test feeding the same templates to both parsers, asserting identical
  segment classification and identical rejections.
- `rest-dsl` capability spec: path-template grammar (placeholder syntax,
  capture rule, specificity, ambiguity rule, fail-closed load error) with one
  scenario per rule.
- Tests on both sides (regression, capture, multi-placeholder, literal,
  malformed-rejection, ambiguity, specificity).

Excluded: REST binding semantics, `raw`/JSON binding, route DSL beyond the
path grammar, Redis work (mission 590).

## Acceptance criteria

- `GET /users/42.xml` on `/{id}.xml` matches with `id=42`;
  `/report-2026-11-15.json` on `/report-{date}.json` matches with
  `date=2026-11-15`; `/{a}-{b}` on `/1-2-3` yields `a=1`, `b=2-3`;
  `/{a}{b}` on `/123` yields `a=""`, `b="123"` (leftmost-shortest, empty
  captures permitted).
- A route whose path contains a stray `{`, an unclosed `{id`, or an empty
  `{}` fails route load with an error naming the path — never a silent 404.
- Specificity is the legacy-reducing pair `(L, C)` (literal-segment count,
  total literal characters), compared lexicographically: `/fixed.xml`
  beats `/{id}.xml`; `/{q}xy` beats `/{p}y` for `/1xy`; `/{u}/b/c` beats
  `/a/{x}/{y}` for `/a/b/c` (legacy precedence preserved).
- `/{id}.xml` vs `/{name}.xml`, `/ab{x}` vs `/a{y}b`, `/a{x}` vs `/{y}a`,
  and `/x/{id}` vs `/{a}/y` are ambiguous within one listener + method —
  including across REST blocks (load error naming both operations).
- Both parsers classify and reject an identical corpus (cross-crate test).
- `CamelHttpPath_<param>` headers carry the captured value for every
  placeholder, including mixed-segment ones.
- The regression test reproduces the 404 first (red), then passes (green).

## Risk budget

Acceptable: public export of the `camel-component-http` matcher parser for
the agreement test; new and stricter load errors (brace-malformed templates
that previously degraded to silent 404s; mixed-template pairs like
`/a{x}` vs `/{y}a` that previously loaded as distinct literal paths and now
fail ambiguity validation). Compatibility guarantee, scoped: configurations
built only from brace-free literal segments and whole-segment placeholders
keep their exact match outcomes and winners — `(L, C)` reduces to the
legacy literal-segment count there. Mixed templates (`/{id}.xml` and
friends) necessarily change from literal matching to parameter capture —
that is the purpose of this change — and may take precedence over legacy
templates by design. The name rule (`{name}` = nonempty, no braces)
accepts every name the old parser accepted (`{user-id}`, `{x.y}`). One
relaxation: legacy template pairs that previously failed load as
false-positive ambiguities may now load with a deterministic `(L, C)`
winner. Out of bounds: any change to REST binding semantics or non-path DSL
behavior; runtime match performance regressions on the hot path (parsing
stays at registration time).
