# Tasks: k8s-lease-holder-identity

## camel-platform-kubernetes

### Task 1.1: Strict identity resolution with injectable resolver seam

**Files:**
- `Cargo.toml` (modified — workspace dependencies)
- `crates/platforms/camel-platform-kubernetes/Cargo.toml` (modified)
- `crates/platforms/camel-platform-kubernetes/src/identity.rs` (modified)

**Steps:**
1. In the root `Cargo.toml` `[workspace.dependencies]` table, add
   `hostname = "0.4"`. In
   `crates/platforms/camel-platform-kubernetes/Cargo.toml`
   `[dependencies]`, add `hostname.workspace = true`.
2. In `identity.rs`, add a private enum
   `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum IdentitySource { PodName, HostnameEnv, LocalHostname }`.
3. Add a private resolver with an injectable seam:
   `fn resolve_node_identity(pod_name: Option<&str>, hostname_env: Option<&str>, local_hostname: Option<&str>) -> Result<(String, IdentitySource), PlatformError>`
   — returns the first `Some` non-empty trimmed value in the order
   PodName → HostnameEnv → LocalHostname paired with its source; when every
   source is empty or absent, returns
   `Err(unresolvable_identity_error())` (the production error helper from
   step 4).
4. Add `pub fn try_from_env() -> Result<Self, PlatformError>` on
   `KubernetesPlatformIdentity`: read `std::env::var("POD_NAME")`,
   `std::env::var("HOSTNAME")`, and `hostname::get()` (mapped to a `String`,
   failure treated as absent) and feed the resolver. Add
   `fn unresolvable_identity_error() -> PlatformError` returning
   `PlatformError::Config("cannot resolve node identity: POD_NAME, HOSTNAME, and local hostname are all empty".into())`.
   Namespace/node_name/service_account still read from
   `POD_NAMESPACE`/`POD_NODE_NAME`/`POD_SERVICE_ACCOUNT` with empty-string
   defaults as today.
5. Add `fn fallback_warning(source: IdentitySource) -> Option<&'static str>`
   returning `None` for `PodName`, and for `HostnameEnv`/`LocalHostname` a
   message naming the fallback source and stating that `POD_NAME` via the
   Downward API is the authoritative source. `try_from_env` emits it with
   `tracing::warn!` after successful resolution.
6. Leave the existing `from_env` body unchanged (legacy infallible
   behavior). Its `#[deprecated]` attribute and the migration of internal
   callers happen in Task 1.3, after the last library caller is replaced, so
   the clippy `-D warnings` gate of this task stays green.

**Tests:** (in the existing `#[cfg(test)] mod tests` in `identity.rs`, calling
the private resolver and helpers directly — no environment mutation)
- `resolver_prefers_pod_name`: given `Some("my-pod")`, `Some("my-host")`,
  `Some("local")` → `Ok(("my-pod".to_string(), IdentitySource::PodName))`.
- `resolver_falls_back_to_hostname_env`: given `None`, `Some("my-host")`,
  `Some("local")` → `Ok(("my-host".to_string(), IdentitySource::HostnameEnv))`.
- `resolver_falls_back_to_local_hostname`: given `None`, `None`,
  `Some("local")` → `Ok(("local".to_string(), IdentitySource::LocalHostname))`.
- `resolver_ignores_empty_strings`: given `Some("")`, `Some("")`, `Some("")` →
  `Err` produced by `unresolvable_identity_error()`.
- `resolver_trims_whitespace`: given `Some("  pod  ")`, `None`, `None` →
  `Ok(("pod".to_string(), IdentitySource::PodName))`.
- `unresolvable_error_names_all_sources`: the `Err` from
  `resolve_node_identity(None, None, None)` carries a message containing
  `POD_NAME`, `HOSTNAME`, and `local hostname` (asserted against the
  production helper's output, not reconstructed text).
- `fallback_warning_only_for_fallback_sources`:
  `fallback_warning(IdentitySource::PodName)` is `None`;
  `fallback_warning(IdentitySource::HostnameEnv)` message contains `HOSTNAME`;
  `fallback_warning(IdentitySource::LocalHostname)` message contains
  `local hostname`.

Command: `cargo test -p camel-platform-kubernetes --lib identity::` —
expected: red before implementation (helpers do not exist → compile failure),
green after.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib identity` exits 0.
- `cargo clippy -p camel-platform-kubernetes -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Canonical namespace, namespaced holder, empty-node rejection

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)

**Steps:**
1. Add `fn canonical_namespace(config: &KubernetesPlatformConfig, identity: &PlatformIdentity) -> String`:
   return the first non-empty of `config.namespace`, `identity.namespace`
   (treated as absent when `None` or empty), then `"default".to_string()`.
   Delete `resolve_lease_namespace` and replace its call site inside the
   spawned leadership task with the canonical namespace stored on the service
   (step 3).
2. Add `fn holder_identity_string(namespace: &str, node_id: &str) -> Result<String, PlatformError>`:
   error with `PlatformError::Config` when `node_id.trim()` is empty — message
   `format!("node_id must not be empty: a node without identity must not compete for leadership (namespace: {namespace})")`
   — else return `Ok(format!("{namespace}/{node_id}"))`. This is the single
   empty-identity gate for the whole service.
3. In `KubernetesLeadershipService::new`, after existing timing validation and
   as the FIRST identity-touching step: compute
   `let namespace = canonical_namespace(&config, identity);` and
   `let holder_identity = holder_identity_string(&namespace, &identity.node_id)?;`
   (`new` already returns `Result<Self, PlatformError>`), and store both as
   fields `namespace: String` and `holder_identity: String` on the service. In
   the `start()` spawned task, replace
   `let holder_identity = self.identity.node_id.clone();` and the
   `resolve_lease_namespace(&config, &self.identity)` call with clones of the
   stored fields.
4. Extract `fn holder_matches(spec: &LeaseSpec, holder: &str) -> bool` with
   body `spec.holder_identity.as_deref() == Some(holder)` and use it in
   `reconcile_lease` (the `is_ours` computation) and in the
   `release_lease` ownership guard, replacing both inline comparisons.

**Tests:** (in the `#[cfg(test)]` module of `platform_service.rs`, pure — no
network)
- `canonical_namespace_prefers_config`: config namespace `"prod"`, identity
  namespace `Some("other")` → `"prod"`.
- `canonical_namespace_uses_identity_when_config_empty`: config namespace
  `""`, identity namespace `Some("staging")` → `"staging"`.
- `canonical_namespace_defaults`: config `""`, identity `None` → `"default"`.
- `holder_identity_string_formats_namespaced`:
  `holder_identity_string("prod", "my-pod")` → `Ok("prod/my-pod")`.
- `holder_identity_string_rejects_empty_node_id` (this is THE empty-identity
  gate — `new` calls it first, so constructor rejection follows from it):
  `holder_identity_string("default", "")` and
  `holder_identity_string("default", "   ")` → `Err(PlatformError::Config(..))`
  whose message contains `must not compete for leadership`.
- `holder_matches_round_trip`: build a `LeaseSpec` with
  `holder_identity: Some("default/pod-a")`; `holder_matches(&spec,
  "default/pod-a")` is `true`; `holder_matches(&spec, "default/pod-b")` is
  `false`; `holder_matches(&spec, "pod-a")` is `false`; a spec with
  `holder_identity: Some("")` matches only `""` (documents why construction
  rejects empty identities).
- `holder_matches_none_never_matches`: `LeaseSpec` with
  `holder_identity: None` → `false` for every holder string.

Command: `cargo test -p camel-platform-kubernetes --lib platform_service::` —
expected: red before implementation (helpers do not exist → compile failure),
green after.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib platform_service` exits 0.
- `cargo clippy -p camel-platform-kubernetes -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: try_default constructs identity first; deprecate from_env

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)
- `crates/platforms/camel-platform-kubernetes/src/identity.rs` (modified)

**Steps:**
1. In `try_default`, move identity construction BEFORE the rustls provider
   install and `Client::try_default()` — immediately after `config.validate()?`
   — so a missing identity surfaces as `PlatformError::Config` and is never
   masked by a client-availability failure:
   `let identity = KubernetesPlatformIdentity::try_from_env()
   .map_err(|err| { tracing::error!(error = %err, "kubernetes identity resolution failed"); err })?
   .into_platform_identity();`
   then the existing rustls setup, client construction, and
   `KubernetesLeadershipService::new(client, identity.clone(), config)` remain
   in their current order (the binding stays immutable — namespace
   normalization lives inside `new` from Task 1.2, which stores the canonical
   namespace and holder once for every construction path).
2. Now that no library code calls `from_env`, mark it
   `#[deprecated(since = "0.35.0", note = "use try_from_env; from_env silently produces an empty node id")]`
   in `identity.rs`.
3. The existing unit test in `identity.rs` (~lines 174-180) that calls
   `KubernetesPlatformIdentity::from_env()` would trip the deprecation lint:
   annotate that test function with `#[allow(deprecated)]` so the legacy
   infallible behavior stays documented and the lint stays clean.
4. Verify the example binary that constructs `KubernetesLeadershipService`
   directly — `examples/kubernetes-platform` (package
   `camel-platform-kubernetes-example`), two `new()` call sites in
   `src/main.rs` (lines ~43 and ~48) — still compiles: its identities use
   non-empty node ids, so no code change is expected; do not edit unless
   compilation fails.

**Tests:**
- No new tests: resolution and normalization coverage lives in Tasks 1.1 and
  1.2. This task is wiring plus deprecation; its acceptance is compile/lint
  based. `try_default`'s strict-identity path is exercised end-to-end by the
  k3s suite in Task 1.4; the ordering change (identity before client) is
  verified by the acceptance commands below.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib` exits 0.
- `cargo clippy -p camel-platform-kubernetes -- -D warnings` exits 0.
- `cargo check -p camel-platform-kubernetes-example` exits 0.
- `rg -w 'from_env\(\)' crates/ examples/` returns matches ONLY in the
  deprecated definition and its `#[allow(deprecated)]` legacy test in
  `identity.rs` (`-w` excludes `try_from_env`).

- [x] 1.3

## camel-test

### Task 1.4: Update k3s integration expectations to namespaced holders

**Files:**
- `crates/camel-test/tests/master_kubernetes_test.rs` (modified)

**Steps:**
1. Update `wait_for_leader(&client, "camel-orders", "test-pod", 30)` to expect
   `"default/test-pod"` (identity `PlatformIdentity::local("test-pod")`; the
   test's `KubernetesPlatformConfig` sets `namespace: "default".to_string()`,
   so the canonical namespace is `"default"`).
2. Update
   `wait_for_leader(&client, "camel-config-orders", "config-test-pod", 30)` to
   expect `"default/config-test-pod"`.
3. Do not change lease names, timings, or assertions unrelated to the holder
   string.

**Tests:**
- `master_component_processes_after_kubernetes_leadership`: existing k3s test —
  setup k3s cluster, act run master route, assert leader becomes
  `default/test-pod` and exchanges flow. Requires Docker + testcontainers;
  when unavailable locally, record
  `integration-verification-deferred-to-CI`.
- `master_route_uses_kubernetes_platform_from_config`: the second
  `wait_for_leader` caller in the same file — same deferral rule.

Command: `cargo check -p camel-test --features integration-tests --test master_kubernetes_test`
(the file is gated by `#![cfg(feature = "integration-tests")]` at line 6 —
without the feature the check compiles an empty crate and proves nothing) —
expected: red before the holder-string edits would only surface at runtime;
this task's compile gate is green after the edits, runtime verification
deferred to CI when Docker is unavailable.

**Acceptance:**
- `cargo check -p camel-test --features integration-tests --test master_kubernetes_test` exits 0.
- `rg '"test-pod"|"config-test-pod"' crates/camel-test/tests/master_kubernetes_test.rs`
  shows the bare names only in `PlatformIdentity::local(...)` constructions,
  never as `wait_for_leader` holder arguments.

- [x] 1.4

## docs

### Task 1.5: Document identity requirements, holder format, migration behavior

**Files:**
- `docs/src/components/master.md` (modified)

**Steps:**
1. Extend the "Leadership backends" section with a "Kubernetes identity"
   subsection stating: production deployments MUST expose `POD_NAME` via the
   Kubernetes Downward API (`POD_NAMESPACE` recommended); resolution falls back
   to `HOSTNAME` then the local hostname with a logged warning, and platform
   construction fails when no source resolves.
2. Document the holder format: the Lease `holderIdentity` is
   `<namespace>/<node_id>`, the value operators see in `kubectl get lease`.
3. Document the migration behavior: the first post-upgrade acquisition
   rewrites each Lease's holder; the format change does not bypass lease
   expiry or optimistic concurrency.

**Tests:**
- `docs_mention_identity_contract`: machine-checkable assertions — each of
  these commands exits 0 (term present in the doc):
  `rg -qF 'Downward API' docs/src/components/master.md`;
  `rg -qF 'POD_NAME' docs/src/components/master.md`;
  `rg -qF 'HOSTNAME' docs/src/components/master.md`;
  `rg -qF 'local hostname' docs/src/components/master.md`;
  `rg -qF 'holderIdentity' docs/src/components/master.md`;
  `rg -qF '<namespace>/<node_id>' docs/src/components/master.md`;
  `rg -qF 'first post-upgrade acquisition' docs/src/components/master.md`;
  `rg -qF 'lease expiry' docs/src/components/master.md`;
  `rg -qF 'optimistic concurrency' docs/src/components/master.md`.
  Expected: red (exit 1) before the doc edits, green (exit 0) after.

**Acceptance:**
- The `rg` command in the test spec returns hits for `Downward API`,
  `POD_NAME`, and `holderIdentity`.
- `cargo xtask lint-context-citations` exits 0 (no new citation violations in
  the edited doc).

- [x] 1.5
