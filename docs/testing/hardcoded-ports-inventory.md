# Hardcoded-port inventory: the test surface (rc-99d5.3)

Audit of every hardcoded port literal across the rust-camel test
surface, as of base `688a0d4a` (verified 2026-09-10 in the
`itest-batch` worktree). Purpose: the nextest integration-tier
expansion (epic rc-99d5) parallelizes test binaries; hardcoded
listener binds spanning binaries collide with `EADDRINUSE`. This
inventory classifies every literal by risk class and states the
migration pattern. It supersedes the stale line references in the
epic's children (see "Stale-reference corrections").

The audit's headline: **no cross-binary hardcoded-port collision
exists today**. No test binary binds a hardcoded port that any other
binary also binds: every shared-surface listener binds `:0`
(kernel-assigned), and every remaining literal is either a
container-internal dial, a config-parse assertion, or a
documented pinned-port fixture used by exactly one test in one
binary (`18221`; the examples job owns `18097`/`18221` separately).
The nextest blockers that remain are the global-state class and the
migrate-on-touch pinned fixtures below.

## Per-binary literal table (current tree)

| Binary | Literal | Sites | Class | Parallel-nextest risk |
|---|---|---|---|---|
| `camel-test/tests/keycloak_jwks_test.rs` | 8080 | 4 (3 bash `/dev/tcp` scripts + 1 base URL) | container-internal dial | none — every HTTP call runs INSIDE the keycloak container via `exec` (no published host port) |
| `camel-test/tests/cxf_test.rs` | 8080 | 2 (pool profile `address`) | dial target (client-side config) | none — the mock SOAP services this suite runs bind `127.0.0.1:0` |
| `camel-test/tests/cxf_test.rs` | 9090/9091 | 4 (multi-profile config + route URIs) | dial target | none — failure-path tests; no listener on those ports |
| `camel-test/tests/http_test.rs` | 8080 | 3 | config-parse assertion (`from_uri`, port assert, proxy URL string) | none — never reaches the network |
| `camel-test/tests/http_static_test.rs` | 8080/9090 | 2 | config-parse assertion | none |
| `camel-integration-test` src tests | 18080 | 6 (`doc_parse_test.rs`, `env_layers_test.rs`) | env/parse fixture string | none — env-layer resolution asserts, no bind |
| `camel-integration-test/tests/partner_verification_test.rs` + `tests/fixtures/retry-route.yaml` | 18221 | 3 | **pinned consumer bind** (route `from:` listener) | intra-binary: only the flagship test boots it; cross-binary: none today — migrate-on-touch |
| `examples/integration-testing/` | 18097, 18221 | fixture pair | **pinned consumer binds** | examples run as their own job; the caveat is documented in `partner-retry.routes.yaml` |

## Cross-binary collision matrix

Under parallel nextest, a collision requires two binaries binding
the same port concurrently. Current state:

- **8080**: zero binds. keycloak's 8080 lives inside the container;
  cxf's 8080/9090 are client dial configs whose servers bind `:0`.
- **18080**: parse-fixture strings only (itest env-layer tests).
- **18221**: bound by exactly one test in one binary
  (`partner_verification_test`). A second test in the SAME binary
  booting it concurrently would collide — today none does. The
  example fixtures pin the same port in a separate CI job.
- **18097**: example fixtures only.

Conclusion: the epic's acceptance gate "zero cross-binary port
collisions under parallel nextest" is already satisfied at the bind
level for the audited binaries. The expansion's residual risk lives
in the global-state class, below.

## The fix pattern (migrate-on-touch)

For any future pinned listener, the established patterns are:

- `stage_http_listener` / `stage_ws_listener`
  (`camel-test/tests/support/mod.rs`): bind `:0`, register with the
  `ServerRegistry` staging area, return the kernel-assigned port.
- itest inbound: `inbound: {bindVar: NAME}` (ADR-0070, rc-5yon) — the
  harness provisions a staged port-0 listener and route files
  interpolate `${env:NAME}`. The `18221`/`18097` pinned fixtures are
  the back-compat shape; migrate them to `inbound:` when next
  touched.
- itest harness partners: always `:0` router keys with
  `bindVar`/env-tier bound authorities (ADR-0069 sections 8-9).

## ServerRegistry::global() — the global-state taxonomy class

Port collisions are not the only cross-test contamination surface.
These process-global statics serialize or share state across tests
in one binary, which parallel nextest (one process per test by
default) neutralizes BETWEEN tests but which still matters for
in-process parallelism and for `--workspace --lib` style runs:

- `ServerRegistry::global()` — shared by `http_static_test`,
  `mcp_server_auth_test` (`McpServerRegistry::global()`), and the
  itest inbound provisioning (`crates/camel-integration-test/src/inbound.rs`).
- `SHARED_CXF_POOL` (`cxf_test.rs` OnceCell) — one bridge pool per
  process.
- itest `RUN_LOCK` (`tests/common/mod.rs`) — serializes document runs
  inside one binary because log-capture windows are process-global
  (rc-tdgh5).

Nextest's default process-per-test isolation converts these from
cross-test hazards into per-process fresh state; the epic's
integration-tier expansion should rely on that isolation rather than
on removing the statics.

## Stale-reference corrections (supersedes epic-child line refs)

- `integration_test.rs:18084` — the file is now 4642 lines and does
  not bind 8080; the per-binary files listed above are the real
  sites.
- "8080 spans 4 binaries (keycloak_jwks, cxf, http_static,
  http_test)" — only keycloak_jwks (container-internal) and cxf
  (dial configs) carry live-wire 8080; http_static and http_test are
  config-parse assertions.
- keycloak_jwks "7×8080" — current tree has 4 sites, all
  container-internal.

## Status for the epic

Inventory delivered (this document, acceptance part 1). The
zero-collision gate (part 2) holds at the bind level for the audited
surface; remaining migration work is the two pinned inbound fixture
pairs (`18221`, `18097`), which are migrate-on-touch per the epic's
own rule, and the global-state inventory above, which nextest's
process isolation addresses.
