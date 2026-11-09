# Design: macos-group-reap

## Approach

Everything lands in the test module (`mod tests`, `#[cfg(all(test, unix))]`)
of `crates/services/camel-bridge/src/group.rs`. Three pieces:

1. **Pure classifier (platform-independent, compiled wherever the unix test
   module compiles — including Linux, where it is unit-tested):**

   - `struct PsRow { pid: i32, pgid: i32, state: String, command: String }`
     — `state` keeps the raw `stat` token verbatim (including modifiers
     such as `S+`); classification reads its first letter only.
   - `fn ps_rows(output: &str) -> Result<Vec<PsRow>, SnapshotUnusable>` —
     whitespace-parses `pid pgid stat comm...` rows, skipping blank lines.
     Any non-blank line that fails to parse (bad pid/pgid, missing state
     column, header row) makes the WHOLE snapshot unusable: the function
     returns `Err` instead of silently dropping the row, because a dropped
     live member's row would let `survivors_from_ps` report a false-empty
     group. Fixture tests on Linux pin the fallback: usable snapshots
     (running, sleeping, zombie, mixed group), and unusable ones (malformed
     row, empty output, header row).
   - `fn is_alive_state(state: &str) -> bool` — inspects the first state
     letter only: `Z` (zombie) is not alive; `X` (dead, Linux `ps` letter)
     is not alive; everything else (`R`, `S`, `I`, `T`, `U`, `L`, …) is
     alive. Trailing modifiers (`+` foreground, `<` privileged, `E` trying
     to exit) are ignored — on macOS a zombie renders as a bare `Z`.
   - `fn survivors_from_ps(output: &str, pgid: i32)
     -> Result<Vec<PsRow>, SnapshotUnusable>` — validates via `ps_rows`,
     then returns alive rows whose pgid matches; this is the
     group-survivor verdict. Fallible by design: the `Err` is what
     explicitly selects the kill-probe fallback at the call site.
   - `fn describe_survivors(output: Option<&str>, kill0: impl Fn(i32) -> bool,
     pids: &[i32], pgid: i32) -> String` — the legible-failure formatter.
     `Some(text)` is a snapshot that has ALREADY passed `ps_rows`
     validation at the call site (never a malformed buffer, so a malformed
     `Some` can never render `GONE`); `None` means the snapshot was
     unavailable OR unusable — it renders `ps unavailable` and classifies
     every probed pid and the group as `unknown`. Per listed pid: raw
     `ps` state token (or `absent`), `kill(pid, 0)` result, command name,
     and the verdict `ZOMBIE` vs `ALIVE` vs `GONE` vs `unknown`. For the
     group: `kill(-pgid, 0)` result, each member row of that pgid with its
     state, alive-member count. The `kill0` probe is injected so the
     formatter stays pure and fixture-testable on Linux.

2. **macOS mechanism (`#[cfg(not(target_os = "linux"))]`, test-only):**
   one invocation of `/bin/ps -A -o pid=,pgid=,stat=,comm=` capturing stdout
   (absolute path, PATH-independent; `-A` selects all processes; the bare `=`
   forms suppress headers). Justification:

   - A zombie on macOS holds its pid slot until reaped, so `kill(pid, 0)`
     keeps succeeding — that is why the old probe cannot converge. The
     kernel does not expose the zombie-vs-alive distinction to signal
     probes; `ps stat` does.
   - One snapshot carries pid, pgid, state, and command for every process:
     per-pid checks, group filtering (exact pgid column match — no
     `pgrep -g`, which cannot return states), and command names all come
     from the same buffer, parsed once per predicate call. Each predicate
     call takes its own snapshot — a few `/bin/ps` invocations per poll
     iteration, an accepted test-only cost well inside the 5 s budget.
   - **Single validation point:** `fn usable_snapshot() -> Option<String>`
     (non-Linux, test-only) runs `ps`, checks exit status and non-empty
     output, and passes the buffer through `ps_rows` validation; it
     returns `Some(usable text)` or `None` for EVERY unusable reason
     (spawn failure, non-zero exit, empty output, unparseable row).
     Helpers and diagnostics share this one point, so error propagation
     has exactly one shape: `None` = fall back / report `unknown`.
   - State letters: `R` running, `S` sleeping, `I` idle (asleep >20 s —
     still alive), `T` stopped, `U` uninterruptible wait, `Z` zombie.
     Only `Z` (and `X` for parity with Linux) means "not a survivor".
   - **Conservative fallback:** for EVERY unusable reason — spawn/capture
     failure, non-zero exit, empty output, any unparseable non-blank row
     (`usable_snapshot()` returns `None`) — each predicate falls back to
     its old bare kill-probe answer (`kill(pid, 0)` for the pid probe,
     `kill(-pgid, 0)` for the group probe), and the diagnostic prints
     `ps unavailable` plus an `unknown` classification for the affected
     pids — unavailable evidence is never reported as `absent`/`GONE`. A
     broken snapshot can therefore only make a test *stricter*, never mask
     a real survivor.

   `pid_running(pid)` = `usable_snapshot()` → `None`: bare probe;
     `Some(text)`: row for pid → `is_alive_state(row.state)`.
   `group_running(pgid)` = `usable_snapshot()` → `None`: bare probe;
     `Some(text)`: `!survivors_from_ps(text, pgid).unwrap_or_default()
     .is_empty()` — TRUE exactly when at least one ALIVE member remains in
     the group (a zombie-only group is NOT running), matching the Linux
     `/proc` filter's polarity (`group.rs:807-811`).
   The Linux `/proc`-based helpers are untouched — no shell dependency is
   added on Linux.

3. **Rewiring and evidence:**
   - `zombie_probe_contract` (currently pins
     `pid_running(zombie) == true` on macOS at `group.rs:1179-1183`) flips
     to `!pid_running(pid)` with an updated comment; its Linux assertion
     and the reap invariant are unchanged.
   - Every teardown poll site (`drop_reaps_without_signals`,
     `dash_shell_establishes_and_drops`, and the sibling tests sharing the
     same predicate) builds its assertion message lazily — only after
     `wait_until` returns `false` — via `describe_survivors`, so the
     message includes ps state, kill-probe result, and command per pid and
     for the group.
   - Stale doc comments on the non-Linux helpers (`group.rs:814-832`:
     "tests must wait (bounded) for the reap") are rewritten to describe
     the zombie-aware semantics.

## Affected crates

- `camel-bridge`: `group.rs` `mod tests` only — helpers, classifier, tests.
  No production path, no API surface, no other crate.

## Architecture boundaries

Services-layer bridge supervision is already spec'd by
`openspec/specs/bridge-process-lifetime/spec.md`; this change adds a delta
requirement there about test-side survivor evidence and touches nothing in
Runtime, DSL, Components, Languages, or Functions. The watchdog contract
(children cannot outlive the parent, non-blocking `Drop`, 5 s bounded test
waits) is consumed, not modified.

## Alternatives considered

- **`pgrep -g <pgid>`** — returns pids only, no state letter; cannot
  distinguish zombie from alive without a second per-pid call. Rejected.
- **Parsing `ps -o stat=` per pid on demand** — N invocations per poll
  iteration instead of 1; noisier on CI, same information. Rejected.
- **Blocking reap in `Drop` (`ReapMode::Blocking`)** — would likely fix the
  flake but changes the production contract (drop never blocks); out of
  scope by order, reported separately if ever pursued.
- **`waitid(WNOWAIT)` from the test** — the test does not own the child
  handles (`tokio::process::Child` inside the guard's slots); reaping in
  the test would race the monitor's cleanup contract. Rejected.
