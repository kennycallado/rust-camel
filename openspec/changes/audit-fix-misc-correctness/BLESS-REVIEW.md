# Bless Review: audit-fix-misc-correctness

**Artifact hash (verified):** `18e72d5590b0d44230e9ef6127f841e896c567a0af1f6a73dfa6da603157c18b` ✅ matches.
**Reviewer:** senior Rust architect, self-grill gate.
**Verdict:** **BLESS-WITH-FIXES** — 2 fatal design errors, 1 false premise, 3 gaps. All fixable in artifacts; re-bless after edits.

---

## Per-issue findings

### rc-3smd — camel-log truncation — REFINE (semantic drift)

**Cross-ref:** `camel-log/src/lib.rs:356-359`. Current code guards `body_str.len() > limit`
(**bytes**) then `truncate(limit)` (**bytes**). Existing test `test_log_truncates_large_body`
(`lib.rs:633`) asserts `body_part.len() <= 10` — a **byte** assertion. So today `max_chars` is
semantically a **byte** cap, despite the name.

**Problem:** design.md:21 proposes `chars().take(limit)` (a **char** cap). That is a silent
**semantic change** from bytes→chars. The design text itself waffles across three candidate
approaches (lines 16-21) before landing on chars — this ambiguity must not survive into planning.

**Required fix:**
1. Make an explicit decision: `max_chars` means **characters** (rename intent stays, honor the
   `Chars` name). State it once in design; delete the `get(..limit)` byte-prefix detours.
2. Note the collateral: the guard `body_str.len() > limit` must become a **char-count** guard
   (`body_str.chars().count() > limit`), else a 4-char/8-byte string wrongly enters the truncation
   branch. The design omits the guard change entirely — **gap**.
3. Update existing test `test_log_truncates_large_body`: with char semantics the assertion should be
   `body_part.chars().count() <= 10`, not `.len()`. The spec must call out that this pre-existing
   test changes, or the "all existing tests pass unchanged" spirit is violated.

Approach otherwise sound. `chars().take(n).collect()` is correct and simpler than
`floor_char_boundary` (which is still unstable on the workspace MSRV — confirm before citing it even
as an alternative).

---

### rc-exa2 — SEDA concurrent forwarders — **FATAL (design is technically wrong)**

**Cross-ref:** `camel-component-seda/src/lib.rs:10` → `use std::sync::{Arc, Mutex}`. The `Single`
receiver is `Mutex<Option<mpsc::Receiver>>` (`lib.rs:373`) and is `take()`n into ONE owned task
(`lib.rs:604-627`).

**Two defects in the proposed fix (design.md:29-33):**
1. **Won't compile.** Design says wrap in `Arc<Mutex<>>` and loop on `receiver.lock().recv()`. The
   `Mutex` in scope is **`std::sync::Mutex`**. Its `MutexGuard` is not `Send`; holding it across
   `.recv().await` inside `tokio::spawn` fails the `Send` bound. The design does not specify
   switching to `tokio::sync::Mutex`.
2. **Even fixed, it defeats concurrency.** With `tokio::sync::Mutex`, `receiver.lock().recv().await`
   holds the lock across the await, **serializing** all N forwarders — the opposite of the claimed
   "work-stealing concurrency: when one forwarder is blocked on `send_and_wait`, others dequeue."
   To actually get parallelism you must lock **only** for the `recv()` and **drop the guard before**
   `forward_envelope().await`, OR switch the channel to an MPMC receiver (e.g. `async-channel`, which
   is clonable and needs no shared mutex).

**Required fix:** rewrite the design section to specify the real mechanism. Recommended: lock a
`tokio::sync::Mutex<Receiver>` only to pull one envelope, drop the guard, then await
`forward_envelope`. Document the ordering consequence (concurrent InOut means completion order is not
enqueue order — spec scenario "InOut process in parallel" already tolerates this, good). Note ADR-0007
supervision: `background_task_handle()` (`lib.rs:698-704`) returns only ONE handle via `.pop()`; with
N forwarders the runtime supervises exactly one and the rest are only `abort()`ed in `stop()`. The
design claims "Runtime still supervises the forwarder handles" (plural) — **false**; only one is
supervised. Either fix the supervision story or document the limitation.

---

### rc-gr8k — proto-compiler unique descriptor — CONFIRM (with note)

**Cross-ref:** `compiler.rs` — descriptor is written by protoc, then `fs::read` + `remove_file`
entirely **inside** `compile_proto`. `cache.rs:125` only receives the decoded `DescriptorPool`; it
never touches the temp file. So NamedTempFile lifecycle (create→protoc writes→read→drop-deletes) is
fully self-contained. Design is correct; cache layer is unaffected.

**Note (not blocking):** protoc writes the file via `--descriptor_set_out=<path>`. With
`NamedTempFile` you must pass `tmp.path()` to protoc (protoc overwrites the file NamedTempFile already
created — fine on Unix, fine on Windows since NamedTempFile keeps the handle but protoc opens by
path). Spec scenario "Descriptor file is cleaned up (no orphan files)" is satisfied by `Drop`, but the
current code ALSO already removes the file explicitly. Design should state whether the explicit
`remove_file` is kept (redundant but harmless) or dropped in favor of `Drop`. Minor.

---

### rc-xvuk — container cleanup docker_host — REFINE (gap)

**Cross-ref:** `cleanup_tracked_containers()` is a free `pub async fn` with **no params**
(`lib.rs:61`). Sole references: the definition and a doc comment at `lib.rs:1751`. No live caller in
the repo — so the `Option<&str>` signature change is a safe pre-1.0 API break.

**Gap:** design.md:53-58 says "accept `Option<&str>` and fall through to
`connect_with_local_defaults()` when `None`." But the existing host-aware connect logic lives in
**instance methods** `docker_socket_path()` (`lib.rs:437`) and `connect_docker_client()`
(`lib.rs:458`), which parse/strip the `unix://` / `npipe://` prefix. A free function taking a raw
`&str` host **cannot reuse** those instance methods and must **replicate** the socket-path parsing.
The design hand-waves this ("or make it read the same config source"). Required: specify the actual
connect path — extract a free helper `connect_docker_from_host(host: &str) -> Result<Docker>` that
both the instance method and cleanup call, so the parsing is not duplicated. Spec scenario expects
connection to `unix:///custom/docker.sock` — verify the helper strips the scheme as
`docker_socket_path` does today.

---

### rc-jh8s — WS TLS readiness — **FATAL (design mechanism unimplementable)**

**Cross-ref:** `spawn_server` (`lib.rs:187-278`). For `wss://` the code runs
`axum_server::bind_rustls(addr, cfg).serve(app).await` **inside** the spawned task (`lib.rs:228-243`).
`mark_ready()` fires at `lib.rs:1016` after `get_or_spawn` returns — premature for TLS. Bug is real.

**Why the proposed fix cannot work as written:** design.md:67-72 says the spawned task "sends `Ok(())`
on bind success" and `start()` "awaits this signal." But `bind_rustls(..).serve(..).await` is a single
combined future that **only resolves when the server terminates** (shutdown or error). There is no
"bind succeeded, now serving" checkpoint between `bind` and `serve` in that call. A oneshot sent
*after* `.serve().await` fires on **shutdown**, not readiness — the opposite of intent.

**The achievable fix (design must adopt it):** `axum-server 0.7` (confirmed `Cargo.toml:24`) exposes
`axum_server::Handle`. Pattern: create `let handle = Handle::new();` pass `.handle(handle.clone())`
to the builder, spawn `serve`, then in `start()` await `handle.listening().await` — returns
`Some(addr)` once bound, `None` if the server errored before binding. Signal `mark_ready()` on `Some`,
`mark_unhealthy()`/return `Err` on `None`. The oneshot idea can wrap this, but the **trigger** must be
`Handle::listening()`, not `.serve().await` completion. Rewrite design.md Fix 5 to name `Handle` and
`listening()`.

**Spec impact:** scenario "wss bind failure does not signal ready" (spec:90-95) asserts `start()`
returns an error AND `mark_ready` never called — only achievable via `Handle::listening()` returning
`None`. Keep the scenario; fix the design mechanism it rests on.

---

### rc-sfy1 — BeanError non_exhaustive — CONFIRM

**Cross-ref:** `error.rs:5-22` — no `#[non_exhaustive]` today. Only external reference is
`step_compilers/core.rs:347` which **constructs** `BeanError::MethodNotFound(...).into()` — a
construction site, unaffected by `#[non_exhaustive]` (only external `match` needs a wildcard arm; no
external match exists). Simple, correct, low-risk. Aligns with ADR-0049 and the workspace
`lint-non-exhaustive` gate. **Approve.**

**Nit:** spec:125 says "camel-bean test suite (23 tests)". Actual `#[test]`/`#[tokio::test]` count in
`src` is **15**. Either the count includes integration tests elsewhere or the number is wrong. Correct
the figure or drop the exact count (say "all existing tests pass unchanged").

---

### rc-7ka6 — endpoint-macros trybuild — **FALSE PREMISE**

**Cross-ref:** proposal.md:29 and design.md:84 claim "**Zero** trybuild compile-fail tests" and
propose *creating* `tests/ui/` + `tests/compile_fail.rs`. **This is false.** The crate ALREADY has:
- `tests/ui_tests.rs` — working harness (`trybuild::TestCases::new(); t.compile_fail("tests/ui/*_fail.rs")`)
- 4 existing case pairs in `tests/ui/`: `kind_typo_fail`, `no_optin_no_metadata_fn_fail`,
  `secret_with_default_fail`, `unknown_key_fail` (each with `.rs` + `.stderr`).

So this is **EXPAND an existing suite**, not create one. Required fixes:
1. Reword proposal/design: the task is adding NEW cases to the existing harness, not bootstrapping it.
2. Spec:160 references a `tests/compile_fail.rs` harness that **does not exist** — the real file is
   `tests/ui_tests.rs`. Fix the filename or the change will fail its own acceptance.
3. Spec:140-144 "Non-struct input rejected → 'only supports structs' message": grep of
   `uri_config.rs` finds **no such string**. The macro may not currently reject enums/unions with
   that wording (the panic/error may come from `syn` parse, or not at all). Verify the actual message
   before locking a `.stderr`, or the ui case cannot be authored. This scenario is **not grounded in
   existing behavior** — either add the guard first or drop the scenario.
4. Real, lockable messages that DO exist and should back the scenarios: `"unknown attribute key: {}"`
   (`uri_config.rs:106,118`), `"unknown uri_config option"` (`:265`), `"only one field can be the
   path field (first field without #[uri_param])"` (`:863`), `"missing #[uri_scheme = \"xxx\"]
   attribute on struct"` (`:147`). Spec scenarios should cite THESE verbatim.

---

## Cross-cutting checks

- **No-placeholders scan:** no `TODO`/`TBD`/`<...>`/`...` tokens in any artifact. ✅ (design.md:16-21
  uses prose "the simplest stable approach" but no literal placeholder token.)
- **Phase coherence:** the 7 tasks are genuinely independent (7 crates, no shared symbols, no
  ordering constraint). Single-phase is correct. ✅
- **ADR conflicts:** ADR-0049 (non_exhaustive) — consistent. ADR-0007 (supervision) — **partially
  violated** by rc-exa2 as designed (only 1 of N forwarders supervised); see finding.
- **Blast radius:** rc-xvuk API break is safe (no live callers). rc-3smd changes a public
  byte→char semantic (user-visible behavior change) — should be flagged as a behavior change in the
  proposal's risk budget, currently listed only as "localised and low-risk" (proposal.md:50). Bump to
  medium.

---

## Verdict: BLESS-WITH-FIXES

**Must fix before re-bless (blocking):**
1. **rc-exa2** — rewrite design: `tokio::sync::Mutex` + drop-guard-before-await (or MPMC channel);
   correct the "supervises all handles" false claim.
2. **rc-jh8s** — rewrite design to use `axum_server::Handle::listening()`; the `.serve().await`
   oneshot mechanism is unimplementable.
3. **rc-7ka6** — correct the "zero tests" false premise (suite exists); fix `compile_fail.rs` →
   `ui_tests.rs`; ground or drop the "only supports structs" scenario; cite real error strings.

**Should fix before re-bless (correctness/accuracy):**
4. **rc-3smd** — commit to char semantics explicitly; add the `chars().count()` guard change; note the
   existing-test assertion change; flag as a behavior change in risk budget.
5. **rc-xvuk** — specify a shared free `connect_docker_from_host` helper to avoid duplicating
   socket-path parsing.
6. **rc-sfy1** — fix the "23 tests" count (actual 15).

**Confirmed sound:** rc-gr8k (minor note on explicit remove vs Drop), rc-sfy1 (the fix itself).
