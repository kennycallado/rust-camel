# Design: bridge-child-lifetime

## Approach

Mechanism chosen (mission option a, refined through four blessed review
rounds): **supervisor-first process-group watchdog driven by a
parent-liveness pipe**, with exclusive single-owner cleanup. Nothing relies
on Rust destructors or exit handlers running in the parent.

1. **Supervisor-first watchdog.** `BridgeProcess::start` first creates a
   close-on-exec pipe (`pipe2(O_CLOEXEC)`) and spawns a POSIX-shell watchdog:
   `sh -c 'trap "" TERM; while read -r _; do :; done; kill -KILL -0'` —
   builtins only, no external binaries, and SIGTERM-immune via `trap` so a
   teardown SIGTERM to the group cannot disarm the parent-death backstop.
   The watchdog's pre_exec makes it its own process-group leader
   (`setpgid(0,0)`); its stdin is the pipe read end (CLOEXEC cleared by the
   stdio dup on the installed fd); stdout/stderr are null so the watchdog
   never pins the parent's stderr. The parent then verifies arming — watchdog
   alive, leading its group, and TERM immunity established — before any
   bridge exists; failure is fail-closed and aborts `start()`.
2. **Bridge joins the group at exec.** The bridge child's pre_exec runs
   `setpgid(0, <watchdog pid>)` (child-side, before exec — no EACCES race; a
   failed join aborts the spawn before exec). Stdout stays piped for
   readiness, stderr becomes piped (below), stdin is null. Because the
   watchdog exists and is armed before the bridge, the startup interval is
   supervised: a parent SIGKILL between arming and bridge spawn fires EOF
   against an empty group and leaks nothing.
3. **Parent death kills the group.** The parent holds the pipe write end
   exclusively, inside a `GroupGuard` that exclusively owns both child
   handles and performs every group signal. The parent never writes; any
   parent exit route — normal exit, panic unwind, SIGTERM, SIGINT, SIGKILL,
   or death during teardown windows — closes the descriptor, the watchdog's
   `read` loop hits EOF, and its built-in `kill -KILL -0` destroys its own
   group: bridge, launcher descendants, watchdog. Guard drop, startup
   failure, and cancellation close the same fd, so ownership end and group
   death always coincide. No atexit registry, no `PR_SET_PDEATHSIG` (cleared
   across `fork`, descendants unprotected), no bare-PID signaling anywhere.
 4. **Single-owner signaling, non-reaping observation, PGID anchoring.** The
    entire cleanup sequence is ONE synchronous core (`cleanup_locked` under a
    `std::sync::Mutex`, with a lock-taking `cleanup_sync` wrapper) executed
    either on the dedicated monitor thread, inside
    `tokio::task::spawn_blocking` from async `stop()`, or inside `Drop` — no
    cleanup lock is ever held across an await, and supervision is
    runtime-independent. All guard-executed group signals serialize through
    that mutex. Exit observation before the final group
    signal NEVER reaps: both the bridge and the watchdog are observed with
    `waitid(P_PID, ..., WEXITED | WNOHANG | WNOWAIT)` — death is seen while
    the child stays waitable, so the identity anchor survives until final
    signaling (this also lets `spawn_bridge` reject a zombie watchdog before
    joining its group). The teardown sequence is fixed: SIGTERM to the group → bounded
    non-reaping wait → unconditional group SIGKILL → invalidate the stored
    group id → reap both children. `Drop` follows the same tail with
    `try_lock` plus non-blocking reap; if the lock is contended the
    in-flight cleanup owns the sequence. No group signal is ever issued after
    invalidation; combined with kernel group-id semantics (an id cannot be
    recycled while any member lives), no signal can reach an unrelated group.
 5. **Early watchdog death is detected, runtime-independently.** A dedicated
    `std::thread` (NOT a tokio task — dropping or shutting down the caller's
    runtime must never disarm supervision) observes the watchdog with the
    same non-reaping `waitid(WNOWAIT)` poll — a zombie watchdog answers
    `kill(pid,0)` forever, so exit detection must be `waitid`-based, not
    signal-based — and on exit while the parent lives performs the serialized
    sync cleanup itself, logging `tracing::error!("bridge watchdog exited
    early; group cleaned")`. The thread holds a `Weak<GroupShared>` and a
    pid — nothing else: not the write end, not the child handles, not a
    strong guard reference — and terminates when the guard drops (failed
    upgrade) or cleanup sets the stop flag. Reaping happens only inside the
    serialized cleanup. Killed-orphan zombies (no fds, no CPU) are distinct
    from running survivors; only running members fail the process-table
    evidence checks.
 6. **Explicit teardown.** Fixed sequence from (4): `stop()` runs
    TERM → bounded non-reaping wait → KILL → invalidate → bounded synchronous
    reap of bridge and watchdog (via `spawn_blocking`), then drops the guard.
    Escalation never
    depends on leader liveness or identity re-validation.
7. **Stdio ownership.** Default stderr becomes `piped()` and is drained by a
   task cloned from the existing stdout drain (64 KiB line bound, 100
   lines/s forwarding budget, overflow dropped with summary, `tracing`
   target `camel_bridge::child`). The drain starts immediately after spawn,
   before readiness parsing, so chatty stderr cannot fill the pipe and
   deadlock startup. `CAMEL_BRIDGE_LOG_STDERR` keeps redirecting stderr to a
   file with today's failure semantics. Concurrent bridge spawns each get
   their own liveness pipe; no descriptor is shared or inherited (CLOEXEC
   discipline).

**Platforms.** Linux and macOS: full guarantee — POSIX shell builtins,
`setpgid`, `trap`, and `waitid` are present on both; no `/proc` dependency.
Windows: no process groups or POSIX shell; behavior is explicit
`stop()`/`Drop` kill only — documented as such. CI evidence runs on Linux.

**Workflow workaround.** Kept in this change. The local proof exercises the
real spawn path with a fake bridge executable; the published smoke runs real
JVMs in CI, and that evidence only exists post-merge. Removal is a filed
follow-up; the workaround is 3 lines and zero-risk to keep one CI cycle.

## Affected crates

- `camel-bridge`: spawn path, teardown, new watchdog/group-guard module,
  evidence tests.
- `camel-test`, `camel-jms`, `camel-cxf`, `camel-validator`, `camel-xj`,
  `camel-xslt`: no code change — they consume the shared `BridgeProcess`
  path. `camel-ws` verified as a non-consumer (no camel-bridge dependency).

## Architecture boundaries

Services layer owns process lifetime; components and tests keep consuming
`BridgeProcess` unchanged. No Runtime/DSL/Functions surface changes. Log
forwarding reuses the established R4-L7 bounded-drain pattern and the
existing `camel_bridge::child` tracing target, staying inside the ADR-0012
log convention and redaction lint.

## Alternatives considered

- **PDEATHSIG + spawner thread (rejected at first bless):** cleared across
  `fork`, so launcher grandchildren survive parent death; thread-scoped.
- **atexit survivor sweep (rejected at first bless):** misses signals and
  abort; exit-time mutex and PGID-reuse hazards.
- **Parent-side watchdog group assignment (rejected at second bless):**
  `setpgid` by the parent after exec fails with EACCES; group joins are
  child-side pre_exec only.
- **Watchdog spawned after the bridge (rejected at second bless):** leaves
  the startup interval unsupervised; supervisor-first removes the window.
- **TERM-mortal watchdog (rejected at third bless):** a teardown SIGTERM
  killed the backstop, leaking the group if the parent died in the
  escalation window; `trap "" TERM` closes it.
- **`kill(pid,0)` liveness probes and reap-before-invalidate ordering
  (rejected at fourth bless):** zombies answer `kill(pid,0)` forever, so
  watchdog death was undetectable; `waitid(WNOWAIT)` observation plus
  KILL→invalidate→reap ordering close both.
- **Pipe+drain only (option c):** fixes the hang, not the orphans; adopted
  for stderr hygiene, not lifecycle.
- **Removing the CI workaround now:** deferred one cycle; see above.
