# Design: bound-bare-recv-waits

## Approach

Mechanical-with-semantics conversion of 117 inventoried bare
`rx.recv().await` sites (see `inventory.md` in this change dir; 100
single-line + 17 multi-line shapes re-derived from the lint's AST scanner
after the inter-phase-2 review caught the original line-grep blind spot)
into
deadline-bounded receives, using four family shapes. The lint's
boundedness rule (recv span inside the 2nd argument of
`tokio::time::timeout` / `timeout_at`, or a timeout call site inside a
`loop` subtree) defines "bounded"; every conversion uses one of these
shapes so the finding disappears without marker abuse.

### Family shapes (AST-verified counts — see `inventory.md`)

- **A — single receive in the test body (41 sites: 30 original + 11
  multi-line additions).**
  `let x = rx.recv().await.expect("m");` becomes
  `timeout(DEADLINE, rx.recv()).await.expect("m within Ns").expect("channel alive");`
  — the established drainclaim idiom (route_controller_drainclaim_tests.rs:266).
  Outer expect fires on `Elapsed`, inner on channel-closed. Sites holding the
  result for a `match`/`assert!` (e.g. grpc server.rs `let received =
  rx.recv().await; assert!(received.is_some());`) keep their logic but receive
  `Ok(Ok(v))` from the wrapper; `Err(_)` (elapsed) must fail with a message
  naming the wait and deadline. Discarding receives (`let _ = rx.recv().await;`)
  gain the same double-expect chain (the discard was hiding a hang).
- **B — `while let` drain directly in the test body (1 site, camel-timer).**
  Convert to the lint's canonical per-iteration-deadline loop:
  `loop { match timeout(DEADLINE, rx.recv()).await { Ok(Ok(env)) => { /*body*/ }, Ok(Err(_)) => break, Err(_) => panic!("recv stalled past Ns") } }`.
  `break` fires only on channel-close (the drain's legitimate end);
  `Elapsed` panics — a silent `break` on `Elapsed` would turn a hang into a
  false pass. The `panic!` must name the channel and deadline.
- **D — receive inside a spawned background task (75 sites: 69 + 6
  multi-line additions).**
  These tasks are pipeline simulators, drainers, and gated responders; they
  legitimately run for the whole test. An overall timeout would kill a healthy
  drainer mid-test, so the bound is **per-iteration**:
  `tokio::spawn(async move { loop { match timeout(DEADLINE, rx.recv()).await { Ok(Ok(env)) => { /*original body*/ }, Ok(Err(_)) => break, Err(_) => break /* stalled: stop draining */ } } });`
  Rationale: the drainer is infrastructure, not assertion — its stall cannot be
  allowed to hang it forever, and test-visible failure comes from the test
  body's own (now-bounded) assertions on effects. Inside the task, `Err(_) =>
  break` (not `panic!`) is correct: a panic in a never-joined task is
  swallowed, and a panic in a joined task would only fire after the stall —
  same information, more machinery.
  **Exception — expects that carry test-alive semantics inside the task**
  (e.g. grpc integration.rs:1753 `release_rx.recv().await.expect("test alive")`):
  keep the expect chain, wrap it: `timeout(DEADLINE, release_rx.recv()).await.expect("release gate within Ns").expect("gate channel alive");`
  so a stalled gate still fails the task (and propagates if the handle is
  joined). These sites are enumerated in the task blocks.

### Deadline policy

- Default `Duration::from_secs(2)` (drainclaim precedent).
- `from_secs(5)` where the same test already waits multi-second on
  container-backed I/O (kafka broker, sql DB, grpc listener) — deadline
  must exceed the slowest legitimate phase of the receive.
- Never below 500ms; no virtual-time tricks (tests are wall-clock).

### Ratchet update

Single update in the final task, after every site is bounded:
`scripts/xtask/ratchet-unbounded-wait.max`: 548 → 431 (117 sites; minus any
adjudicated via marker — markers also leave the unadjudicated count).
`lint-unbounded-wait` must exit green at 431. Intermediate commits keep
548 (a ceiling — counts only drop as edits land).

## Affected crates

- camel-core: 11 sites, family A (lifecycle adapter tests)
- camel-component-grpc: 27 (server.rs unit tests: A incl. match-shape;
  integration.rs/server_auth_test.rs: mostly D pipeline simulators +
  the "test alive" gated responder)
- camel-sql: 14, all D (pipeline simulators); camel-kafka: 9 (D + A)
- camel-component-seda: 8 (D), camel-http: 6 (D incl. `while is_some`
  drains), camel-direct: 6 (D), camel-master: 3 (A)
- camel-processor: 3 (A, discarding receives), camel-component-api: 3 (A)
- camel-component-mcp: 3 (server_consumer A; server_tool_dispatch 2 D),
  camel-component-wasm: 2 (D), camel-test: 2 (both D: http_static while-let
  inside a spawned task; mcp_server_auth expect inside a task), camel-ws: 1
  (D), camel-timer: 1 (B), camel-dsl: 1 (D)
- scripts/xtask: ratchet file only (no code)

## Architecture boundaries

Test-function bodies only, inside `#[cfg(test)]` modules or `tests/`
directories — zero data-plane or control-plane code changes, so the
Runtime/DSL/Components boundary is untouched. The change enforces
ADR-0069 §13.2 R1 (no unbounded waits; receive-with-timeout at
transport boundaries) and ADR-0064's tiering: these are integration-tier
assertions at channel boundaries. The ratchet is the
`lint-unbounded-wait` enforcement hook (ADR-0069 §13.2 R1; monotone
ceiling per `lint-test-sleep` mirror).

## Phases

### Phase 1: core test idiom establishment
- **Goal:** bound camel-core (11), camel-processor (3),
  camel-component-api (3) — 17 sites, 5 files, family A.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** commits per file; affected tests green.
- **Exit-criteria:** zero recv findings in these 5 files (analyzer);
  `cargo test -p camel-core -p camel-processor -p camel-component-api`
  lib targets pass.

### Phase 2: grpc cluster (families A/D)
- **Goal:** bound camel-component-grpc 27 sites — the densest D cluster
  (pipeline simulators, drainers, the gated "test alive" responder).
- **Dependencies:** Phase 1 idioms.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** bounded drainers; integration tests compile + unit
  tests pass (container-backed integration paths compile-verified;
  runtime verification deferred to CI where Docker-gated).
- **Exit-criteria:** zero recv findings in grpc files; unit tests green;
  `cargo clippy -p camel-component-grpc --all-targets -- -D warnings`.

### Phase 3: container-backed I/O components
- **Goal:** bound camel-sql (14), camel-kafka (9) — 23 sites.
- **Dependencies:** Phase 1 idioms; 5s deadline policy for
  container-backed paths.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** commits per file.
- **Exit-criteria:** zero recv findings; sql/kafka unit tests green
  (Docker-gated integration deferred to CI).

### Phase 4: remaining components + ratchet closeout
- **Goal:** bound seda (8), http (6), direct (6), master (5), mcp (13:
  consumer 3, tool_dispatch 2, consumer_e2e 5, dsl_e2e 2 + dsl rest_stream 1),
  wasm (2), ws (1), timer (1), camel-test (2) — 33 + 17 - 6 = 44 sites in
  tasks 4.1-4.3 net of task 2.4's six; exact per-file lists in tasks.md;
  then ratchet 548 → 431 and full gate run.
- **Dependencies:** Phases 1-3 (ratchet delta assumes all prior sites
  bounded).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** ratchet commit; `lint-unbounded-wait` green at 431.
- **Exit-criteria:** workspace recv-finding count = 0 (scanner over
  all 27 files); ratchet = 431; gates green.

## Alternatives considered

- **`rx.recv_timeout(d)` / `try_recv` polling loop:** rejected as
  default — `recv_timeout` is not a tokio mpsc method (broadcast only),
  and try_recv polling reimplements the timeout the runtime already
  provides; kept as an option only where a site genuinely needs
  poll-now semantics (none inventoried).
- **Marker everything `allow-test-wait`:** rejected — the marker is an
  adjudication escape for semantically-unbounded waits, not a bulk
  device; bulk-marking would hollow out the ratchet (ADR-0069 R1).
- **A shared `recv_bounded` test helper:** deferred — 100 call sites in
  16 crates would each need the helper re-exported; the raw timeout
  idiom is already canonical in-repo. Introducing new shared test
  surface belongs to a separate proposal if repetition proves painful.

## Self-grill record

**Questions generated:**
1. [glossary] "Per-iteration deadline" vs "overall timeout" — does the lint
   accept the per-iteration shape as bounded, and is a `while let` drain
   convertible to it without an `ExprLoop` rewrite?
2. [sharpen] "Silent break on stall inside a JOINED task could mask a stall"
   — is the design's break-not-panic rule for D safe when the handle IS
   joined, or does a joined drainer need a stricter rule?
3. [scenario] Site 45 (integration.rs:1753) sits INSIDE the site-44 drain
   loop (1746). Both are recv findings. Does bounding the outer `while let`
   with a per-iteration timeout accidentally satisfy the inner recv, or must
   both be converted independently?
4. [cross-ref] Does the 2s/5s deadline policy actually exceed the slowest
   legitimate receive in the container-backed paths it targets?

**Answers (with citations):**
1. The lint marks a `while let` drain bounded ONLY if a `timeout` call site
   lies in scope; for `ExprWhile` there is no loop-subsumption, so the recv
   MUST be individually wrapped: `while let Some(x) = timeout(d, rx.recv())
   .await …` does not type-check (yields Result), so the canonical shape is
   `loop { match timeout(d, rx.recv()).await { … } }`. Design.md B/D shapes
   use exactly this. The per-iteration `timeout` inside a `loop` is bounded
   via `TimeoutSeeker` (`lint_unbounded_wait.rs:569-578, 660-668`). Correct.
2. Safe with one caveat now made explicit. For a JOINED drainer, `Err(_)=>
   break` ends the task cleanly; the join returns `Ok(())`, and the stall
   surfaces through the test body's own bounded assertion on the missing
   effect (which now times out and panics). A `panic!` in the drainer would
   fire only AFTER the same stall and carry no extra information — same
   signal, more machinery (design.md D rationale). The stricter rule is NOT
   needed BECAUSE every test that joins a drainer also asserts on the
   drainer's downstream effect under a bounded receive; the D shape never
   stands alone as the sole liveness check. This invariant is the load-
   bearing assumption and must hold per site. (design.md:36-41)
3. Independent conversions. 1746 (`ExprWhile` drain) and 1753 (nested single
   recv with `.expect("test alive")`) are two separate `visit_expr_await`
   findings (`lint_unbounded_wait.rs:683`). Wrapping the outer loop's recv
   does not bound the inner one — the inner `release_rx.recv().await` is a
   distinct await span outside any `timeout` future-arg region. Design.md
   correctly enumerates 1753 as the expect-preservation exception (wrap +
   keep expect chain). Verified against live source (integration.rs:1746-1753).
4. Yes. Default 2s matches the drainclaim precedent
   (route_controller_drainclaim_tests.rs:266); 5s applies where the same
   test already blocks multi-second on broker/DB/listener I/O. Both exceed
   the slowest legitimate receive phase; 500ms floor prevents flake from
   sub-second scheduling jitter. (design.md:48-54)

**Outcome:** confirm
**Self-grill mode:** self-grill-proposals skill
