# Design: restpath

## Approach

One grammar, expressed once per side over a richer segment representation.
A template segment parses into a **segment pattern**: an ordered list of
parts, `Lit(String)` and `Param(String)`, e.g. `{id}.xml` →
`[Param("id"), Lit(".xml")]`. Bare `{name}` is the single-part pattern
`[Param("name")]`, so existing templates keep their meaning.

**Grammar** (identical in both implementations): within one `/`-separated
segment, `{name}` is recognised anywhere — literal prefix, infix, suffix —
and any number of times per segment. `name` is one or more characters, none
of which is `{` or `}` (maximal non-brace run, so every name the old parser
accepted — `user-id`, `x.y` — stays valid). Any raw `{` or `}` not consumed
by a well-formed placeholder (stray brace, unclosed `{id`, empty `{}`) is a
**load error** naming the path — fail closed, never a literal (this
degradation is the defect). The name rule keeps every name the old parser
accepted valid (`{user-id}`, `{x.y}`); mixed segments that previously
degraded to literals change meaning to parameter capture by design.

**Capture rule** (runtime matcher): placeholders capture left to right;
each captures the shortest substring that still lets the remainder of the
segment match (recursive leftmost-shortest). Trailing literals are not part
of the value (`/42.xml` on `/{id}.xml` → `id=42`; `/1-2-3` on `/{a}-{b}` →
`a=1`, `b=2-3`; `/123` on `/{a}{b}` → `a=""`, `b="123"`). Empty captures
are permitted. Braces in request *values* are data. Parsing stays at
registration time; per-request matching is a bounded dynamic program over
part index × char-boundary offsets (O(parts × segment_len²) worst case,
never exponential backtracking — request paths are untrusted input).

**Specificity** is executable from the contract and legacy-reducing: a
template's specificity is the pair `(L, C)` — `L` = number of segments
containing at least one literal part (bare `{name}` = 0; mixed or
literal-only = 1), `C` = total literal characters across all segments —
compared lexicographically. On bare/literal templates `L` is exactly the
legacy literal-segment count, so legacy winners never reverse; `C` only
splits ties the legacy rule could not decide. At match time the highest
pair wins among matching candidates; an exact tie is `Ambiguous`
(defensive — load-time rejection should prevent it within one method).

**Ambiguity (load)**: two operations on the same listener (host, port) and
the same HTTP method — across all REST blocks bound to that listener, per
the existing cross-block rules in `lower_all_rest_to_routes` — are
ambiguous iff their templates have equal `(L, C)` pairs AND at least one
request path matches both. Whole-path overlap decomposes into per-segment
language overlap at every position (segments partition on `/`). A segment
pattern's language is literal strings joined by placeholder holes matching
any substring including empty; two patterns overlap iff their regular
languages intersect — decide exactly via product-automaton reachability
(literal chars as labeled edges, placeholder as any-char self-loop). This
subsumes positional compatibility (`/ab{x}` vs `/a{y}b` overlap at `/abb`
despite different part counts) and avoids false positives (`/{p}y` vs
`/{q}xy` overlap at `/1xy` but differ in `C`, so the longer literal wins).

**Anti-drift** (decision 4): a cross-crate corpus test lives in
`camel-dsl`'s test targets (camel-dsl already dev-depends on
`camel-component-http`). It feeds one corpus of templates to both parsers
and asserts identical part-by-part classification and identical rejection.
`camel-component-http`'s `rest_match` parsing API (types +
`parse_path_template`) becomes publicly exported for this purpose.

## Affected crates

- `camel-component-http` (`rest_match.rs`): fallible segment-pattern parse,
  part-wise `try_match` with recursive leftmost-shortest multi-placeholder
  capture, `(L, C)` specificity, exact-overlap tie-breaking.
- `camel-dsl` (`rest.rs`): fallible `parse_path_template` with the same
  grammar; `validate_path_template` rejects malformed braces/names naming
  the path and operation; `extract_param_names` walks parts (multi-param
  segments); `template_specificity` becomes the `(L, C)` rule;
  `templates_ambiguous` uses overlap + equal pairs, scoped to same
  listener and method across blocks.
- Cross-crate agreement test (camel-dsl dev-tests).
- `openspec/specs/rest-dsl` delta + tests on both sides.
- `docs/` only if a guide documents path templates (check; if so, one
  grammar sentence + `mdbook test`).

## Architecture boundaries

Control-plane (DSL) validates and lowers; data-plane (component) matches and
captures. The grammar is duplicated in exactly two places, pinned by the
corpus test (ADR-0011/0016 route-model stability: no `RouteDefinition`
shape change). Fail-closed load errors follow the fail-closed validation
posture (ADR-0033); request paths are untrusted exchange data (ADR-0032):
capture is bounded by segment length, allocates only the matched substring,
and braces in request values are plain data.

## Alternatives considered

- **Share one parser crate**: rejected — no shared grammar crate exists
  today; a corpus test (decision 4) is lighter than a new dependency.
- **Regex per segment**: rejected — allocation and compile cost at load plus
  hot-path regex matching; a hand-rolled bounded dynamic program keeps the
  lazy-capture rule explicit and is polynomial in segment length.
- **Greedy capture**: rejected — decision 2 mandates shortest-match; greedy
  would make `/{a}-{b}` on `/1-2-3` yield `a=1-2`.
- **Positional part compatibility for ambiguity**: rejected — it is not
  language overlap (`/ab{x}` vs `/a{y}b` overlap at `/abb` despite
  different part counts); product-automaton intersection is exact and small.
- **Per-segment lexicographic score vectors**: rejected — they reverse
  legacy multi-segment precedence (`/{u}/b/c` must still beat
  `/a/{x}/{y}` for `/a/b/c`); the summed `(L, C)` pair reduces to the
  legacy literal-segment count on legacy templates.
