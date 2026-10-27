# ADR-0089: Self-executing watchdog for bridge process supervision

- Status: Accepted (decided 2026-10-26)
- Source: bd `rc-fnng9`; openspec change `417-bridge-self-exec-watchdog`
- Ruling: e_opus, 2026-10-26 — option D, re-execute the host binary
- Cross-refs: ADR-0036 (bridge IPC security), ADR-0075 (self-contained
  executable artifact format), ADR-0080 (stdio data-plane posture)

## Context

Every Unix `BridgeProcess::start` armed its parent-liveness watchdog by
spawning `sh -c <script>`. The script watched a pipe, ignored `SIGTERM`,
and `SIGKILL`ed its process group when the parent died. The guarantee was
sound. The dependency was not: the shipped `production` image is
`FROM scratch` and the `gnu` image is distroless. Neither contains a shell.
Every bridge route in those images therefore failed at start with
`watchdog spawn (sh) failed`.

A second defect compounded the first. `BridgeTlsMaterial::generate()`
created its PEM directory under `$TMPDIR` or `/tmp` before the watchdog was
armed. Scratch images have neither path. Bridge routes in `production`
failed during certificate generation unless an operator mounted `/tmp`.

The bridge process-lifetime specification mandated a POSIX-shell watchdog,
so the fix required a specification delta. The rule in this ADR restores the
supervision guarantee without adding a runtime dependency or a second
artifact.

## Decision

### The host binary supervises itself in a hidden role

`camel-bridge` exposes `camel_bridge::watchdog::maybe_run()`. The embedder
calls it as the first statement of a synchronous `main`. When argv carries
the hidden role marker, the function becomes the watchdog and never
returns. Otherwise it records that the hook is installed and returns.

The role is a re-execution of the supervising executable with:

- `argv[1]` equal to `__camel-bridge-watchdog`.
- `argv[2]` equal to the acknowledgement descriptor, always fd 9.
- A single acknowledgement byte `0x01` written to that descriptor.

The role refuses to arm unless it already leads its own process group, and
exits with status 74 before writing the acknowledgement. A non-leader could
otherwise `kill(0, SIGKILL)` foreign processes. The role ignores `SIGTERM`,
reads the liveness pipe on stdin until EOF, then calls `kill(0, SIGKILL)`.
This is the shell script's protocol, word for word. The arming, verification,
and teardown code paths are shared with the shell fallback.

### Executable source is platform-specific

On Linux the role is spawned by exec-ing the literal `/proc/self/exe`. The
magic link is inode-stable across binary replacement.

On other Unix targets the role uses `std::env::current_exe()`, captured once
by the hook at process startup. A later re-exec changes what
`current_exe()` returns, so the startup value is the only stable source.

### Embedder contract

`maybe_run()` MUST be the first statement of a synchronous `main`. It MUST
run before any code consumes argv. In `camel-cli` it runs before
`self_detect_artifact`. A compiled artifact would otherwise consume the
watchdog argv, and the watchdog would never start.

The hook is called on every platform and is a no-op outside Unix. In
`camel-cli` it is unconditional in every flavor, slim included: the xsd bridge
(`camel-component-validator`) is an unconditional dependency, so `camel-bridge`
is in every closure. Without the hook the crate keeps the POSIX `sh` fallback.
Without the hook and without a shell, establishment fails closed with an error
that names both remedies: install the hook, or provide a POSIX `sh`.

### TLS material does not require `/tmp`

`BridgeTlsMaterial` resolves a base directory in this order:

1. `$TMPDIR`, when set, non-empty, and usable.
2. The system temporary directory, when usable.
3. `{cache_dir}/{version}/tls`, created on demand with mode `0700`.

Each candidate is probed by performing the real `tempdir_in` creation. There
is no separate writability probe and no check-then-use window. Directory
components that this crate creates get mode `0700`. Components that already
exist keep their mode. If every candidate fails, the error names
`XDG_CACHE_HOME` and `$TMPDIR` as the remedies.

### Container guarantees

With the hook installed, bridge routes start in the `production` (scratch)
and `gnu` (distroless) images with no shell and no usable `/tmp`. The
operator must provide one writable location for TLS material: a writable
cache directory, or a writable `$TMPDIR`. A read-only root filesystem with a
non-root user works when a writable volume is mounted and `XDG_CACHE_HOME`
points at it.

Windows is unchanged. It has no process groups and no POSIX shell.
Supervision there is explicit-path teardown only: `stop()` and `Drop` kill
the child directly. An unclean parent death can leave the bridge and its
descendants running. That is a known leak, not a supervised guarantee.

## Consequences

Bridge containment no longer depends on a shell being present. The same
executable provides both the bridge host and its supervisor, so the release
carries no second watchdog artifact. Startup costs one additional exec per
bridge, which is small relative to bridge bootstrap.

The hook is process-global state and must be installed before argv is read.
An embedder that installs it late, or from an argument-parsing layer, gets
the shell fallback or a fail-closed error. This ordering is part of the
public contract and is documented in the bridge README and service page.

The TLS fallback removes the `/tmp` requirement but adds an operator
responsibility in hardened containers: a writable cache location or
`$TMPDIR` must exist. The container guard verifies the default and the
read-only variants. The macOS evidence suite has not been run; the guarantee
holds by design there.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| `PR_SET_PDEATHSIG` | The signal is per-thread, not per-process. It misses grandchildren and does not cover the process-group guarantee. It is Linux-only. |
| Separate helper binary | Same mechanism cost as the chosen option, plus an extra release artifact that must be built, downloaded, verified, and kept in version lockstep. |
| Bundled shell | Image bloat and a new attack surface. It reintroduces the dependency the decision removes. |
| Require a shell in every image | Forces `scratch` and distroless images to drop their isolation guarantees, and leaves third-party embedders without a remedy unless they rebuild their image. |

## Evidence and limits

The container guard runs bridge routes in the `production` and `gnu` images
with no shell and no usable `/tmp`. It also runs a read-only, non-root
variant with a mounted cache volume. The guard fails on the pre-fix code.

Automated tests cover the role protocol, refusal before acknowledgement,
acknowledgement failure, the no-hook fallback, and the no-hook plus
no-shell fail-closed message. SIGTERM immunity and the stdin-EOF group kill
are exercised on both the shell path (the parent-death matrix) and the hook
path: the harness-free end-to-end binary re-executes the embedder in the
bridge-owning role, confirms a group SIGTERM leaves the watchdog, bridge,
and grandchild alive, then SIGKILLs the parent and confirms the group
disappears on EOF. A compiled-artifact test proves the hook handles the
role argv before artifact self-detection can consume it. TLS tests cover
each fallback rung, the created-directory mode, pre-existing component
modes, and the all-rungs-failed error.

## References

- ADR-0036: Bridge IPC mutual TLS. The watchdog carries no credential
  material and does not alter the bridge transport.
- ADR-0075: Self-contained executable artifact format. The hook must run
  before artifact self-detection consumes argv.
- ADR-0080: Stdio data-plane posture. The watchdog owns stdin and writes
  nothing to inherited stdout or stderr.
- bd `rc-fnng9`: defect and e_opus ruling of 2026-10-26.
- openspec change `417-bridge-self-exec-watchdog`, design and specification
  delta `bridge-process-lifetime`.
