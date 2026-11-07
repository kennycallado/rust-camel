# Evidence: surrealdb 3.2.4 → 4.0.0 advisory upgrade

Captured during Task 1.1. Raw artifacts live outside the repo in
`/home/shared/tmp/surrealdbadv-evidence/`.

## Base and worktree

- Base commit: `c70c29cb0` (comparison point for `Cargo.toml` / `Cargo.lock`).
- Worktree: `/home/shared/rust-camel-worktrees/surrealdbadv`, branch
  `feature/surrealdbadv`.
- Change under test is the working tree at capture time (not committed):
  one line in root `Cargo.toml` (`surrealdb` requirement `"3"` → `"4"`) plus
  the `cargo update -p surrealdb` re-resolution of `Cargo.lock`.
- Toolchain: `cargo 1.99.1`, `cargo-audit 0.22.1`, advisory DB 1422
  advisories.

## Before audit (`audit-before.txt`)

- `cargo audit` exit 1: **2 vulnerabilities**
  - `surrealdb 3.2.4` — RUSTSEC-2026-0453
  - `surrealdb-core 3.2.4` — RUSTSEC-2026-0454
- 7 allowed warnings (informational/yanked): RUSTSEC-2026-0440
  (`astral-tokio-tar 0.6.4`), RUSTSEC-2023-0089 (`atomic-polyfill 1.0.3`),
  RUSTSEC-2025-0141 (`bincode 2.0.1`), RUSTSEC-2024-0436 (`paste 1.0.15`),
  RUSTSEC-2026-0441 (`pin-utils 0.1.0`), RUSTSEC-2025-0134
  (`rustls-pemfile 2.2.0`), plus yanked `async-executor 1.14.0` (no ID).

## After audit (`audit-after.txt`)

- `cargo audit` exit **0**: no `error:` line, no vulnerability is reported
  against any crate in the lock. (cargo-audit 0.22 prints no `0
  vulnerabilities` success sentence; a clean run is exit 0 with only the
  warnings block, so "0 vulnerabilities" is asserted as "no vulnerability
  IDs and no `error:` line", verified below.)
- 7 allowed warnings, **identical advisory set** to `audit-before.txt`
  (same six IDs plus the yanked `async-executor 1.14.0`); `audit-after.txt`
  diffed line-by-line against `audit-before.txt` — the advisory-set
  identity is exact, and the residual line-level differences are limited
  to the scan-count header (`1220` → `1259` crate dependencies) and
  dependency-tree re-indentation inside warning blocks (including the new
  `└── surrealmx 1.1.0` tree line).
- Cross-check with the project ignore list removed
  (`audit-unignored-before.txt` / `audit-unignored-after.txt`, run with
  `cargo audit -f <lock> --no-fetch` in a directory without
  `.cargo/audit.toml`): before = 5 vulnerabilities / 8 warnings; after =
  2 vulnerabilities (`rsa 0.9.10` RUSTSEC-2023-0071, `crypto-bigint 0.5.5`
  RUSTSEC-2026-0387) / 8 warnings. So the bump removed 3 of the 5
  pre-existing vulnerability hits (RUSTSEC-2026-0453, -0454, -0373) and
  left the two ecosystem-blocked ones, which stay covered by the
  pre-existing, documented ignores.

## Versions

| Crate | Before (3.2.4) | After (4.0.0) | Note |
| --- | --- | --- | --- |
| `surrealdb` | 3.2.4 | 4.0.0 | required |
| `surrealdb-core` | 3.2.4 | 4.0.0 | required |
| `affinitypool` | 0.7.0 | 0.8.0 | required ≥ 0.8.0 |
| `reblessive` | 0.4.3 | — (removed) | no longer in lock |
| `rsa` | 0.9.10 | 0.9.10 | unchanged; RUSTSEC-2023-0071 (no patched release) |
| `rkyv` | — (absent) | 0.8.18 | new, patched (≥ 0.8.17) |
| `ammonia` | 4.1.7 | 4.2.4 | patched (≥ 4.2.4) |
| `crypto-bigint` | 0.5.5 | 0.5.5 | unchanged; RUSTSEC-2026-0387 |
| `jsonwebtoken` | 10.4.0 + 11.1.0 | 10.4.0 + 11.1.0 | duplicate unchanged |
| `ed25519-dalek` | 2.2.0 + 3.0.0 | 2.2.0 + 3.0.0 | duplicate unchanged |

`affinitypool`, `surrealdb`, `surrealdb-core` all satisfy the Task 1.1
acceptance conditions (`surrealdb = 4.0.0`, `surrealdb-core = 4.0.0`,
`affinitypool = 0.8.0 ≥ 0.8.0`).

## Lock delta (`lock-diff.txt`, vs `c70c29cb0`)

`cargo update -p surrealdb` moved 116 lock entries: 48 added, 9 removed,
59 version-changed. 27 of them are the `surrealdb*` family (19 new crates
from the 4.0 split — `surrealdb-catalog`, `-cnf`, `-common`, `-datastore`,
`-diskann`, `-engine-api`, `-engine-local`, `-expr`, `-iam`, `-idx`,
`-keyspace-macro`, `-kvs`, `-kvs-any`, `-kvs-mem`, `-observe`,
`-parser-next`, `-rpc`, `-runtime`, `-stored-derive` — plus 8 version
changes: `surrealdb`, `surrealdb-core`, `surrealdb-collections`,
`-parse-common`, `-protocol` (0.10.2/0.13.1 → 0.14.1), `-strand`,
`-types`, `-types-derive`).

89 non-`surrealdb*` transitives moved:

- Added (29): `artmap`, `bytecheck`, `bytecheck_derive`, `byteslice`,
  `crawdad`, `crc-fast`, `daachorse`, `egg`, `env_logger`, `generator`,
  `lindera`, `lindera-dictionary`, `loom`, `memmap2`, `munge`,
  `munge_macro`, `ptr_meta`, `ptr_meta_derive`, `quanta`, `rancor`,
  `raw-cpuid`, `regex-lite`, `rend`, `rkyv`, `rkyv_derive`, `scoped-tls`,
  `slotmap`, `symbol_table`, `symbolic_expressions`.
- Removed (9): `bnum`, `crossbeam-skiplist`, `diskann`, `earcutr`,
  `fastnum`, `reblessive`, `spade`, `utf-8`, `vart`.
- Changed (51): `affinitypool` 0.7.0→0.8.0, `ammonia` 4.1.7→4.2.4,
  `cssparser` 0.37.0→0.38.0, `html5ever` 0.39.0→0.40.1, `markup5ever`
  0.39.0→0.40.0, `string_cache` 0.9.0→0.11.0,
  `string_cache_codegen` 0.6.1→0.11.2, `object_store` 0.13.2→0.14.2,
  `diskann-utils`/`-vector`/`-wide` 0.54.0→0.61.0, `sysinfo`
  0.37.2→0.39.6, `surrealmx` 0.22.0→1.1.0, `crossbeam-deque`/`-epoch`/
  `-queue`, `headers`, `heck`, `ipnet`, `itertools`, `phf*`, `quick-xml`
  (0.42.0 retained, 0.41.0 added), `smallvec` 1.15.2→1.16.4, `spin`,
  `tokio-tungstenite`/`tokio-tungstenite-wasm`/`tungstenite`, `unicase`,
  `web_atoms`, `wasm-bindgen*`, `web-sys`, `js-sys`, and the `windows*`
  family (0.61.x → 0.62.2 / 0.100.0 target-gated).

`Cargo.toml` diff vs base is exactly one line (the `surrealdb` requirement),
so every other lock movement is resolver fallout from the `surrealdb`
5-crate family upgrade.

## audit.toml

`audit.toml` unchanged — warning set identical to `audit-before.txt`
(7 allowed warnings, same six IDs plus yanked `async-executor`).

Deviation from the plan assumption "all 5 ignores still encountered":
3 of 5 ignores still match a crate in the post-bump lock, 2 are now stale.
Verified by re-running `cargo audit -f <lock>` with the ignore list absent
(appendix files):

- Still matched: RUSTSEC-2023-0071 (`rsa 0.9.10`, no patched release),
  RUSTSEC-2026-0249 (`smartstring 1.0.1`, unmaintained),
  RUSTSEC-2026-0387 (`crypto-bigint 0.5.5`, patched only in ≥ 0.7.6).
- Now stale (no matching crate, so the ignore is a no-op):
  - RUSTSEC-2026-0373 (`ammonia`): 4.1.7 → 4.2.4, patched range
    `>= 4.2.4`, so the advisory no longer fires.
  - RUSTSEC-2026-0235 (`rkyv`): no `rkyv < 0.8.17` is in the lock. This
    one was already stale **before** the bump (`rkyv` was absent from the
    base lock entirely); post-bump it is `0.8.18`, which is patched.

Neither stop-token condition fired: no `audit-regression` (no
vulnerability appears that was not already ignored) and no
`warning-set-drift` (warning set identical). Removing the two stale
ignore entries is follow-up work for the audit-config owner, out of Task
1.1 scope — `audit.toml` is untouched here by design. Tracked as:
`rc-9mq7v` (ammonia, exit condition now met by this bump, commented) and
`rc-fiw58` (rkyv, created with `discovered-from:rc-yhpyz`).

## Dup trees

- `tree-surrealdb.txt`: `surrealdb v4.0.0` ← only
  `camel-component-surrealdb 0.65.0` (plus its example, test and bundle
  consumers). No other in-graph crate depends on `surrealdb`.
- `tree-jwt.txt` (both qualifiers, since two versions remain in lock):
  - `jsonwebtoken@10.4.0` — reached from `surrealdb-core v4.0.0`
    (6 chain lines). Survives because SurrealDB 4.0 still links
    `jsonwebtoken` 10 for token verification.
  - `jsonwebtoken@11.1.0` — reached from `camel-auth 0.65.0` only (no
    `surrealdb-core` line).
- `tree-ed25519.txt`:
  - `ed25519-dalek@2.2.0` ← `jsonwebtoken 10.4.0` ← `surrealdb-core
    v4.0.0` (`surrealdb-expr`, `surrealdb-catalog`, `surrealdb-datastore`,
    `surrealdb-idx`, `surrealdb-runtime`) → `surrealdb-engine-local` →
    `surrealdb` → `camel-component-surrealdb`.
  - `ed25519-dalek@3.0.0` ← `camel-cli 0.65.0` only.
- `tree-dup-target-all.txt`: cross-target duplicate scan. Beyond the two
  crypto pairs above, duplicates remain in `itertools` (0.13/0.14/0.15),
  `phf` (0.11/0.12/0.14), `quick-xml` (0.41/0.42), `spin` (0.9/0.10),
  `tokio-tungstenite` (0.29/0.30) and the `windows*` target-gated family —
  all pre-existing or resolver fallout, none inside the advisory scope.

**Finding:** `jsonwebtoken 10.4.0` + `ed25519-dalek 2.2.0` remain in the
lock via `surrealdb-core 4.0.0`. This is ecosystem-blocked (SurrealDB
controls that dependency), out of scope for this change, and the same
class of deferral as mission 515. `ammonia`, `rkyv` and the two
`surrealdb*` advisories are resolved; `rsa` and `crypto-bigint` keep
their documented, non-applicable ignore entries.

## Artifacts

| File | Bytes | Content |
| --- | --- | --- |
| `lock-diff.txt` | 57947 | `git diff c70c29cb0 -- Cargo.lock` |
| `audit-after.txt` | 53102 | `cargo audit` with project `audit.toml` |
| `audit-unignored-before.txt` | 71271 | `cargo audit -f lock-before` without ignores |
| `audit-unignored-after.txt` | 79900 | `cargo audit -f lock-after` without ignores |
| `tree-surrealdb.txt` | 830 | `cargo tree --target all -i surrealdb` |
| `tree-jwt.txt` | 101577 | both qualified `jsonwebtoken` inversions |
| `tree-ed25519.txt` | 1967 | both qualified `ed25519-dalek` inversions |
| `tree-dup-target-all.txt` | 458442 | `cargo tree -d --target all` |
| `update.log` | 4719 | full `cargo update -p surrealdb` output |

## Local verification (task 1.2)

State at capture: worktree `/home/shared/rust-camel-worktrees/surrealdbadv`,
branch `feature/surrealdbadv`, HEAD `b5642a45e`; lock resolves
`surrealdb 4.0.0`. Working tree clean before the run (`git status` empty).

Every cargo command ran foreground through
`systemd-run --user --scope --collect --unit=fleet-surrealdbadv`
(`MemoryMax=12G`, `MemorySwapMax=2G`, `TasksMax=1024`, `OOMPolicy=kill`,
`CARGO_BUILD_JOBS=6`, `TMPDIR=/home/shared/tmp`, `cargo ... -j4`), cwd =
worktree root, with `PATH` = the validated devshell subset from the
tasks.md containment preamble (sccache, gcc-wrapper `cc`, gnumake,
rust-default, cargo-audit, coreutils, `/run/current-system/sw/bin`,
`/usr/bin`, `/bin`). The scope is `--collect` (auto-GC after each run) and
`pgrep -c compiled_artifa` was 0 immediately before and after each
command. Raw logs:
`/home/shared/tmp/surrealdbadv-evidence/task12-*.log`.

| # | Command | Exit | cargo wall time | Log |
| --- | --- | --- | --- | --- |
| 1 | `cargo build --workspace` | 0 | 13m 13s | `task12-build.log` |
| 2 | `cargo check --workspace --all-targets` | 0 | 4m 19s | `task12-check.log` |
| 3 | `cargo test -p camel-component-surrealdb --all-targets` | 0 | 8m 42s | `task12-test-component.log` |
| 4 | `cargo test -p camel-test --all-targets` | 0 | 2m 27s | `task12-test-cameltest.log` |
| 5 | `cargo test --workspace --lib` | 0 | 9m 24s | `task12-test-workspace-lib.log` |

No failure indicator in any log: no `FAILED` summary, no `panicked at`,
no `error:`/`error[...` line. (The only `panicked`/`failed` substrings in
the logs are inside *passing* test names such as
`monitor_task_handles_panicked_task ... ok`.) No compile error surfaced
from the 4.0.0 bump, so no adapter fix was needed (step 6).

### Step 3 — `camel-component-surrealdb --all-targets`

One test binary (crate has no `tests/` directory; lib unit tests only):

```
Running unittests src/lib.rs (target/debug/deps/camel_component_surrealdb-a99c97447f36bf36)
test result: ok. 211 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

Aggregate: **211 passed, 0 failed**.

### Step 4 — `camel-test --all-targets`

83 test binaries; aggregate **346 passed, 0 failed, 0 ignored**. Non-zero
binaries (51):

```
camel_test 17 | bean_integration_test 8 | bearer_token_producer_test 2
cache_admin_test 5 | cache_eip_smoke 4 | cache_resilience 4
component_emission_test 5 | controlbus_test 10 | core_catalog_real_metadata_test 2
dev_watch_proto_test 3 | dev_watch_template_test 4 | direct_top_level_test 3
do_try_test 11 | error_semantics_test 1 | exchange_uow_yaml_test 3
function_context_test 1 | function_hot_reload_test 9 | function_route_controller_test 2
gh62_language_boundary_regression_test 7 | harness_test 8 | hot_reload_integration 5
http_error_status_expression_test 1 | http_readiness_test 2 | integration_test 78
inventory_metrics_test 5 | jsonpath_test 5 | jsonpath_xpath_boolean_parity_test 1
loop_test 5 | marshal_test 3 | master_explicit_delegate_test 1
master_noop_test 2 | mcp_server_auth_test 7 | metrics_wiring_test 14
mime_split_integration 3 | on_exceptions_handled_by_test 8 | on_exceptions_wildcard_test 3
otel_direct_hop_regression 1 | otel_trace_tree_test 8 | rhai_parse_json_body_test 2
route_example_parser_sweep 29 | route_start_order_test 1 | script_readonly_rejected_test 5
script_test 8 | seda_test 6 | tar_split_integration 2
throttle_test 2 | tracer_test 3 | ws_security_test 8
xpath_test 6 | yaml_gap_test 12 | zip_split_integration 1
```

Zero-test binaries (32, `0 passed; 0 failed`): `audience_substitution_test`,
`auth_multi_credential_test`, `cache_payload_offload`, `container_test`,
`cxf_test`, `http_static_test`, `http_test`, `jms_durable_test`, `jms_test`,
`jms_tls_test`, `kafka_test`, `kernel_fail_closed_test`, `keycloak_jwks_test`,
`kubernetes_test`, `late_registration_gate_test`, `master_kubernetes_test`,
`mqtt_test`, `mtls_bridge_test`, `opensearch_test`, `redis_repositories_test`,
`redis_sentinel_test`, `redis_sentinel_tls_test`, `redis_test`,
`redis_tls_test`, `security_policy_test`, `sql_test`, `surrealdb_test`,
`xml_bridge_charset_probe_test`, `xml_bridge_validator_test`,
`xml_bridge_xerces_diagnostics_test`, `xml_bridge_xj_test`,
`xml_bridge_xslt_test`.

`surrealdb_test.rs` runs 0 tests here by design: the feature-gated
surrealdb suite is not compiled by a plain `--all-targets` run; that
coverage is Task 1.3.

### Step 5 — `cargo test --workspace --lib`

73 lib test binaries; aggregate **13,058 passed, 0 failed, 2 ignored**
(ignored: 1 in `camel_component_file`, 1 in `camel_language_js`).
Per-binary passed counts:

```
bench_loadgen 136 | camel_api 730 | camel_auth 224 | camel_bean 15
camel_bean_macros 24 | camel_bench 3 | camel_bridge 103 | camel_builder 162
camel_bundles 42 | camel_cli 620 | camel_component_api 168
camel_component_container 64 | camel_component_controlbus 26
camel_component_cron 48 | camel_component_cxf 204 | camel_component_direct 48
camel_component_exec 26 | camel_component_file 135 (1 ignored)
camel_component_grpc 217 | camel_component_http 557 | camel_component_jms 352
camel_component_kafka 183 | camel_component_keycloak 75 | camel_component_llm 380
camel_component_log 36 | camel_component_mail 262 | camel_component_mcp 49
camel_component_mock 148 | camel_component_mqtt 56
camel_component_opensearch 104 | camel_component_rabbitmq 113
camel_component_redis 748 | camel_component_seda 88 | camel_component_sql 259
camel_component_stream 36 | camel_component_surrealdb 211
camel_component_timer 27 | camel_component_validator 182
camel_component_wasm 193 | camel_component_ws 181 | camel_config 345
camel_core 1046 | camel_dataformat_protobuf 18 | camel_dsl 926
camel_endpoint 87 | camel_endpoint_macros 23 | camel_examples_tests 0
camel_function 31 | camel_health 29 | camel_integration_test 427
camel_language_api 42 | camel_language_js 167 (1 ignored)
camel_language_jsonpath 31 | camel_language_minijinja 71
camel_language_rhai 250 | camel_language_simple 267 | camel_language_xpath 43
camel_lint 248 | camel_lsp 31 | camel_master 64 | camel_matchers 27
camel_otel 86 | camel_platform_kubernetes 63 | camel_processor 943
camel_prometheus 73 | camel_proto_compiler 76 | camel_redis_repo 62
camel_template 126 | camel_test 17 | camel_wit 13 | camel_xj 33 | camel_xslt 73
xtask 85
```

### Forced changes (step 6)

None. All five commands were green with the 4.0.0 bump as committed.
`git diff c70c29cb0 -- '**/*.rs'` was empty at task-1.4 finalization;
the single STAGE 4 harness fix below superseded that (see the STAGE 4
section). At task-1.2 capture the full diff vs base contained only
`Cargo.toml`, `Cargo.lock`, and the OpenSpec change files; Task 1.4
later added `CHANGELOG.md`. Because no `.rs` file changed at the time,
`cargo fmt --check` /
`cargo clippy` are N/A for this task (STAGE 4 gates cover them). No
public surface or documented behavior changed.

## Runtime gate (Docker, server v3.1.4)

Task 1.3. Full log: `/home/shared/tmp/surrealdbadv-evidence/task13-surrealdb-test.log`.

- Docker reachability: `docker info >/dev/null && echo docker-ok` → `docker-ok`.
- Command: `cargo test -p camel-test --features integration-tests --test surrealdb_test`.
- Server fixture: `crates/camel-test/tests/support/surrealdb.rs`, image
  `surrealdb/surrealdb:v3.1.4` (unchanged pin; testcontainers, wait
  `Started web server`).
- Containment: unit `fleet-surrealdbadv`, run via
  `systemd-run --user --scope --collect -p MemoryMax=12G -p MemorySwapMax=2G
  -p TasksMax=1024 -p OOMPolicy=kill`, `TMPDIR=/home/shared/tmp`,
  `CARGO_BUILD_JOBS=6`, preamble PATH (sccache, gcc-wrapper, gnumake,
  rust-default 1.99.1, cargo-audit 0.22.1), `-j4`; foreground.
  `--collect` auto-GC'd the scope after exit; `systemctl --user
  list-units 'fleet-surrealdbadv*'` is empty afterwards.
- `pgrep -c compiled_artifa` = 0 before and after.
- Wall time: **637 s** total (10 m 33 s of it compiling `camel-test`;
  test execution 3.55 s).
- Executed tests: **26** (`running 26 tests`), failing the "0 tests" trap:
  the feature gate was genuinely enabled.
- Per-`test result:` line (single binary):

```
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.55s
```

- Result: **26 passed, 0 failed**, exit 0. Tests exercised create/select
  one/select all/update/upsert/delete/patch/query/run/relate, vector
  store + vector search, LIVE-select consumer notifications, polling
  consumer, datasource health, and the HTTP-scheme rejection guard, plus
  pre-existing `support` unit tests compiled into the same binary.
- Image provenance: the raw log contains no docker-pull/container-creation
  markers — the `surrealdb/surrealdb:v3.1.4` image was locally cached.
  The proof chain for "server v3.1.4" is the fixture source
  (`crates/camel-test/tests/support/surrealdb.rs` hardcodes
  `tag = "v3.1.4"` + `WaitFor::message_on_stdout("Started web server")`,
  unchanged since base — the `.rs` diff vs `c70c29cb0` is empty) plus
  behavioral proof: 17 of the 26 passing tests are container-backed via
  `shared_surrealdb()` (real WS I/O); a failed container start would have
  panicked the shared init and failed them.
- Classification: no 4.x drift observed — no query-shape, LIVE payload,
  or auth-handshake failure. No `.rs` file changed, no documentation-only
  drift flagged, `documented-behavior-moved` was not reported, and
  `behavior-change-needs-design` did not apply. `cargo fmt --check` /
  `clippy` are therefore N/A for this task.

## Gate provenance (design.md items 1–10)

Final `Cargo.lock` versions (at a glance): `surrealdb = 4.0.0`,
`surrealdb-core = 4.0.0`.

| # | Gate (design.md) | Run location | Result |
| --- | --- | --- | --- |
| 1 | `cargo build --workspace` | Task 1.2, row 1 of local verification | exit 0 (13m 13s) |
| 2 | `cargo test -p camel-component-surrealdb --all-targets` | Task 1.2, Step 3 | 211 passed, 0 failed |
| 3 | `cargo test -p camel-test --all-targets` | Task 1.2, Step 4 | 346 passed, 0 failed (83 binaries) |
| 4 | `cargo test -p camel-test --features integration-tests --test surrealdb_test` | Task 1.3, runtime gate | 26 passed, 0 failed (Docker, server v3.1.4) |
| 5 | `cargo test --workspace --lib` | Task 1.2 Step 5; re-run green in STAGE 4 battery | 13,058 passed, 0 failed, 2 ignored |
| 6 | `cargo fmt --check --all` | conductor STAGE 4 battery | exit 0 (first pass and after the STAGE 4 fix) |
| 7 | AGENTS.md clippy set (workspace + per-crate extras) | conductor STAGE 4 battery | exit 0 ×4 after the STAGE 4 fix (`clippy-workspace` first pass failed E0004, see below; kafka/cli/cli-flavor green first pass) |
| 8 | Full `cargo xtask lint-*` battery (24 lints; `lint-commits` skipped — conductor policy) | conductor STAGE 4 battery | exit 0 ×24 |
| 9 | `cargo audit` | Task 1.1 after audit (`audit-after.txt`); re-run in STAGE 4 battery | exit 0, 0 vulnerabilities, 7 allowed warnings |
| 10 | `cd docs && mdbook test` if `docs/**` changed | N/A — no docs changed | `doc-guide-test: N/A — no docs changed` |

Conductor ADD gates (conductor-light STAGE 4): `cargo build --workspace`
(row 1, re-verified exit 0 in the battery), `cargo test --workspace
--lib` (row 5, re-run exit 0 in the battery), and the hexagonal boundary
test `cargo test -p camel-core --test
hexagonal_architecture_boundaries_test` — exit 0 in the battery. Also
run in the battery: `cargo xtask schema --check` exit 0, and
`RUSTDOCFLAGS="-D warnings" cargo doc` on the AGENTS.md crate list,
exit 0.

`doc-guide-test: N/A — no docs changed`. Task 1.3 reported no
`documented-behavior-moved`, so
`crates/components/camel-component-surrealdb/CONTEXT.md` and
`docs/src/components/surrealdb.md` are untouched (`git diff c70c29cb0 --
crates/components/camel-component-surrealdb/CONTEXT.md docs/` is empty).
At task-1.4 time no adapter fixes had been made; the STAGE 4 battery
below records the single forced harness fix that superseded that
statement.

## STAGE 4 gate battery and the one forced fix

All conductor-owned gates ran foreground inside the contained scope
(`fleet-surrealdbadv`, same flags/PATH as above; `pgrep -c
compiled_artifa` 0 before/after; logs `stage4-{A,B,B2,CD}.log`, summary
`stage4-summary.txt` in the evidence dir).

First pass: every gate green except `clippy --workspace --all-features`
(E0004, exit 101). Root cause: surrealdb-types 4.0.0 widened two enums
that `crates/camel-integration-test/src/runner/surreal_validate.rs`
(feature `surreal`-gated — not compiled by the default-feature checks of
task 1.2) matches on exhaustively, fail-closed per kind:

- `Value` gained `Date`, `Time`, `Timestamp`, `Interval`.
- `Number` gained `I8`, `U8`, `I16`, `U16`, `I32`, `U32`, `U64`, `F16`,
  `F32`, `BigDecimal`.

**Forced-change rationale (the change's only `.rs` edit).** The mapping
preserves the file's established contract — temporals stringify
(`Datetime` precedent), structured kinds fail closed (`Duration`/
`Bytes`/… precedent), integers map through (`Int` precedent), floats
keep the finite/NaN/infinite rule (`Float` precedent):

- `Date`/`Time`/`Timestamp` → `Value::String(to_string())` (same as
  `Datetime`); a server at the fixture pin (v3.1.4) never emits them,
  so these arms are exhaustiveness-only.
- `Interval` → `Err(unsupported("interval"))` (same class as
  `Duration`).
- Integer widths → `Value::from` (serde_json supports every width).
- `F16`/`F32` → finite → `Value::from(f64)`; NaN/infinite → the same
  typed error as `Float`.
- `BigDecimal` → `Err(unsupported("big decimal"))` (same class as
  `Decimal`).

No public surface moved: the helper is `pub(crate)` inside the test
harness; the component is untouched. This supersedes the earlier "no
adapter fixes were made" statement, which recorded the pre-battery
state (tasks 1.1–1.4 and the Docker gate were already green before the
battery ran; the fix affects only the `--all-features` compile of the
harness).

Re-run after the fix (all exit 0): `cargo fmt --check --all`,
`cargo clippy -p camel-integration-test --features surreal
--all-targets -- -D warnings`, `cargo clippy --workspace
--all-features --exclude camel-cli --exclude camel-component-kafka
--exclude security-keycloak --exclude security-wasm-policy -- -D
warnings`, then the remaining battery legs `doc-build`, `hexagonal`,
`workspace --lib` — see the gate provenance table above for the final
per-gate status. Note the fix landed in TWO steps (the summary log
shows the full forcing history): pass B failed E0004 on the
temporal/`Interval` arms; after that first edit, B2's
`clippy-inttest-surreal` and `clippy-workspace` both still failed
E0004 on the ten `Number` arms; after the second edit, targeted
`clippy-inttest-surreal` and the full `clippy-workspace` leg went
green, followed by the remaining battery legs.
