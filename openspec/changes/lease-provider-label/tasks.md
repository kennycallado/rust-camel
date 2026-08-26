# Tasks: lease-provider-label

## camel-platform-kubernetes

### Task 1.1: Provider label on first-time Lease creation

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)

**Steps:**
1. In `platform_service.rs`, extract the inline first-time Lease construction from the `let Some(mut lease) = maybe_lease else` arm of `reconcile_lease` (~lines 605-627) into a pure function placed above `reconcile_lease`:
   `fn build_first_time_lease(lease_name: &str, holder_identity: &str, config: &KubernetesPlatformConfig, now: JiffTimestamp) -> Lease`
   The function body is the existing construction verbatim, with two changes: (a) it gains a `let mut labels = BTreeMap::new(); labels.insert("provider".to_string(), "camel".to_string());` block mirroring the existing annotations block, and (b) the `ObjectMeta` gains `labels: Some(labels)` between `name` and `annotations`. `JiffTimestamp` is `Copy` — pass `now` by value; the body moves verbatim with `MicroTime(now)` at both acquire_time and renew_time.
2. Replace the inline construction in the create arm with `let lease = build_first_time_lease(lease_name, holder_identity, config, now);`.
3. Add a unit test in the in-file `#[cfg(test)]` module (near the existing config/conflict tests):
   `#[test] fn first_time_lease_carries_provider_label_and_term_annotation()` — construct `KubernetesPlatformConfig::default()`, call `build_first_time_lease("test-lock", "ns/node-a", &config, JiffTimestamp::now())`, and assert: `lease.metadata.name == Some("test-lock".to_string())`; `lease.metadata.labels` equals `Some(BTreeMap from [("provider","camel")])`; `lease.metadata.annotations` equals `Some(BTreeMap from [("camel.io/leader-term","1")])` (use the `LEADER_TERM_ANNOTATION` constant — the test module imports it via `use super::*`; also `KubernetesPlatformConfig::default()` suffices for the config); `lease.spec` holder_identity = `Some("ns/node-a")` and `lease_duration_seconds` = `Some(config.lease_duration.as_secs() as i32)`.

**Tests:** (executable spec)
- `first_time_lease_carries_provider_label_and_term_annotation`: setup = pure helper exists with config default; action = build the lease for lock `test-lock`, holder `ns/node-a`; assert = exact metadata triple (name, labels `provider=camel`, leader-term `1`) and spec holder/duration as listed above.
- `command`: `cargo test -p camel-platform-kubernetes first_time_lease` — **expected**: fails before step 1 (helper absent), passes after.
- `command`: `cargo test -p camel-platform-kubernetes` — **expected**: all existing tests still pass (renew/takeover paths untouched: in-place mutation + `replace` round-trips fetched labels).

**Acceptance:**
- `rg -n "build_first_time_lease" crates/platforms/camel-platform-kubernetes/src/platform_service.rs` shows exactly one definition, one non-test call site in `reconcile_lease`, and the test's invocation (3 hits total).
- The created Lease asserts green under `cargo test -p camel-platform-kubernetes` (exit 0).
- `cargo fmt --check --all` and `cargo clippy -p camel-platform-kubernetes --all-targets -- -D warnings` exit 0.

- [x] 1.1
