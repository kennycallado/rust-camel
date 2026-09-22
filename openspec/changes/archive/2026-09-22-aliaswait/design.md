# Design: aliaswait

## Approach

Single choke point: the `ResolvesPaths` trait. Every consumer
(`TimeoutCollector` future-arg regions, `TimeoutSeeker` loop deadlines,
`SpawnCollector` handle sources, `WaitFinder` awaited calls) funnels
through `is_path_target`, so all four inherit the fix from one change.

**1. Import data.** `type Imports = HashMap<String, String>` becomes
`struct Imports { named: HashMap<String, Vec<String>>,
items_value: HashSet<String>, items_type: HashSet<String>,
globs: Vec<String> }`. `named` is a MULTIMAP: Rust permits
same-spelling type- and value-namespace imports in one scope
(`use tokio::task::spawn; use types::spawn;` — both legal), and a
syn-based collector cannot know an import's namespace, so every
same-scope same-name import is kept as a candidate reading (union,
never overwrite). Targets and glob roots are recorded with their
ABSOLUTE marker: a `use ::tokio::…` root or an `extern crate` alias
stores a leading `"::"`-prefixed string, and expansion never rewrites
a leading absolute segment (an absolute target keeps its extern-prelude
semantics even when a local alias spells `tokio`). `Item::ExternCrate`
with a rename is collected as a module alias (`extern crate tokio as
runtime;` → `runtime → ::tokio`); a bare `extern crate tokio;` adds
nothing (already implicit). `collect_use_tree`'s `Glob` arm records
the prefix as a glob root (absolute-prefixed when the use root is);
named/rename arms unchanged apart from the multimap/absolute encoding
(module aliases already map). Per-module items are tracked
NAMESPACE-AWARE because type and value namespaces coexist
(`type spawn = (); use tokio::task::spawn; spawn();` compiles and
the call is tokio's): `items_value` = `fn`/`const`/`static` and
tuple-struct/unit-struct constructors ONLY (`syn::Fields::Unnamed`/
`Unit`) — named-field structs and enums have NO value-namespace
callable and are excluded (`struct spawn { x: u32 }` coexists with an
imported `spawn()`); `items_type` = `mod`/
`struct`/`enum`/`union`/`trait`/`type` and terminates QUALIFIED
first-segment lookups (tuple/unit structs sit in both sets). A
termination on the wrong-namespace item would suppress findings; the
split prevents that, and over-inclusion within a namespace is still
conservative.

Test-fn-body bindings are collected SCOPE-AWARE, replacing the fn-wide
shadow approximation for lookups:

- **Terminal body items** — `fn`/`const`/`static`/tuple-struct/
  unit-struct ITEM statements directly in the fn block. Rust makes
  these visible for the whole fn body regardless of position, so they
  terminate value lookups legitimately.
- **Non-terminal locals** — every other local binding (let / closure /
  for / if-let / match-arm patterns, nested-block item statements).
  These contribute a `<local>` candidate but the walk CONTINUES: a
  late or nested `let spawn = …` must not suppress detection of an
  earlier awaited import call. For BOUNDING the outcome is identical
  to the old fn-wide rule in every case (a `<local>` candidate either
  makes the set ambiguous or leaves it a non-target singleton — never
  a match); only DETECTION changes, in the unsuppressing direction.
- **Body imports** — `use` statements direct in the fn block form
  `body_top: Imports` (their Rust scope is the whole fn body, so they
  belong to the body scope rather than `body_nested`), innermost after
  `body_nested`; like ALL imports they are NON-TERMINAL (namespace
  unknowable). `use` statements (named AND glob) anywhere deeper
  form `body_nested` — each named reading and each glob root records
  its BLOCK-NESTING DEPTH (a `Vec<(target, depth)>` per name; sibling
  conflicts union, never overwrite; nested glob roots contribute
  non-terminal candidates — `{ use tokio::task::*; spawn(…).await }`
  must stay detected). Depths order the scopes: `body_nested` entries
  are deeper than `body_top`, which is deeper than the module chain
  (chain index 0 innermost). Rule 1's inner-ness test compares these
  depths: a glob candidate strictly deeper than the named reading's
  scope blocks rule 1.

**2. Expansion is set-valued.** `expand_readings(path) -> Vec<String>`:
if the path carries the absolute marker (leading `"::"`), it is immune
to rewriting and returned as-is (after marker-aware comparison
normalization). Otherwise, rewrite the leading segment through the
named maps (innermost module wins — the scope's full candidate list
for that name branches the expansion set), and when the leading
segment has NO named binding, BRANCH through every glob root visible
in the chain (each root `G` yields candidate `G::leading`) — this
resolves glob-derived prefixes inside alias targets (`use tokio::*; use
task::spawn as s;` → `s` → `task::spawn` → `tokio::task::spawn`).
Recurse until the leading segment resolves nowhere or a LEADING NAME
repeats (cycle: `a → b::x` with `b → a::y` repeats leading names; emit
the pre-cycle string unresolved). Expansion consults the whole visible
chain (approximation; extra branches only widen the candidate set —
ambiguity blocks bounding, any-hit only aids detection: both safe).

**3. Candidate-set resolution.** `is_path_target` computes plausible
readings for the called name, then matches by class.

*Leading colon:* if `path.leading_colon` is set, the path is absolute
(extern prelude / crate root) and BYPASSES aliases: candidates = the
joined literal path only (`::tokio::time::timeout` bounds even when
`use crate::fake as tokio;` is in scope).

*Candidate walk* (innermost → outermost), for the name (single segment)
or first segment (qualified). TERMINALITY RULE: only namespace-KNOWN
bindings terminate — syntactic item categories (`fn`, `const`,
`static`, tuple/unit struct; `mod`, type items). An IMPORT's namespace
is unknowable at syn level (`use types::spawn;` may be type-only while
a same-scope glob or an outer scope provides the VALUE reading), so a
named-import hit ADDS its readings but never terminates:

1. Non-terminal locals (single-segment value lookups only): if the
   name is locally bound (let/closure/for/if-let/match-arm/nested-block
   item), add `<local>`; continue.
2. `body_nested` (non-terminal): named → add every `expand_readings`
   of each Vec target; globs → add `expand_readings(G::name)` per
   root; continue.
3. `body_top` (the fn-body scope): named → add ALL readings, continue
   (namespace unknown); namespace-known terminal body items set → add
   `<local>`, **terminal**; globs → add per root, continue; else
   transparent.
4. Real module scopes: named → add ALL readings, continue; the
   namespace-appropriate items set (`items_value` for single-segment
   value lookups, `items_type` for qualified prefixes) → `<local>`,
   **terminal**; globs → add per root, **continue** (glob may not
   export); else transparent.
5. Zero candidates for a qualified first segment → the literal joined
   path is the fallback reading (extern-crate/crate-root namespace).
   NO literal short-circuit before the walk — an aliased first segment
   shadows the extern name.

Value-namespace locals contribute only the non-terminal `<local>`
reading above (they can never suppress a wait finding); they never
touch qualified first segments (module namespace; a local cannot be a
path prefix).

*Matching by class:*

- Bounding (`TIMEOUT_TARGETS`): candidates carry PROVENANCE —
  `Named(path, scope_depth)` (explicit import reading),
  `Glob(path, scope_depth)` (glob-derived), `Literal(path)` (the
  original/extern joined path), `Local` (a non-terminal local
  binding), and bare-name artifacts (the as-written token retained
  for detection — never a resolution). The bounding set excludes only
  bare-name artifacts; `Local` readings BLOCK. Boundedness rules, in
  order:
  1. **Named rule:** a `Named` candidate that is a target bounds when
     it originates from `body_top` or a module-chain scope (a
     `body_nested` Named reading NEVER establishes bounding — its
     block-local visibility cannot be confirmed for the call site, so
     it only contributes a blocking candidate), no other `Named`
     candidate exists, no `Local` reading exists, and no `Glob`
     candidate sits in a scope INNER to the named reading's scope
     (an inner glob displaces an outer named import in Rust; same-
     scope and outer globs are displaced BY the named import). Depth
     alone does not encode ancestry, so the conservative origin
     restriction replaces per-block ancestry tracking.
  2. **Literal rule (explicit scope relaxation):** a `Literal`
     candidate that is a target (qualified paths) bounds when no
     `Named` candidate and no `Local` reading exists. Glob candidates
     do NOT block. Rationale: a glob can displace the extern-crate
     reading only by exporting a same-named module — an identity
     re-export (`pub use tokio;`) resolves to the same crate and
     changes nothing; a non-identity masquerade (`mod tokio { … }`
     mock) is a deliberate adversarial pattern, corpus-zero, and the
     detection side stays immune (any-hit). This relaxation is a
     documented decision, not an approximation discovered later.
  3. **Glob rule:** when every non-artifact candidate is `Glob`-derived,
     the set bounds iff it is a singleton target (two same-scope globs
     exporting one name are a compile error; the glob is then the only
     possible provider — the original blessed glob-singleton proof).
  Otherwise — a named non-target reading exists (`use my::timeout;`),
  two named readings conflict, a `Local` reading exists, an inner glob
  blocks, or no target reading — never bound (report).
- Detection (`WAIT_CALL_TARGETS`): match when any candidate is a
  target (provenance ignored). Ambiguity never suppresses a wait
  finding.

*Corpus evidence for the precedence rules* (measured on the live tree
during implementation): the pure singleton rule over-reports 108 sites
across ~30 files — every one a Rust-truth bounded loop whose deadline
reads through `use super::*;` or an extern glob (e.g.
`use tokio::time::timeout;` alongside `use super::*;`, or qualified
`tokio::time::timeout` alongside `use streaming::*;`), where Rust's
explicit-beats-glob / extern-prelude semantics bound the wait but the
singleton rule read {target-reading, glob-reading} as ambiguity. The
design's earlier empirical premise ("zero live glob sites") held only
for tokio-shaped globs; relative (`super::*`) and extern globs are
idiomatic here — nearly all affected files carry `use super::*;`.
Expected outcome under rules 1-3: 502 − 108 = 394 baseline, PLUS one
new TRUE finding from the fixed false-negative class (awaited
`TcpStream::connect` through a named `use tokio::net::TcpStream;`
import — the base detector's literal qualified matching missed it):
the ceiling lands at exactly 395 with the inventoried raise.

**4. Soundness.** Residual risk ledger (each entry names the concrete
wrongly-bounded shape, its precondition, and its guard):
- **R1 — inner glob displaces outer named import (BLOCKED by rule 1):**
  an inner-scope glob that truly exports the name displaces an
  outer-scope `use tokio::time::timeout;` in Rust; rule 1 refuses to
  bound while an inner glob candidate exists, so this shape reports
  (safe). Pinned by a negative unit test.
- **R2 — glob masquerade of an extern crate name (ACCEPTED, rule 2
  relaxation):** a glob exporting a NON-identity module spelled like
  an extern crate (`mod tokio { … }` mock re-exported via `super::*`)
  displaces the extern reading of a qualified path; rule 2 bounds
  anyway. Precondition: deliberate in-repo masquerade of a foreign
  crate name — corpus-zero, adversarial-only. Identity re-exports
  (`pub use tokio;`) resolve to the same crate and are harmless.
  Detection is immune (any-hit). Guard: the ratchet ceiling plus the
  module-doc residual note; revisit only if a masquerade pattern ever
  appears in-repo.
- **R3 — type-only named import (safe direction):** a type-only
  import of the name counts as a `Named` reading and blocks rules 1-2
  → over-report, never suppression.
- **Local bindings (BLOCKED):** any `Local` reading blocks bounding —
  a local `let timeout = …` that plausibly resolves the call must not
  be bounded over. Pinned by a negative unit test.
Glob-singleton bounding (rule 3) keeps the original blessed proof:
compilation forces the glob to provide the name; `tokio::time`
exports `timeout`/`timeout_at` as stable pinned API; dual same-scope
globs exporting one name are a compile error.
Remaining approximations are conservative: nested-block unions and
non-terminal locals over-report/over-detect (a shadowed import call
may be reported and adjudicated with the marker — never silently
missed); set-valued expansion may over-branch (blocks bounding, never
suppresses detection); alias cycles stop unresolved. Implementation
hardening (required in tasks): candidate deduplication and
visited-state memoization per expansion keep the branch product finite
and cheap.

**5. Consumers unchanged.** `WAIT_METHODS`/`BLOCKING_METHODS`
method-call rules untouched. `is_path_target` gains a class parameter
(or thin `is_bounding_path`/`is_wait_path` wrappers).

**6. Docs.** Module doc resolution section and the
`ratchet-unbounded-wait.max` header updated to the candidate-set
contract (including the rule-2 relaxation note). The ceiling landing
gate: exactly 395 — the 394 baseline plus the one inventoried new
TRUE finding from the fixed false-negative class (awaited
`TcpStream::connect` through a named import; raise committed with
dated justification in the ratchet file, bd rc-krbym). Verified by a
base-vs-new finding-list diff on the same tree (+1 site, 0 removals),
captured in the task trail. Any further deviation is adjudicated per
ratchet convention (burn-down note on decrease; never raise without
new true findings, each inventoried).

**7. Testability.** Every delta-spec scenario maps 1:1 to a named
in-module unit test with an exact fixture string; tasks.md pins each
test name, fixture, and expected `findings()` result. Bounding
fixtures carry their inner wait as `async { rx.recv().await }` (a
bare `rx.recv()` without `.await` is never detectable and makes a
test vacuous). The mission-201 test
`recv_inside_aliased_timeout_not_reported` is vacuous in exactly that
way: it is preserved unchanged for regression pinning AND joined by a
non-vacuous companion (`with_deadline(d, async { rx.recv().await
}).await` → no findings).

## Affected crates

- `scripts/xtask` (dev-tooling): `lint_unbounded_wait.rs` resolution
  logic, tests, module doc; `ratchet-unbounded-wait.max` header comment
  only (ceiling integer untouched unless a findings-diff demands it).

## Architecture boundaries

Assurance tooling only (ADR-0069 §13.2 R1 enforcement). No Runtime,
DSL, Components, Services, or Languages crate is modified. Normative
requirement lands in the `test-determinism` capability beside the
existing deadline-bounded loop requirement from the loopsweep change.
