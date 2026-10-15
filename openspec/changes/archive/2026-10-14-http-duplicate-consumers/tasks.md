# Tasks: http-duplicate-consumers

## HTTP Component

### T1: Reproduce and reject duplicate registrations

Files: crates/components/camel-http/src/registry.rs, crates/components/camel-http/src/lib.rs, crates/components/camel-http/src/lib_tests.rs, crates/components/camel-http/CONTEXT.md, crates/camel-test/tests/http_static_test.rs.

Steps:
1. Add behavioral regressions that compile against the existing infallible API. Run the new consumer tests against unmodified production code and record red output.
2. Make register_api_route and register_rest_endpoint return Result<(), CamelError>. Under their existing write locks reject occupied keys with Config errors before mutation. Error text identifies the duplicate path or method/template. Remove overwrite semantics from comments. Preserve separate-registry dispatch precedence and existing template equality.
3. Propagate registration failures with ? in HttpConsumer::start, before mark_ready and before the cleanup loop. Migrate every registration call site to handle success explicitly, including the existing camel-test HTTP/static co-hosting fixture.
4. Add registry concurrency coverage after the fallible API exists. Document unique-key rejection in camel-http CONTEXT.md.
5. Run the new tests and the complete camel-component-http suite. Request review, resolve every critical/important finding.

Tests (all in lib_tests.rs; bounded operations, staged listeners, no added sleeps):
- test_duplicate_legacy_consumer_preserves_original: stage a listener on 127.0.0.1:0; create two endpoints with identical /duplicate path and distinct route IDs. Start A with StartupSignal and await readiness within 2s. Start B with its own signal. Bound B start within 2s, cancel B on timeout so failure does not leak tasks; assert it returns CamelError::Config mentioning duplicate and /duplicate, and B readiness never succeeds. Call B stop and cancel its token. Send a real HTTP request while A pipeline receiver replies body original; assert status 200 and body original. Cancel A and join within 2s. Start a fresh consumer with identical URI and route ID A; assert readiness succeeds, request returns replacement, then cancel/join it.
- test_duplicate_method_consumer_preserves_original: same sequence with httpMethod=GET and /users/{id}; additionally start POST on the same template and await readiness. Issue GET requests to /users/42; assert duplicate error includes GET and template information, original body original, replacement body replacement. After cancelling/joining original GET and again after replacement GET starts, issue POST requests and have POST pipeline receiver reply sibling; assert status 200 and body sibling both times. Cancel/join POST at end.
- test_duplicate_registry_registration_is_atomic: for both legacy and method-aware keys on a fresh registry, create two distinct channels. Hold registry.inner.write() guard, pin both registration futures, and poll each with std::future::poll_fn, asserting both return Pending behind the held guard. Release guard, then drive both with a 2s-bounded tokio::join!. Assert exactly one Ok and one Err(Config), exactly one stored entry, and stored sender same_channel as successful sender. Assert rejected receiver gets no requests.
- Verify existing method_aware_dispatch_same_path_different_verbs, deregister_one_method_keeps_sibling_verbs and dispatch_exact_legacy_beats_rest_template tests; these cover sibling methods and mixed-registry precedence.

Acceptance: new consumer tests fail behaviorally before fix and pass after fix; concurrent registration has one winner; no existing HTTP tests regress; error propagation occurs before readiness; no new ownership structures or dependencies.

- [x] T1

### T2: Validate and prepare local commit

Files: OpenSpec artifacts and modified files from T1.

Steps:
1. Validate OpenSpec change, run cargo fmt --check --all, required clippy profiles, repository lints and schema checks in this feature worktree. Ratchets stay 471/296/7.
2. Review final diff and ignore status; commit only tracked/nonignored intended files with subject fix(http): reject duplicate consumers and body Bd: rc-om77p.
3. Record evidence in bd. Keep ticket open pending human merge/push; do not merge or push without requested review step.

Tests: all quality gates in AGENTS.md when supported; report environmental blockers and unrelated baseline failures with exact evidence. Do not run cargo commands in main.

Acceptance: targeted tests green, review clean, local commit ready for human approval; main and other worktrees untouched.

- [x] T2
