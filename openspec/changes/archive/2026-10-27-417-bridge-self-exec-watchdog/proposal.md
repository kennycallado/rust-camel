# Proposal: 417-bridge-self-exec-watchdog

## Why

Every Unix `BridgeProcess::start` spawns `sh -c <watchdog script>` as the
parent-liveness watchdog (`GroupGuard::establish`, camel-bridge). The shipped
images have no shell: `production` is `FROM scratch` and `gnu` is distroless,
so any bridge route inside them fails at start with
`watchdog spawn (sh) failed`. Separately — and earlier —
`BridgeTlsMaterial::generate()` creates its PEM tempdir under `$TMPDIR`/`/tmp`
before the guard runs; scratch has neither, so bridge routes in `production`
have been broken since 5be506864 unless `/tmp` was mounted (ADR-0084). The
spec (`bridge-process-lifetime` :37) currently MANDATES a POSIX-shell
watchdog, so the fix requires a spec delta. Found by the owner 2026-10-26
reviewing the xml bridge in scratch-based images; v0.61.0 ships the defect
(bd rc-fnng9, P0, e_opus ruling 2026-10-26: option D).

## What Changes

- Add a `camel_bridge::watchdog` module: embedders call `maybe_run()` as the
  first statement of a synchronous `main`. When argv carries the hidden role
  marker (`__camel-bridge-watchdog` + ack fd), the binary re-run in that role
  executes the watchdog protocol (SIGTERM ignore, ack byte on the inherited
  fd, stdin EOF, `kill(0, SIGKILL)`) and refuses to arm unless it is its own
  process-group leader.
- `GroupGuard::establish()` spawns the supervising executable in the watchdog
  role (Linux: `/proc/self/exe`; other Unix: executable path captured at
  startup) instead of `sh`, keeping `setpgid`, the fd-9 dup2, and the
  ack-before-arming contract unchanged. POSIX `sh` remains only as the
  no-hook fallback; if neither the hook nor a shell exists, establishment
  fails closed naming both remedies.
- `camel-cli` installs the hook: sync `main` (drops `#[tokio::main]`) calls
  `maybe_run()` before `self_detect_artifact`, then runs the async body on an
  explicitly built multi-thread runtime.
- `BridgeTlsMaterial` resolves its base directory without requiring `/tmp`:
  `$TMPDIR` (if set and usable) -> system temp dir (if usable) ->
  `{cache_dir}/{version}/tls` (mode 0700, created on demand), with a clear
  error naming the cache-dir env override when nothing is writable.
- Container guard in the `docker-smoke-test` CI job: `production` and `gnu`
  images run a route that starts a bridge via a static fake-bridge fixture
  (mounted read-only, `CAMEL_JMS_BRIDGE_BINARY_PATH`), with no shell and no
  `/tmp`; a `--read-only --user 65532` + cache-volume variant. The guard must
  fail on pre-fix code.
- ADR (0089) + docs; no workspace version bump (v0.61.1 + release-notes known
  issue are the owner's step).

## Impact

- Specs: `bridge-process-lifetime` — requirements :6, :37, :196 modified; new
  TLS-directory requirement and new watchdog scenarios.
- Code: `camel-bridge` (new `watchdog` module, `group.rs`, `tls.rs`),
  `camel-cli` (`main.rs`, `Cargo.toml`), CI workflow + new guard script,
  new test-only fixture crate, ADR, bridge docs.
- Affected: every bridge consumer in containers (xml, jms, cxf); embedders
  opt into the hook for full no-shell support.
