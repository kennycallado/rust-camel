# Design: 417-bridge-self-exec-watchdog

Mechanism ruled by e_opus (bd rc-fnng9 comment, 2026-10-26): option D —
re-exec the host binary itself as the watchdog. Rejected: PDEATHSIG
(per-thread, misses grandchildren, Linux-only), helper binary (same cost as D
plus an extra artifact), bundled shell.

## Watchdog role (new `camel_bridge::watchdog`)

`crates/services/camel-bridge/src/watchdog.rs` (unix):

- `pub fn maybe_run()` — synchronous, no runtime. Reads `std::env::args_os()`.
  Always records, in process-global state, that the hook is installed plus the
  executable path (`std::env::current_exe()` captured once at startup — this
  is the "captured at startup" path used by non-Linux unix). If
  `argv[1] != "__camel-bridge-watchdog"`, returns immediately.
  Non-unix: no-op (the crate has no unix supervision there).
- Role execution (`argv[1] == "__camel-bridge-watchdog"`, `argv[2]` = ack fd
  number, always 9): refuse to arm — exit non-zero BEFORE writing the ack —
  unless `getpgrp() == getpid()`; `signal(SIGTERM, SIG_IGN)`; write the single
  ack byte `\x01` to the fd; read stdin until EOF; `kill(0, SIGKILL)`. This is
  the shell script's protocol, word for word.
- `pub(crate) fn hook_executable() -> Option<&'static Path>` — the recorded
  path, `None` when `maybe_run()` was never called.

## GroupGuard spawn change (`group.rs`)

`establish()` keeps `establish_with_program("sh")` as the no-hook fallback and
adds the hook path: when `hook_executable()` is `Some`, spawn
`[exe, "__camel-bridge-watchdog", "9"]` instead of `["sh", "-c", script]` —
same liveness pipe as stdin, stdout/stderr nulled, same `pre_exec`
(`setpgid(0,0)` + ack dup2 to fd 9), same arming verification, same monitor.
Executable source: Linux reads `/proc/self/exe` at establish time (exec of the
magic symlink is inode-stable across binary replacement); other unix uses the
path captured by `maybe_run()`. If the no-hook `sh` spawn fails, the error
names BOTH remedies: install `camel_bridge::watchdog::maybe_run()` as the
first statement of main, or provide a POSIX `sh`.

Embedder contract: `maybe_run()` MUST be the first statement of a
synchronous `main`, before anything that consumes argv — in `camel-cli`,
before `compile::runtime::self_detect_artifact()` (a compiled artifact would
otherwise swallow the watchdog argv).

## camel-cli hook

`main.rs`: drop `#[tokio::main]`; `fn main()` is synchronous — call the hook
first (unconditional: `camel-bridge` is a non-optional direct dependency
because the xsd bridge in `camel-component-validator` is always linked),
install the rustls provider, build the
multi-thread runtime explicitly (`Builder::new_multi_thread().enable_all()` —
the `#[tokio::main]` default), `block_on` the existing async body moved to
`async fn async_main()`.

## TLS material without /tmp (`tls.rs`)

`generate()` resolves a BASE directory, then creates the per-spawn PEM dir
inside it with the existing unique-tempdir + 0700 mechanics
(`tempfile::Builder::tempdir_in`, `Drop` cleanup unchanged):

1. `$TMPDIR` when set and usable (a usable probe = the `tempdir_in` attempt
   itself — no separate writability probe, no TOCTOU);
2. `std::env::temp_dir()` when usable (the system temp, `/tmp` on Linux);
3. `dirs::cache_dir()/{camel-bridge version}/tls` — each component created
   with mode 0700, validated by the same probe; on failure the error names the
   platform cache-dir override (`XDG_CACHE_HOME` on Linux).

In scratch: no `$TMPDIR`, no `/tmp` -> rung 3 with `HOME=/` resolves to
`/.cache/...` (writable as root); under `--user 65532` + read-only rootfs the
operator mounts a writable volume and points `XDG_CACHE_HOME` at it.

## Container guard

New script `.github/scripts/docker-bridge-guard.sh` called by the existing
`docker-smoke-test` CI job (also runnable locally for the pre-fix failure
proof). Per image (`production`, `gnu`):

- Build the static fake-bridge fixture (new `crates/tools/camel-fake-bridge`,
  `publish = false`: musl static tonic server speaking
  `camel-jms`'s `BridgeService` — `Health` returns healthy with empty
  capabilities, which passes the startup gates for a non-TLS broker and no
  selectors — binds `QUARKUS_HTTP_SSL_PORT` (0 = OS-assigned), announces the
  standard ready line `{"status":"ready","port":N}`, serves until killed).
- Run `camel run` on a minimal jms route with the fixture mounted read-only
  and `CAMEL_JMS_BRIDGE_BINARY_PATH` pointing at it; container has NO shell
  and NO `/tmp`; assert the bridge-ready marker appears in container logs
  (readiness from the host), then `docker stop` and assert a clean, bounded
  exit (teardown: guard drop -> EOF -> group SIGKILL inside the dying
  container's PID namespace).
- Variant: `--read-only --user 65532` with `XDG_CACHE_HOME` on a mounted
  writable volume.
- The guard must FAIL against pre-fix code (sh/TLS failure modes) — captured
  as a transcript in the park report before the core fix lands.

Nightly/release keeps the real xml-bridge run (explicitly out of scope; no new
workflow matrix).

## Specs

`bridge-process-lifetime` delta: :6 "shell watchdog" -> "watchdog process";
:37 rewritten (self-executing watchdog with hook; POSIX `sh` only as no-hook
fallback; refuse-to-arm; fail-closed naming both remedies); :196 container
statement (full guarantee in `production`/`gnu` with no shell and no `/tmp`;
operator-provided writable cache dir). ADDED requirement: TLS material
directory resolution without `/tmp`. New scenarios: empty-PATH/no-shell start
succeeds; no-hook + no-shell fails closed naming both remedies; watchdog
refuses to arm when not a group leader; hook precedes artifact self-detection.

## Phases

- **Phase 1: guard harness + pre-fix proof** — fixture crate, guard script,
  CI wiring; run the guard against the base (pre-fix) binary and capture the
  expected failure. Exit: transcript recorded, no production code changed.
- **Phase 2: self-exec watchdog core** — `watchdog` module + `group.rs` hook
  path + lifecycle/arming tests (harness-free e2e test binary installs the
  hook exactly like an embedder). Exit: `cargo test -p camel-bridge` green
  including new tests.
- **Phase 3: TLS directory fallback** — `tls.rs` rungs + per-rung tests.
  Exit: TLS-dir tests green.
- **Phase 4: camel-cli hook** — sync main + non-optional dep wiring. Exit: cli
  tests green, xml_bridge_validator_test regression proof.
- **Phase 5: post-fix proof + decision record** — guard passes on fixed
  images (production + gnu + read-only variant), ADR-0089, docs, gates,
  `cargo xtask archive`.

Autopilot budget is global across phases (conductor rule).

## Test strategy notes

- The hook-path lifecycle tests use a `harness = false` integration test
  target whose `main` calls `maybe_run()` first — the test binary re-execs
  ITSELF through `GroupGuard::establish()` with an empty `PATH`, exercising
  the real embedder shape with no shell and no fixture binaries.
- Refuse-to-arm: spawn the same test binary with the role argv and NO
  `setpgid` (child inherits the caller's group) — child must exit non-zero
  without writing the ack.
- No-hook + no-shell fail-closed: harness test (hook never installed) with
  `PATH` emptied -> `establish()` -> error naming both remedies.
- Existing lifecycle tests keep using the shell fallback unchanged (their
  processes never install the hook), proving backward compatibility.
