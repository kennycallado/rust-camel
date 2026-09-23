# Tasks: drainscope

## camel-component-grpc (tests)

### Task 1.1: Convert start_tls_consumer drain to per-iteration deadline (D-recipe)

**Files:**
- `crates/components/camel-component-grpc/tests/integration.rs` (modified)

**Steps:**
1. Locate the `pipeline_task` binding inside `async fn start_tls_consumer`
   (search `let pipeline_task = tokio::spawn(async move {`; the drain is
   the `while let Some(envelope) = route_rx.recv().await` loop, ~lines
   1292-1303 — do not touch the `consumer_task` spawn above it).
2. Replace the `while let` drain with the per-iteration deadline form
   (mpsc `recv()` yields `Option<T>`; `timeout` wraps it into
   `Result<Option<T>, Elapsed>`):

   ```rust
   let pipeline_task = tokio::spawn(async move {
       loop {
           match timeout(Duration::from_secs(2), route_rx.recv()).await {
               Ok(Some(envelope)) => {
                   let name = match &envelope.exchange.input.body {
                       Body::Json(v) => v["name"].as_str().unwrap_or("World").to_string(),
                       _ => "World".to_string(),
                   };
                   let resp = Exchange::new(Message::new(Body::Json(
                       serde_json::json!({"message": format!("Hello {name}")}),
                   )));
                   let _ = envelope.reply_tx.unwrap().send(Ok(resp));
               }
               Ok(None) => break,      // channel closed: drain complete
               Err(_elapsed) => break, // stalled producer: end the drain
           }
       }
   });
   ```

   Keep the loop body byte-identical to the current `while let` body
   (same `name` match, same `resp` construction, same
   `envelope.reply_tx.unwrap().send(Ok(resp))`). Use the file's existing
   imported bare names `timeout` and `Duration` (same as the test bodies
   at :262 etc.) — do not introduce fully-qualified paths or new imports.
3. Run `cargo fmt --check` from the worktree root; if the rewrite
   introduced formatting drift, run `cargo fmt` on the file.
4. Run `cargo clippy -p camel-component-grpc --all-targets -- -D warnings`
   from the worktree root; fix any finding the rewrite introduced.

**Tests:** (existing suite is the contract — no new test fns; the
converted helper is exercised end-to-end by all five callers: the TLS
trio plus the two reload-handler tests, which drive the pure-idle
break path)
- `inbound_tls_handshake_real`: helper started with server-auth config →
  tonic client performs TLS RPC → assertion on Hello reply passes with
  the bounded drain in place. Command:
  `cargo test -p camel-component-grpc --test integration -- tls` —
  expected pass before and after (behavior-preserving for healthy runs).
- `inbound_mtls_handshake_real`: helper started with mTLS config →
  mutual-TLS RPC roundtrip → reply assertion passes. Same command
  (covered by the `-- tls` filter).
- `inbound_mtls_rejects_certless_client`: helper started, certless
  client rejected → pipeline receives no envelope, drain idles → the
  2 s per-iteration deadline ends the loop instead of parking the task
  forever; test's own assertions on the rejected handshake pass. Same
  command.
- `grpc_tls_server_registers_reload_handler` /
  `grpc_mtls_server_registers_reload_handler`: helpers started, zero
  RPCs → drain runs the pure-idle path and exits via the deadline or
  channel-close at teardown; reload-handler assertions pass. Same
  command (the `-- tls` filter selects all six tls-named tests — the
  five helper callers plus `outbound_tls_handshake_roundtrip`; the
  plaintext reload test does not match and does not use the helper).

**Acceptance:**
- `cargo test -p camel-component-grpc --test integration -- tls`
  exits 0 (all six tls-named tests green, covering every caller of the
  converted helper).
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings`
  exits 0.
- `cargo fmt --check` exits 0.
- No `while let` remains on `route_rx.recv()` in the file
  (`grep -n 'while let Some(envelope) = route_rx.recv'` returns nothing).

- [x] 1.1

## follow-up tracking

### Task 1.2: File scope-widening follow-up bd; verify ratchet unchanged

**Files:**
- (no repo files — bd issue + gate verification only)

**Steps:**
1. From the REPO ROOT (`/home/kenny/dev/rust-camel`), create the
   follow-up issue capturing deferred path (a) — contract text inlined
   verbatim (single-quoted, shell-safe):

   ```bash
   bd create 'Widen lint-unbounded-wait to helper fns under tests/ dirs' \
     --description='Widen lint-unbounded-wait scan scope to non-test helper fn bodies in files under tests/ directories. Binding acceptance criteria (design.md of archived change drainscope): (1) AST-derived full inventory of every newly-visible helper-fn site — the widened scanner is the source of truth; the lexical seed (30 candidates, 27 unenclosed, 18 files) is Appendix A of drainscope design.md. (2) Per-site adjudication: bounded in-tree (timeout / per-iteration deadline), allow-test-wait marker with site-specific justification, or ratchet-ceiling entry — no silent sites. (3) Explicit decision on spawned-closure traversal inside helper fns (spawn-closure bodies stay pruned in test bodies today; binding-indirection kin tracked in rc-eow0s). (4) Lint scope unit tests: helper fn visible, test fn findings unchanged, ceiling monotone — may not increase without review justification. (5) Spec delta lands with enforcement: unbounded-wait-bounding requirement text extended to helper fns under tests/ so spec scope and ratchet scope move together.' \
     -t task -p 3 --deps discovered-from:rc-j27pc --json
   ```
2. Capture the create output and derive the new id: run the fenced
   `bd create` command from step 1 with its stdout captured into
   `CREATE_JSON`, then
   `NEW_ID=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' <<<"$CREATE_JSON")`,
   then `printf '%s\n' "$CREATE_JSON"` to record the filed issue.
3. From the worktree root, run `cargo xtask lint-unbounded-wait`; it
   must exit 0 at ceiling 393 (the converted site is spawn-closure
   scope, invisible before and after — count unchanged).

**Tests:**
- `ratchet-unchanged`: worktree at task 1.1 complete → run
  `cargo xtask lint-unbounded-wait` → exit code 0, reported count 393
  (no ceiling movement, no new findings).
- `followup-bd-exists`: after steps 1-2 → run
  `bd show "$NEW_ID" --json` from repo root → JSON shows status open,
  priority 3, issue_type task, dependency discovered-from rc-j27pc.

**Acceptance:**
- `cargo xtask lint-unbounded-wait` exits 0 (ceiling 393).
- `bd show "$NEW_ID" --json` succeeds and shows the discovered-from
  dependency on rc-j27pc.
- The bd description contains all five numbered contract criteria.

- [x] 1.2
