# Evidence: deps-majors-minijinja-ed25519

Worktree: `/home/shared/rust-camel-worktrees/depsmajors` (branch `feature/depsmajors`).
Base commit: `fdddcd3ee`. Toolchain: rustc/cargo 1.99.1.
Captured 2026-11-06.

Locked-graph policy: every routine cargo invocation below passes `--locked`.
The single exception is the `--dry-run` resolver reproduction under
[transitive dup blocker](#transitive-dup-blocker), which by contract never
writes `Cargo.lock`. No command in this task modified `Cargo.lock`; the
post-run `git diff --stat Cargo.lock` is recorded in that section and is empty.

## minijinja 3.0.5

### Suite run

```text
$ cargo test --locked -p camel-template -p camel-language-minijinja --all-targets
exit status: 0
```

Per-binary summary lines (tail):

```text
     Running unittests src/lib.rs (target/debug/deps/camel_language_minijinja-0a1ce5053e6588f9)
test result: ok. 71 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
     Running tests/exact_diagnostics.rs (target/debug/deps/exact_diagnostics-63f028d21e2e6834)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/missing_value_semantics.rs (target/debug/deps/missing_value_semantics-0efa02ffa1b713e1)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/lib.rs (target/debug/deps/camel_template-c1883aa5b9929c2b)
test result: ok. 126 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
     Running tests/engine_free.rs (target/debug/deps/engine_free-81ea4f32a064a38d)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/template_reload_integration.rs (target/debug/deps/template_reload_integration-45c420a3947c6e64)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/template_render_integration.rs (target/debug/deps/template_render_integration-fdaf5016051564e8)
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Aggregate: 234 passed, 0 failed, 0 ignored across 7 test binaries
(`camel-language-minijinja` lib 71 + `exact_diagnostics` 7 +
`missing_value_semantics` 4; `camel-template` lib 126 + `engine_free` 1 +
`template_reload_integration` 6 + `template_render_integration` 19).

### Duplicate check: `cargo tree -d`

`--locked` enforces the frozen graph and does not change the resolved tree.

```text
$ cargo tree -d --locked | grep -n minijinja
1221:│   │   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja)
1500:│   │   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
1531:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
1908:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
2275:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
2302:├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
2664:│   │   │   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
3054:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
3528:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
3933:│   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
4200:│   │   │   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
4259:│   │   │   ├── minijinja v3.0.5
4260:│   │   │   │   ├── camel-language-minijinja v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/languages/camel-language-minijinja) (*)
```

A duplicated package is printed by `cargo tree -d` as a column-0 section
header, i.e. a line beginning `minijinja v`. There is none, and the whole
output contains exactly one `minijinja` package node. Note that an unanchored
substring pattern is not a valid count — it also matches the
`camel-language-minijinja` crate name — so the boundary-aware pattern is the
one that answers the question:

```text
$ cargo tree -d --locked | grep -c '^minijinja v'
0

$ cargo tree -d --locked | grep -c 'minijinja v'
13
(12 of the 13 are `camel-language-minijinja v0.65.0` intermediate nodes — the
pattern substring-matches the crate name; the 13th is the single
`minijinja v3.0.5` node.)

$ cargo tree -d --locked | grep -cE '(^|[^a-zA-Z-])minijinja v'
1
(boundary-aware count: exactly one `minijinja` package node, version 3.0.5 —
no duplicate version.)
```

The single `minijinja v3.0.5` line is an intermediate node of the inverse
(incoming-edge) tree that `-d` prints, with its two consumers
`camel-language-minijinja` and `camel-template` beneath it. No duplicate
version exists.

### Duplicate sections: ed25519-dalek / jsonwebtoken / surrealdb-core

Target note: `ed25519-dalek 2.2.0` is reachable only in the union-of-targets
graph. On the host target `cargo tree --locked` contains `jsonwebtoken
10.4.0` and `ed25519-dalek 3.0.0` but no `ed25519-dalek 2.2.0`, so the
host-only `cargo tree -d` lists the jsonwebtoken duplicate but not the dalek
one. The `--target all` run below resolves the same lockfile against the
superset target set; both runs are recorded rather than one being silently
substituted for the other.

Host target:

```text
$ cargo tree -d --locked | grep -E '^(ed25519-dalek|jsonwebtoken|surrealdb-core)'
jsonwebtoken v10.4.0 (*)
jsonwebtoken v11.1.0 (*)
```

`--target all`:

```text
$ cargo tree -d --target all --locked | grep -E '^(ed25519-dalek|jsonwebtoken|surrealdb-core)'
ed25519-dalek v2.2.0 (*)
ed25519-dalek v3.0.0 (*)
jsonwebtoken v10.4.0 (*)
jsonwebtoken v11.1.0 (*)
```

ed25519-dalek v2.2.0 section, verbatim (`cargo tree -d --target all --locked`,
inverse tree, captured log lines 2109-2111):

```text
│   ├── curve25519-dalek v4.1.3
│   │   └── ed25519-dalek v2.2.0
│   │       └── jsonwebtoken v10.4.0 (*)
```

ed25519-dalek v3.0.0 section, verbatim (same command, captured log lines
2203-2205):

```text
    ├── curve25519-dalek v5.0.0
    │   └── ed25519-dalek v3.0.0
    │       └── camel-cli v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-cli)
```

jsonwebtoken sections, verbatim column-0 duplicate headers (identical in both
runs):

```text
jsonwebtoken v10.4.0 (*)

jsonwebtoken v11.1.0 (*)
```

surrealdb-core: single version in the lockfile (`3.2.4`), therefore it is not
a duplicate and no column-0 section exists. The node and its single incoming
edge, verbatim (`cargo tree -d --locked`, captured log lines 360-366; the
repeated `(*)` occurrences elsewhere in the inverse tree are the same single
package):

```text
├── surrealdb v3.2.4
│   └── camel-component-surrealdb v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/components/camel-component-surrealdb) (*)
├── surrealdb-core v3.2.4
│   └── surrealdb v3.2.4 (*)
└── surrealdb-types v3.3.5
    ├── surrealdb v3.2.4 (*)
    └── surrealdb-core v3.2.4 (*)
```

### ed25519-dalek 2.2.0 parent chain

```text
$ cargo tree -i ed25519-dalek@2.2.0 --target all --locked
ed25519-dalek v2.2.0
└── jsonwebtoken v10.4.0
    └── surrealdb-core v3.2.4
        └── surrealdb v3.2.4
            └── camel-component-surrealdb v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/components/camel-component-surrealdb)
                ├── camel-bundles v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-bundles)
                │   ├── camel-cli v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-cli)
                │   ├── camel-integration-test v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-integration-test)
                │   │   └── camel-cli v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-cli)
                │   └── stream-echo v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/examples/stream-echo)
                └── surrealdb-example v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/examples/surrealdb-example)
```

Sole parent chain: `ed25519-dalek 2.2.0` <- `jsonwebtoken 10.4.0` <-
`surrealdb-core 3.2.4` <- `surrealdb 3.2.4` <- `camel-component-surrealdb`.
The remaining lines are workspace consumers of `camel-component-surrealdb`.

### `from_serialize` migration completeness

```text
$ grep -rn "from_serialize" crates/ --include='*.rs'
(no output; exit status 1)
```

## k8s-openapi / wasmtime status

```text
$ grep -A1 'name = "k8s-openapi"' Cargo.lock
name = "k8s-openapi"
version = "0.29.0"
```

```text
$ grep -A1 'name = "kube"' Cargo.lock
name = "kube"
version = "5.0.0"
```

```text
$ grep -A1 'name = "wasmtime-wasi"' Cargo.lock
name = "wasmtime-wasi"
version = "49.0.5"
```

`k8s-openapi` is already `0.29.0` and `kube` `5.0.0` (resolved on main by
mission 490; the order's upgrade premise was stale, not re-opened).
`wasmtime-wasi` remains on `49.0.5`: the 49 -> 50 migration is out of scope
for this change and stays tracked in bd `rc-k93qw`.

## transitive dup blocker

The original mission-start resolver experiment was read-only on the
then-clean lock and altered nothing (`git diff --stat Cargo.lock` was empty).
This reproduction uses `--dry-run`, which resolves for the error text but
never writes `Cargo.lock`, guaranteeing the same. This is the single
design-blessed exception to `--locked`.

```text
$ cargo update --dry-run jsonwebtoken@10.4.0 --precise 11.1.0
exit status: 101
stdout: (empty)
stderr:
    Updating crates.io index
error: failed to select a version for the requirement `jsonwebtoken = "^10.3.0"`
candidate versions found which didn't match: 11.1.0
location searched: crates.io index
required by package `surrealdb-core v3.2.4`
    ... which satisfies dependency `surrealdb-core = "^3.2.4"` (locked to 3.2.4) of package `surrealdb v3.2.4`
    ... which satisfies dependency `surrealdb = "^3"` (locked to 3.2.4) of package `camel-test v0.65.0 (/home/shared/rust-camel-worktrees/depsmajors/crates/camel-test)`
```

Lock state after the dry run:

```text
$ git diff --stat Cargo.lock
(no output; still empty)

$ git status --short
(no output)
```

`surrealdb-core 3.2.4` requires `jsonwebtoken ^10.3.0`, so the resolver cannot
unify on `11.1.0`; the `ed25519-dalek 2.2.0` duplicate is therefore
ecosystem-blocked until an upstream `surrealdb-core` release moves to
jsonwebtoken 11.

## ed25519-dalek 3.0 signing vector

```text
$ cargo test --locked -p camel-cli --test signing_compat_test
exit status: 0
```

```text
running 3 tests
test regenerate_fixture ... ignored, slow test: fixture regeneration helper, overwrites the fixture file; run explicitly with --ignored
test fixture_inputs_are_stable ... ok
test fixture_envelope_from_dalek_2_verifies_under_3 ... ok

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The two active fns are `fixture_inputs_are_stable` and
`fixture_envelope_from_dalek_2_verifies_under_3`; the regeneration helper
`regenerate_fixture` stays `#[ignore]`d and was not run.

### Contract asserted by the test

`crates/camel-cli/tests/signing_compat_test.rs` lines 1-8:

```rust
//! Cross-version compatibility for the detached Ed25519ph envelope
//! (ADR-0083): a frozen envelope produced with ed25519-dalek 2.2.0 must
//! verify and re-sign byte-identically under 3.x.
//!
//! The fixture under `tests/fixtures/ed25519ph_envelope_dalek_2_2.hex`
//! carries its own provenance header and regeneration recipe. The seed
//! and prehash below are the public synthetic inputs that recipe uses;
//! the fixture file is the committed cross-version contract.
```

Fixture: `crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex`
(committed, 1337 bytes, with its own provenance header and regeneration
recipe).

### ADR-0083 grounding

`docs/adr/0083-artifact-signing-envelope.md`, section `## Decision`,
paragraph `**Detached envelope.**` — the fixed 148-byte layout (offsets for
verifying key / signature / checksum / terminal magic), quoting at most five
lines:

```text
| 12 | 32 | Ed25519 verifying key (compressed) |
| 44 | 64 | signature |
| 108 | 32 | BLAKE3 checksum |
| 140 | 8 | terminal magic `CAMELSG1` |
```

Same section, paragraph `**Ed25519ph, not pure Ed25519.**` — the prehash
rule that makes the byte identity reproducible:

```text
Ed25519ph (RFC 8032 §5.1 prehash with a null context) hashes the message
with SHA-512 while it streams, then signs the 64-byte digest. Signing and
verification both use flat memory and one stream per invocation.
```

### Locked-graph check

Expected empty; observed empty in both commands.

```text
$ git diff fdddcd3ee...HEAD -- Cargo.toml '*/Cargo.toml' Cargo.lock 'crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex'
(no output; exit status 0 — EMPTY)

$ git status --short -- Cargo.toml '*/Cargo.toml' Cargo.lock 'crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex'
(no output; exit status 0 — EMPTY)
```

No manifest, lockfile, or signing fixture changed anywhere in this mission's
history relative to `fdddcd3ee`.

## mission summary

### (a) Per-dependency outcome

| dependency | change | outcome | evidence |
|------------|--------|---------|----------|
| minijinja | 2.24 -> 3.0.5 | **MIGRATED** by mission 490 (commit `54ad37c97`); verified this mission | 234 tests green (incl. 7 exact-diagnostic pins), zero `from_serialize` call sites, exactly one `minijinja v3.0.5` in the tree (no duplicate) |
| ed25519-dalek | 2.2 -> 3.0 | **MIGRATED** by mission 490 (commit `3ef664e22`); verified this mission | `signing_compat_test` green: frozen 2.2.0-produced envelope verifies and re-signs byte-identically under 3.x; workspace direct surface is a single `ed25519-dalek 3.0.0` |
| ed25519-dalek (transitive) | 2.2.0 duplicate | **ECOSYSTEM-BLOCKED** | `jsonwebtoken 10.4.0` <- `surrealdb-core 3.2.4` pins `jsonwebtoken ^10.3.0`, and that version pins `ed25519-dalek 2.2.0`; the `--dry-run` unification to `11.1.0` fails, so the 2.2.0 node cannot be removed until upstream `surrealdb-core` moves to jsonwebtoken 11. See [transitive dup blocker](#transitive-dup-blocker). |

**Carry-forward (required):** the duplicate is target-gated, so the check
must be target-explicit. On the host target, plain `cargo tree -d` does NOT
list the `ed25519-dalek` duplicate — `jsonwebtoken`'s `ed25519-dalek`
dependency is gated to non-host targets, and the host graph resolves only
`ed25519-dalek 3.0.0`. `cargo tree -d --target all` shows both `2.2.0` and
`3.0.0` as duplicate roots. Both runs are recorded verbatim under
[Duplicate sections: ed25519-dalek / jsonwebtoken / surrealdb-core](#duplicate-sections-ed25519-dalek--jsonwebtoken--surrealdb-core);
the Target note there documents the host-vs-`--target all` distinction. Do
not conclude "no dalek dup" from a host-only tree.

### (b) Deferred items

- `wasmtime-wasi` 49 -> 50: **deferred**, stays in bd `rc-k93qw`. It is a
  breaking component API change (`HasIo` / `HasData`) and needs its own
  mission; out of scope for this change. Lock remains `49.0.5`.
- `k8s-openapi` 0.28 -> 0.29: **already resolved on main** by mission 490
  (single lock version `0.29.0`, `kube 5.0.0`). The mission order's premise
  was stale; it was not re-opened.

### (c) Gate-input table (515.2-515.4)

Exit codes for the 515.2 rows were the declared acceptance preconditions of
that task at capture time (the task is checked off with the suite green);
515.3 and 515.4 rows are reproduced in this file. This task re-ran only the
signing vector, per its scope.

| command | exit | result |
|---------|------|--------|
| `cargo fmt -p camel-language-minijinja` + `cargo fmt --check -p camel-language-minijinja` (515.2) | 0 | clean, no diff |
| `cargo clippy -p camel-language-minijinja --all-targets -- -D warnings` (515.2) | 0 | no findings |
| `cargo test --locked -p camel-language-minijinja --all-targets` (515.2) | 0 | 71 lib + 7 `exact_diagnostics` + 4 `missing_value_semantics` = 82 passed |
| `grep -n "contains(" crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs` (515.2) | 1 | no hits: all pins are full-message `assert_eq!` |
| `cargo test --locked -p camel-template -p camel-language-minijinja --all-targets` (515.3) | 0 | 234 passed, 0 failed, 0 ignored across 7 binaries |
| `grep -rn "from_serialize" crates/ --include='*.rs'` (515.1/515.3) | 1 | no hits: zero call sites |
| `cargo tree -d --locked \| grep -c '^minijinja v'` | 1 (zero matches) | `0`: no `minijinja` duplicate; single `minijinja v3.0.5` |
| `cargo tree -d --locked \| grep -E '^(ed25519-dalek\|jsonwebtoken\|surrealdb-core)'` (host) | 0 | `jsonwebtoken 10.4.0` + `11.1.0` only; no `ed25519-dalek` duplicate on host (target-gated) |
| `cargo tree -d --target all --locked \| grep -E '^(ed25519-dalek\|jsonwebtoken\|surrealdb-core)'` | 0 | `ed25519-dalek 2.2.0` + `3.0.0`, `jsonwebtoken 10.4.0` + `11.1.0` |
| `cargo tree -i ed25519-dalek@2.2.0 --target all --locked` | 0 | sole chain: `jsonwebtoken 10.4.0` <- `surrealdb-core 3.2.4` <- `surrealdb 3.2.4` <- `camel-component-surrealdb` |
| `grep -A1 'name = "k8s-openapi"' Cargo.lock` / `kube` / `wasmtime-wasi` | 0 | `k8s-openapi 0.29.0`, `kube 5.0.0`, `wasmtime-wasi 49.0.5` |
| `cargo update --dry-run jsonwebtoken@10.4.0 --precise 11.1.0` | 101 | expected resolver failure: `jsonwebtoken = "^10.3.0"` required by `surrealdb-core v3.2.4`; lock untouched |
| `git diff --stat Cargo.lock` (post dry-run) | 0 | empty |
| `cargo test --locked -p camel-cli --test signing_compat_test` (515.4) | 0 | `test result: ok. 2 passed; 0 failed; 1 ignored` |
| `git diff fdddcd3ee...HEAD -- Cargo.toml '*/Cargo.toml' Cargo.lock 'crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex'` (515.4) | 0 | EMPTY: protected paths untouched |
| `git status --short -- <same protected paths>` (515.4) | 0 | EMPTY: working tree clean for those paths |

### (d) Buzzer draft

depsmajors verified: both majors landed by 490, suites+vector green, transitive ed25519 dup ecosystem-blocked (surrealdb-core→jwt ^10.3), host tree -d misses target-gated dalek dup — use --target all
