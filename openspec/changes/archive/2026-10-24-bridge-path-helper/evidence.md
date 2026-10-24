# Evidence: bridge-path-helper

## Checkpoint A — wiring guard is red against a drifted production path

Task 1.1 proves that `ensure_binary_uses_workspace_layout` pins the current
workspace-layout construction before the helper extraction. The test seeds a
dummy at the independent oracle path `bridges/cxf/build/native/cxf-bridge`,
uses an empty cache and a loopback release URL, and asserts production returns
exactly that path.

Seeding safety: `rg -n "CXF_BRIDGE" crates/services/camel-bridge/` shows no
existing test resolves the CXF workspace binary or calls
`workspace_binary_present(&CXF_BRIDGE)`, so no lock was needed.

### 1. Clean tree — green

Command:

```
cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout
```

Output tail:

```
running 1 test
test download::tests::ensure_binary_uses_workspace_layout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 89 filtered out
```

Exit code: `0`

### 2. Sabotage (not committed) — red

Production `ensure_binary_for_spec` temporarily changed from
`.join("build")` to `.join("out")` in the workspace-layout construction.

Command (same as above):

```
cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout
```

Output tail:

```
running 1 test
test download::tests::ensure_binary_uses_workspace_layout ... FAILED

failures:

---- download::tests::ensure_binary_uses_workspace_layout stdout ----

thread 'download::tests::ensure_binary_uses_workspace_layout' panicked at crates/services/camel-bridge/src/download.rs:707:20:
production must resolve the seeded workspace binary: UrlNotAllowed("http://127.0.0.1:9")

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 89 filtered out
```

Exit code: `101` — assertion failure on the returned `Err`, not a skip and not
a setup error.

### 3. Restored tree — green

Exact original `.join("build")` line restored; `git diff` shows no `join("out")`
residue.

Command (same as above):

```
running 1 test
test download::tests::ensure_binary_uses_workspace_layout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 89 filtered out
```

Exit code: `0`

## Checkpoint B — helper drift is red for the layout oracle, green for the wiring guard

Task 1.2 extracts the workspace layout into `workspace_binary_path_from`.
Both production and the skip guard now call it, and the wiring guard seeds its
dummy through `workspace_binary_path` (the shared helper). The independent
oracle `workspace_binary_path_matches_documented_layout` rebuilds the expected
path from local literals, so only it can detect helper drift.

### 1. Clean tree — both green

Command:

```
cargo test -p camel-bridge --lib download::tests::workspace_binary_path_matches_documented_layout
```

Output tail:

```
running 1 test
test download::tests::workspace_binary_path_matches_documented_layout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out
```

Exit code: `0`

Command:

```
cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout
```

Output tail:

```
running 1 test
test download::tests::ensure_binary_uses_workspace_layout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out
```

Exit code: `0`

### 2. Sabotage the HELPER (not committed) — layout oracle red, wiring guard green

`workspace_binary_path_from` temporarily changed from `.join("build")` to
`.join("out")` (a single line, above `ensure_binary_for_spec`).

Layout oracle, command (same as above):

```
running 1 test
test download::tests::workspace_binary_path_matches_documented_layout ... FAILED

---- download::tests::workspace_binary_path_matches_documented_layout stdout ----

thread 'download::tests::workspace_binary_path_matches_documented_layout' panicked at crates/services/camel-bridge/src/download.rs:645:13:
assertion `left == right` failed: documented workspace layout drifted for jms-bridge
  left: "/home/shared/tmp/.tmpxIRnMY/bridges/jms/out/native/jms-bridge"
 right: "/home/shared/tmp/.tmpxIRnMY/bridges/jms/build/native/jms-bridge"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 90 filtered out
```

Exit code: `101` — the independent oracle detected the drift.

Wiring guard, command (same as above):

```
running 1 test
test download::tests::ensure_binary_uses_workspace_layout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out
```

Exit code: `0` — the seed uses the same helper as production, so both sites
drift together; this is why the independent layout oracle exists.

### 3. Restored tree — both green

Exact original `.join("build")` line restored. Layout oracle exit code: `0`,
wiring guard exit code: `0` (same commands as above).

## Checkpoint C — full crate suite green, no sabotage residue

Command:

```
cargo test -p camel-bridge
```

Output tail:

```
test result: ok. 91 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
test bridge_startup_uses_system_broken ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Exit code: `0`. The non-skipped wiring run
`download::tests::ensure_binary_uses_workspace_layout` executed and passed as
part of the 91 lib tests (it did not take the read-only-checkout skip branch).

No sabotage remains in the committed source: `git grep -n 'join("out")' -- crates/ scripts/`
exits `1` (no matches), and `git status` shows only
`crates/services/camel-bridge/src/download.rs` plus this evidence file.
