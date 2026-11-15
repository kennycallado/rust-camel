# Tasks: restpath

Single-phase change: one grammar, two pinned implementations, spec delta.
NEW-symbol contract (identical names in both crates; the cross-crate test
maps both into a test-local enum):

- `SegmentPart { Lit(String), Param(String) }` (derive Debug, Clone,
  PartialEq, Eq)
- `SegmentPattern { pub parts: Vec<SegmentPart> }` (same derives; field is
  public so the corpus test adapters can map it)
- `TemplateParseError { UnclosedPlaceholder { segment: String },
  StrayCloseBrace { segment: String }, EmptyPlaceholder { segment: String } }`
  (derive Debug, Clone, PartialEq, Eq; `#[must_use]`; `impl Display` whose
  text explains the malformation — used in `format!("{err}")` at the
  wrapping call sites)
- `parse_path_template(template: &str) -> Result<Vec<SegmentPattern>, TemplateParseError>`
- `template_specificity(segments: &[SegmentPattern]) -> (usize, usize)` —
  `(L, C)`: `L` = segments containing at least one `Lit` part, `C` = total
  `Lit` characters
- `segment_patterns_overlap(a: &SegmentPattern, b: &SegmentPattern) -> bool`
  (DSL side only)
- `templates_ambiguous(a: &[SegmentPattern], b: &[SegmentPattern]) -> bool`
  (DSL side only)
- `extract_param_names(segments: &[SegmentPattern]) -> Vec<String>`

## camel-component-http

### Task 1.1: segment-pattern grammar and fallible parse in rest_match.rs

**Files:**
- `crates/components/camel-http/src/rest_match.rs` (modified)
- `crates/components/camel-http/src/registry.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/lib_tests.rs` (modified — its
  `PathSegment` uses move to `SegmentPattern`)
- `crates/components/camel-http/benches/registry.rs` (modified — its
  `PathSegment` uses move to `SegmentPattern`)

**Steps:**
1. In `rest_match.rs`, replace `PathSegment` and the infallible
   `parse_path_template` with the NEW-symbol contract types above. Parsing
   splits the template on `/` (dropping empty segments, leading `/`), then
   parses each segment left to right: a `{` starts a placeholder whose name
   is the maximal run of characters excluding `{` and `}` and which MUST be
   terminated by `}`; a `}` outside a placeholder is an error; an empty name
   is `EmptyPlaceholder`. Literal text between placeholders becomes `Lit`
   parts; a segment with no placeholders is one `Lit` part. Every parse
   failure carries the offending segment text in its `segment` field.
2. Update `registry.rs`: line 124 `pub use` re-exports `SegmentPattern` and
   `SegmentPart` instead of `PathSegment`; the registration API (around
   line 196) takes `Vec<SegmentPattern>` and its template reconstruction
   (around line 213) renders `Lit(s)` as `s` and `Param(n)` as `{n}`;
   in `unregister_rest_endpoint` (line ~239, which returns `()`), handle
   the fallible parse with `let Ok(target_segments) =
   crate::rest_match::parse_path_template(path_template) else { return; };`
   — a malformed template can never have been registered (registration
   parses and rejects), so unregistration of one is a no-op.
3. Update `lib.rs` line ~2076: `parse_path_template(&self.config.path)`
   handles `Result`, surfacing the error through the existing
   config/consumer startup error path with the path template text in the
   message. In the same file change `pub(crate) mod rest_match;` to
   `pub mod rest_match;` — the parsing API becomes public per design.md
   anti-drift (the cross-crate corpus test in Task 3.1 imports
   `camel_component_http::rest_match::…`).
4. Delete every remaining `PathSegment` reference; run
   `cargo clippy -p camel-component-http --all-targets -- -D warnings`.

**Tests:**
- `parse_bare_placeholder_segment`: parse `/users/{id}` → `Ok`, one
  segment `[Param("id")]` → assert exact `SegmentPattern` equality →
  `cargo test -p camel-component-http --lib` → expected: fail before step 1
  (type/shape change), pass after
- `parse_mixed_segment_parts`: parse `/files/{id}.xml` → `[Param("id"),
  Lit(".xml")]`; parse `/report-{date}.json` → `[Lit("report-"),
  Param("date"), Lit(".json")]`; parse `/a{y}bc` → `[Lit("a"), Param("y"),
  Lit("bc")]` → assert exact structures → same command → expected: fail
  before, pass after
- `parse_multi_placeholder_segment`: parse `/{a}-{b}` → `[Param("a"),
  Lit("-"), Param("b")]`; parse `/{a}{b}` → `[Param("a"), Param("b")]` →
  assert → same command → expected: fail before, pass after
- `parse_legacy_names_stay_valid`: parse `/users/{user-id}` → `[Param
  ("user-id")]`; parse `/f/{x.y}.dat` → `[Param("x.y"), Lit(".dat")]` →
  assert → same command → expected: fail before, pass after
- `parse_placeholder_like_literal_text_ok`: parse `/files/%7Bid%7D` →
  `Ok` with two literal segments: `[Lit("files")]` and `[Lit("%7Bid%7D")]`
  (the template splits on `/`), no `Param` parts → assert → same command →
  expected: pass before and after (guards literal behavior)
- `parse_rejects_unclosed_placeholder`: parse `/files/{id.xml` →
  `Err(TemplateParseError::UnclosedPlaceholder { segment: "{id.xml" })` →
  assert → same command → expected: fail before (returned a literal), pass
  after
- `parse_rejects_stray_close_brace`: parse `/files/id}.xml` →
  `Err(StrayCloseBrace { segment: "id}.xml" })` → assert → same command →
  expected: fail before, pass after
- `parse_rejects_empty_placeholder_name`: parse `/files/{}.xml` →
  `Err(EmptyPlaceholder { segment: "{}.xml" })` → assert → same command →
  expected: fail before (became a literal), pass after
- `parse_rejects_nested_braces`: parse `/{{id}}` → `Err` (Unclosed or
  Stray kind — assert `matches!` on either `UnclosedPlaceholder` or
  `StrayCloseBrace`) → same command → expected: fail before, pass after

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0
- `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0
- `cargo fmt --check --all` exits 0
- No `PathSegment` identifier remains in the crate
  (`grep -rn PathSegment crates/components/camel-http/src` is empty)

- [x] 1.1

### Task 1.2: part-wise matching, leftmost-shortest capture, (L, C) specificity

**Files:**
- `crates/components/camel-http/src/rest_match.rs` (modified)

**Steps:**
1. Implement segment matching of one request segment against one
   `SegmentPattern` with bounded-state dynamic programming, never
   exponential backtracking: `reachable[i][j]` = "the first `i` parts match
   the request segment prefix up to byte offset `j`" for
   `i in 0..=parts.len()`, `j` over char-boundary offsets. `Lit(s)`
   advances deterministically (only if the next bytes equal `s`);
   `Param(_)` transitions to every char-boundary `j' >= j` (empty capture
   = `j' == j` allowed). A segment matches iff `reachable[parts.len()][len]`
   is set; worst case O(parts × len²), which is polynomial and
   request-input-safe. Recover captures with a backward `can_finish[i][j]`
   table ("parts `i..` can match the request suffix from `j`", computed
   the same way in reverse): walking `i` forward, each `Param` takes the
   SMALLEST `j'` with `reachable`-compatible prefix AND
   `can_finish[i+1][j']` set (leftmost-shortest per decision 2). All
   offsets land on `char` boundaries (`char_indices`) so multi-byte UTF-8
   in literals and captures slices safely. Braces in request text are
   plain chars — the request is never parsed.
2. Implement `template_specificity(segments: &[SegmentPattern]) ->
   (usize, usize)` per the NEW-symbol contract.
3. In `match_endpoint`, replace the literal-count specificity comparison
   with lexicographic `(L, C)` comparison (higher pair wins). Preserve the
   existing tie behavior: two candidates with the SAME `(L, C)` both
   matching set `ambiguous` (existing `MatchOutcome::Ambiguous` path).
4. Run `cargo clippy -p camel-component-http --all-targets -- -D warnings`.

**Tests:**
- `mixed_segment_matches_extension_request` (REGRESSION — reproduces the
  silent 404): endpoint `GET /files/{id}.xml` → match `GET /files/42.xml`
  → `Found`, `path_params["id"] == "42"` →
  `cargo test -p camel-component-http --lib` → expected: RED before
  (returns `NotFound`, the defect that surfaces as HTTP 404), GREEN after
- `trailing_literal_not_captured`: endpoint `GET /report-{date}.json` →
  match `GET /report-2026-11-15.json` → `date == "2026-11-15"` (NOT
  `"2026-11-15.json"`) → same command → expected: red before, green after
- `two_placeholders_leftmost_shortest`: endpoint `GET /{a}-{b}` → match
  `GET /1-2-3` → `a == "1"`, `b == "2-3"` → same command → expected: red
  before, green after
- `three_placeholders_split`: endpoint `GET /{a}-{b}-{c}` → match
  `GET /1-2-3-4` → `a == "1"`, `b == "2"`, `c == "3-4"` → same command →
  expected: red before, green after
- `adjacent_placeholders_first_takes_empty`: endpoint `GET /{a}{b}` →
  match `GET /123` → `a == ""`, `b == "123"` → same command → expected:
  red before, green after
- `empty_mixed_capture_permitted`: endpoint `GET /a{y}bc` → match
  `GET /abc` → `y == ""` → same command → expected: red before, green after
- `braces_in_request_value_are_data`: endpoint `GET /files/{id}.xml` →
  match `GET /files/{oops}.xml` → `id == "{oops}"` → same command →
  expected: red before, green after
- `literal_placeholder_like_text_matches_literally`: endpoint
  `GET /files/%7Bid%7D` → match `GET /files/%7Bid%7D` → `Found`,
  `path_params` empty → same command → expected: pass before and after
- `literal_beats_mixed_specificity`: endpoints `GET /fixed.xml` and
  `GET /{id}.xml` → match `GET /fixed.xml` → `Found` with payload
  `/fixed.xml`, `path_params` empty → same command → expected: green
  before and after (the old parser also routes `/fixed.xml` correctly —
  guards no regression once the mixed template becomes real)
- `longer_mixed_literal_wins`: endpoints `GET /{p}y` and `GET /{q}xy` →
  match `GET /1xy` → payload `/{q}xy`, `q == "1"` → same command →
  expected: red before, green after
- `legacy_multi_segment_precedence_preserved`: endpoints
  `GET /a/{x}/{y}` and `GET /{u}/b/c` → match `GET /a/b/c` → payload
  `/{u}/b/c`, `u == "a"` → same command → expected: green before and
  after (guards legacy precedence: `(L, C)` must reduce to the legacy
  literal-segment count)
- `specificity_pair_shape`: `template_specificity` of `/files/{id}.xml` is
  `(2, 9)` (segments `files` and `{id}.xml` both literal-bearing; literal
  chars `files` + `.xml`), `/fixed.xml` is `(1, 9)`, `/{a}` is `(0, 0)`,
  `/x/{id}` is `(1, 1)` → assert exact tuples → same command → expected:
  fail before (signature change), pass after
- `adversarial_many_placeholders_no_match_bounded`: endpoint
  `GET /{a}{b}{c}{d}{e}{f}{g}{h}z` → match request path
  `format!("/{}", "y".repeat(64))` (64 y, no trailing `z`) → `NotFound`,
  and the whole test body completes in under 1 second
  (`std::time::Instant` assertion — naive partition enumeration over 8
  holes would blow past it; the bounded DP is microseconds) → same command
  → expected: green before and after for the `NotFound` verdict (the old
  parser made this segment a literal too); the elapsed-time assertion is
  the bounded-execution regression guard
- `adversarial_many_placeholders_match_leftmost_shortest`: same endpoint
  → match request path `format!("/{}z", "y".repeat(64))` (64 y then `z`)
  → `Found`, and the captures satisfy leftmost-shortest recovery: `a`
  through `g` are all `""` and `h` equals the 64-char y-run → same
  command → expected: fail before (old parser literal), pass after
- `utf8_capture_boundaries`: endpoint `GET /{a}.txt` → match
  `GET /café.txt` → `a == "café"` (multi-byte capture sliced on char
  boundaries) → same command → expected: red before, green after

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 (including the
  regression test, red-then-green)
- `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0
- `cargo fmt --check --all` exits 0

- [x] 1.2

### Task 1.3: CamelHttpPath_<param> header translation for mixed captures

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Extract the per-request translation at lib.rs ~2160-2165 (the loop over
   `envelope.path_params` setting `CamelHttpPath_<param>` headers) into a
   pure helper `pub(crate) fn path_param_headers(path_params:
   &std::collections::HashMap<String, String>) -> Vec<(String, String)>`
   returning `(header_name, value)` pairs where the header name is
   `CamelHttpPath_{name}`. The consumer loop consumes the helper's output
   unchanged (behavior-preserving refactor).
2. Run `cargo clippy -p camel-component-http --all-targets -- -D warnings`.

**Tests:**
- `camel_http_path_headers_from_mixed_segment`: run `match_endpoint` for
  `GET /files/{id}.xml` vs request `GET /files/42.xml` (Task 1.2 code),
  pass its `path_params` to `path_param_headers` → assert the result
  contains `("CamelHttpPath_id", "42")` →
  `cargo test -p camel-component-http --lib` → expected: fail before
  (helper does not exist), pass after
- `camel_http_path_headers_multi_param_segment`: `match_endpoint` for
  `GET /{a}-{b}` vs `GET /1-2-3`, then `path_param_headers` → assert the
  sorted result equals `[("CamelHttpPath_a", "1"), ("CamelHttpPath_b",
  "2-3")]` → same command → expected: fail before, pass after
- `camel_http_path_headers_braces_are_values`: `match_endpoint` for
  `GET /files/{id}.xml` vs `GET /files/{oops}.xml`, then
  `path_param_headers` → assert `("CamelHttpPath_id", "{oops}")` → same
  command → expected: fail before, pass after

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0
- `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0
- The consumer loop at lib.rs ~2160 no longer formats header names inline
  (it calls `path_param_headers`)

- [x] 1.3

## camel-dsl

### Task 2.1: same grammar in the DSL parser, fail-closed validation, extraction

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)
- `crates/camel-dsl/src/openapi.rs` (modified)

**Steps:**
1. In `rest.rs`, replace `PathSegment` and the infallible
   `parse_path_template` with the NEW-symbol contract types and grammar
   (byte-for-byte the same rules as Task 1.1 step 1; duplicate the
   implementation — decision 4 pins agreement by the corpus test).
2. Update `validate_path_template` to wrap `TemplateParseError` into
   `CamelError::RouteError` naming the operation and the full path:
   `rest operation '{op_label}': path '{full_path}': {reason}`. Preserve
   the existing empty-name message text ("parameter names are required")
   via the `EmptyPlaceholder` arm. Keep the duplicate-parameter-names
   check, now walking parts via `extract_param_names` (a name appearing
   twice anywhere in one path, including twice in one segment, is a
   duplicate).
3. Update `extract_param_names` to walk `parts` and collect every
   `Param(name)` in order (multi-placeholder segments contribute several
   names).
4. Update `openapi.rs` call sites (lines ~108-109, in `build_operation`
   which returns `Value`, called from `generate_openapi` returning
   `OpenApiGenerationResult` — NOT a `Result`): on `TemplateParseError`,
   push `format!("path '{full_path}': {err}")` into the function's
   `warnings: &mut Vec<String>` parameter and continue with empty
   `param_names`. No signature change: callers
   (`camel-cli/src/commands/openapi.rs` and the openapi integration tests)
   stay untouched. No `unwrap` anywhere.
5. Run `cargo clippy -p camel-dsl --all-targets -- -D warnings`.

**Tests:**
- `validate_rejects_unclosed_placeholder_naming_path`: operation
  `GET /files/{id.xml` → `validate_path_template` → `Err(RouteError)`
  whose message contains `/files/{id.xml` →
  `cargo test -p camel-dsl --lib` → expected: red before (loads as
  literal), green after
- `validate_rejects_stray_close_brace_naming_path`: path
  `/files/id}.xml` → `Err(RouteError)` containing `/files/id}.xml` → same
  command → expected: red before, green after
- `validate_rejects_empty_placeholder_name`: path `/files/{}.xml` →
  `Err(RouteError)` containing `/files/{}.xml` AND `parameter names are
  required` → same command → expected: red before (the mixed segment
  `{}.xml` degraded to a literal, so the existing whole-segment `{}`
  check never fired), green after
- `validate_rejects_duplicate_names_across_mixed_segments`: path
  `/{id}.xml/{id}` → `Err(RouteError)` containing `more than once` → same
  command → expected: red before (mixed segment was a literal, no dup
  detected), green after
- `validate_accepts_legacy_names`: path `/users/{user-id}` → `Ok(())` →
  same command → expected: green before and after
- `extract_multi_param_segment_names`: parse `/{a}-{b}.json` →
  `extract_param_names` returns `["a", "b"]` in order → same command →
  expected: red before (returned `[]`), green after
- `lowering_rejects_malformed_templates_naming_path`: `lower_all_rest_to_routes`
  over blocks whose op paths are `/files/{id.xml`, `/files/id}.xml`, and
  `/files/{}.xml` (one document per case) → each returns `Err(RouteError)`
  whose message contains the offending full path → same command →
  expected: red before (all three loaded as literals), green after

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- No `PathSegment` identifier remains in `crates/camel-dsl/src`
  (`grep -rn PathSegment crates/camel-dsl/src` is empty)

- [x] 2.1

### Task 2.2: (L, C) specificity and overlap-based ambiguity in the DSL

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)

**Steps:**
1. Implement `template_specificity` per the NEW-symbol contract (returns
   `(usize, usize)`).
2. Implement `segment_patterns_overlap(a: &SegmentPattern, b:
   &SegmentPattern) -> bool`: each pattern is a concatenation of literal
   strings with placeholder holes matching any substring including empty;
   decide language intersection exactly via product-automaton
   reachability — states are `(index_a, index_b, offset_a, offset_b)` over
   part-and-literal positions; literal chars are labeled edges, a
   placeholder edge consumes any char of the other side's current literal
   or its own hole; a state where both patterns are fully consumed is
   reachable-satisfying. (Any exact-decision implementation is acceptable
   if the tests below pass, including `/ab{x}` vs `/a{y}b` → true and
   `/{p}y` vs `/{q}xy` → true with different specificity.)
3. Rewrite `templates_ambiguous(a, b)` as: `a.len() == b.len()` AND
   `template_specificity(a) == template_specificity(b)` AND
   `segment_patterns_overlap` holds for EVERY segment position pair.
   Keep `lower_all_rest_to_routes`'s listener (host, port) + method
   scoping unchanged.
4. In `lower_all_rest_to_routes` (rest.rs ~48) handle the now-fallible
   `parse_path_template(&full_path)` with `map_err` into
   `CamelError::RouteError(format!("rest operation '{op_label}': path
   '{full_path}': {err}"))` and `?` (the function returns
   `Result<Vec<RouteDslRoute>, CamelError>`).
5. Update the ambiguity error in `lower_all_rest_to_routes` to name BOTH
   operations: extend the `registered` tuples from `(host, port, method,
   segments)` to also carry the first operation's `op_label` and
   `full_path`, and format the error as
   `rest operation '{op_label}': path '{full_path}' is ambiguous with
   operation '{other_label}' path '{other_path}' of equal specificity for
   method {verb}`. Update any existing test asserting the old message
   text.
6. Run `cargo clippy -p camel-dsl --all-targets -- -D warnings`.

**Tests:**
- `ambiguous_equal_specificity_mixed_pair`: `templates_ambiguous`
  (`/{id}.xml`, `/{name}.xml`) → `true` → `cargo test -p camel-dsl --lib`
  → expected: fail before, pass after
- `ambiguous_despite_part_order`: `/ab{x}` vs `/a{y}b` → `true` (both
  match `abb`) → same command → expected: fail before (positional rule
  misses it), pass after
- `ambiguous_prefix_suffix_forms`: `/a{x}` vs `/{y}a` → `true` (both
  match `aa`) → same command → expected: fail before, pass after
- `ambiguous_mirrored_segment_literals`: `/x/{id}` vs `/{a}/y` → `true`
  (both match `x/y`) → same command → expected: green before and after
  (legacy also rejected this pair — precedence guard)
- `not_ambiguous_different_specificity`: `/{p}y` vs `/{q}xy` → `false`
  (specificity `(1,1)` vs `(1,2)`) → same command → expected: green
  before and after (before the change both were unequal literals, also
  non-ambiguous — guards against over-ambiguity)
- `not_ambiguous_disjoint_literals`: `/a{x}` vs `/b{y}` → `false` (no
  common request) → same command → expected: green before and after
- `lowering_rejects_cross_block_same_listener_ambiguity`: two REST blocks
  with the same host and port, `GET /{id}.xml` (operation_id `get_xml`) and
  `GET /{name}.xml` (operation_id `get_xml2`) →
  `lower_all_rest_to_routes` → `Err(RouteError)` whose message contains
  both `/{id}.xml` AND `/{name}.xml` AND both operation labels `get_xml`
  and `get_xml2` → same command → expected: fail before, pass after
- `lowering_error_names_both_operations_same_block`: one block with
  `GET /ab{x}` and `GET /a{y}b` (distinct operation_ids) →
  `lower_all_rest_to_routes` → `Err(RouteError)` naming both paths and
  both labels → same command → expected: fail before, pass after
- `lowering_rejects_prefix_suffix_ambiguity`: one block with `GET /a{x}`
  and `GET /{y}a` (distinct operation_ids) → `lower_all_rest_to_routes`
  → `Err(RouteError)` naming both paths and both labels (both match
  `GET /aa`) → same command → expected: fail before (loaded as unequal
  literals), pass after
- `lowering_rejects_mirrored_segments_ambiguity`: one block with
  `GET /x/{id}` and `GET /{a}/y` (distinct operation_ids) →
  `lower_all_rest_to_routes` → `Err(RouteError)` naming both paths and
  both labels (both match `GET /x/y`) → same command → expected: red
  before, green after (legacy also REJECTED this pair, but its message
  named only the current operation — the both-operations diagnostic is
  new; guards preserved strictness)
- `lowering_accepts_mixed_templates`: one block with `GET /files/{id}.xml`
  and `POST /report-{date}.json` → `lower_all_rest_to_routes` → `Ok`,
  two routes → same command → expected: green before and after (loading
  always succeeded — the defect is at match time; this guards the
  fail-closed work does not over-reject valid mixed templates)
- `lowering_allows_different_listener_and_method`: same templates on
  different host:port → `Ok`; same listener but `GET` vs `POST` → `Ok` →
  same command → expected: green before and after (existing scoping
  preserved)
- `lowering_accepts_resolvable_overlap_pairs`: operations
  `GET /a/{x}/{y}` and `GET /{u}/b/c` on one listener → `Ok` (specificity
  `(1,1)` vs `(2,2)` differs) → same command → expected: green before and
  after (legacy also loaded this pair — its specificity counts already
  differed; guards no over-ambiguity)

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0

- [x] 2.2

## cross-crate agreement

### Task 3.1: corpus test pinning both parsers together

**Files:**
- `crates/camel-dsl/tests/rest_template_corpus.rs` (new)

**Steps:**
1. Create the integration test with a test-local `Part` enum
   (`Lit(String)`, `Param(String)`) and a `classify` adapter per crate that
   maps `camel_component_http::rest_match::{SegmentPattern, SegmentPart,
   TemplateParseError}` (publicly exported by Task 1.1 step 3) and
   `camel_dsl::rest::{SegmentPattern, SegmentPart, TemplateParseError}`
   into `Vec<Vec<Part>>` / a test-local error-kind enum (`Unclosed`,
   `StrayClose`, `EmptyName`).
2. Define one corpus constant: VALID = [`/users/{id}`, `/files/{id}.xml`,
   `/report-{date}.json`, `/{a}-{b}`, `/{a}{b}`, `/a{y}bc`,
   `/users/{user-id}`, `/f/{x.y}.dat`, `/files/%7Bid%7D`, `/x/{id}`,
   `/a/b/c`]; INVALID = [`/files/{id.xml`, `/files/id}.xml`,
   `/files/{}.xml`, `/{{id}}`, `/x/{a{b}`].
3. One test fn per corpus class asserting: both parsers classify each
   VALID template into identical `Part` structures AND the structure
   matches a hardcoded expectation for at least the mixed entries; both
   parsers reject every INVALID template with the same error kind.

**Tests:**
- `both_parsers_classify_valid_corpus_identically`: for each VALID
  template → both `parse_path_template` return `Ok`, mapped structures
  equal each other and the hardcoded expectations →
  `cargo test -p camel-dsl --test rest_template_corpus` → expected: fail
  before (DSL still on old parser until Tasks 1.1/2.1 land), pass after
- `both_parsers_reject_invalid_corpus_identically`: for each INVALID
  template → both return `Err`, mapped error kinds equal → same command →
  expected: fail before, pass after

**Acceptance:**
- `cargo test -p camel-dsl --test rest_template_corpus` exits 0
- The two assertions cover every corpus entry (count assertions in the
  test: VALID length 11, INVALID length 5)

- [x] 3.1

## docs and gates

### Task 4.1: docs grammar sentence (only if a guide documents path templates)

**Files:**
- (none, unless the search in step 1 has hits — then the specific guide
  file that matched)

**Steps:**
1. Run `grep -rln 'path template\|{id}\|{name}' docs/src 2>/dev/null` and
   `grep -rln 'rest.*path\|path.*placeholder' docs/src 2>/dev/null` over
   the mdbook sources. If NO file matches path-template documentation,
   record "no docs reference REST path templates" in the task result and
   stop — no file is modified.
2. If a guide documents path templates, edit only that file: add exactly
   one sentence stating the grammar (a `{name}` placeholder may appear
   anywhere in a `/`-separated segment with literal text around it; a
   malformed brace fails route load). Do not touch binding semantics or
   other content.
3. Only when step 2 edited a file: `cd docs && mdbook test`.

**Tests:**
- (documentation task — the verification is the command in step 3, or the
  recorded empty-search result from step 1)

**Acceptance:**
- Either "no docs reference REST path templates" is recorded with the grep
  output showing zero relevant hits, or `cd docs && mdbook test` exits 0
  with the one-sentence grammar addition

- [x] 4.1

### Task 4.2: whole-change gate sweep

**Files:**
- (none — verification only)

**Steps:**
1. From the worktree root run `cargo fmt --check --all`; record exit code.
2. Run `cargo clippy -p camel-component-http -p camel-dsl --all-targets
   -- -D warnings`; record exit code.
3. Run `cargo test -p camel-component-http --lib` and
   `cargo test -p camel-dsl --lib`; record exit codes.
4. Run `cargo test -p camel-dsl --test rest_template_corpus`; record exit
   code.
5. Fix any failure by returning to the owning task (1.1/1.2 for
   camel-component-http, 2.1/2.2 for camel-dsl, 3.1 for the corpus);
   do not weaken assertions to go green.

**Tests:**
- (verification task — each step is a recorded command with exit code)

**Acceptance:**
- All commands in steps 1-4 exit 0 and the exit codes are reported in the
  task result

- [x] 4.2
