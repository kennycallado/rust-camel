# Task 5 — Full v3 Measurement Run (Handoff)

> Generated 2026-07-20 after conductor investigation + Task 4.5
> verification + artifact rebuilds. This is the user-run task: the
> 5-6h measurement campaign that produces v3 benchmark data.

## TL;DR

Everything is rebuilt and verified at the smoke level. Run one command
to launch the full v3 measurement campaign:

```bash
cd /home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark
export JAVA_HOME=/tmp/rc-f3g9-jdk21  # any JDK 21 works now (container does native-image)
export PATH=$JAVA_HOME/bin:$PATH
nohup bash benchmarks/harness/run.sh \
  --metric=m1+m2 \
  --scenarios=startup-minimal,t2-realistic-eip,http-server,xslt-bridge,xsd-validation-bridge \
  --n=50 \
  --warmup=3 \
  --warmup-time=30 \
  --warmup-msgs=100000 \
  --rounds=5 \
  --samples-per-round=100 \
  > /tmp/v3-task5-full.log 2>&1 &
echo "task5 PID=$!"
```

Expected wall-clock: 5-6 hours (first invocation downloads the
mandrel builder image, ~600 MB). Output lands at
`benchmarks/results/<timestamp>/`.

## Pre-flight state (verified 2026-07-20 12:20)

### Binaries (all rebuilt)

| Binary | Path | Size |
| --- | --- | --- |
| rust-camel-cli (`camel`) | `target/release/camel` | 89 MB |
| rust-camel-lib × 5 | `benchmarks/scenarios/<scenario>/rust-camel-lib/target/release/<scenario>` | 5–8 MB each |
| bench-loadgen | `benchmarks/harness/loadgen/target/release/bench-loadgen` | ~10 MB |
| Bridge binary | `bridges/xml/build/native/xml-bridge` | 104 MB |
| Quarkus native × 4 | `benchmarks/scenarios/<scenario>/camel-quarkus/*-native/build/*-runner` | 60–75 MB each |
| Camel standalone jars × 8 | `benchmarks/scenarios/<scenario>/camel-standalone/*/target/*-jar-with-dependencies.jar` | ~6 MB each |

### Smoke-verified end-to-end

M2 smoke for `http-server` (8 cells):
- Calibration: 8/8 cells calibrated at 1280/sec.
- Measurement: 8/8 cells produced Protocol A summary files.
- rust-camel-cli: `p50=136µs p99=171µs` (BCa CI 95%: 126–150µs).
- 4 cells hit `MessageBoundUnconverged` at `--warmup-msgs=1000` (smoke
  config too small). The full run uses `--warmup-msgs=100000`, which
  is the spec-mandated value — convergence expected.

### Wrapper bug FIXED

Commit `ea71de05` resolved the rust-camel-cli Protocol A wrapper
orphan-port bug via `_kill_process_tree_recursive` (walks
`/proc/<pid>/task/<tid>/children`, kills descendants before parent,
handles setsid-reparented children correctly). Verified by direct
isolation test: SIGKILL on wrapper → camel child killed → port 8080
released. Pre-fix runs left orphan; post-fix runs clean.

### Container-based native builds (NEW — commit `ad20a7c2`)

Quarkus native artifacts are now built inside a Mandrel container
(`quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21`) instead
of requiring host-installed GraalVM CE 21.0.2 + NixOS-specific zlib
path. The host JDK requirement is relaxed to "any JDK 21" (used only
for gradle invocation; native-image runs in the container).

Override env vars to switch modes:
- `QUARKUS_NATIVE_CONTAINER_RUNTIME=docker` (default) — container build
- `QUARKUS_NATIVE_CONTAINER_RUNTIME=local` — legacy host GraalVM path
  (requires JAVA_HOME= GraalVM CE 21.0.2 + NixOS zlib workaround)
- `QUARKUS_NATIVE_BUILDER_IMAGE=...` — override the builder image tag

The container produces binaries that run bare-metal on NixOS
(validated: dynamic linker resolves to nix-store glibc 2.40, marker
emits cleanly, RSS + bridge subprocess all work). M1 smoke for
http-server 8/8 cells confirms numbers match prior local-build runs
within noise.

First run downloads the mandrel image (~600 MB). Subsequent runs
reuse the cached image.

## What the full run produces

The harness writes one results dir under `benchmarks/results/<timestamp>/`:

```
m1-summary.csv          # cold-start + RSS for all cells
m2-round-0/             # M2 round 0
  <scenario>_<contender>/
    protocol-a-summary.txt   # T3 HTTP cells
    protocol-b-samples.txt   # T1/T2/T4 cells
    rss-samples.txt          # RSS walk samples
    m2-summary.txt           # parsed stats
  ...
m2-round-1/             # 5 rounds total
...
m2-summary.csv          # combined M2 stats with BCa CI
```

After the run, generate the v3 report:

```bash
# Manually assemble the v3 report from the results CSVs.
# Sections to cover (per spec §6):
#   1. Cold-start (M1) — same shape as v2 report
#   2. Warm p99 (M2 Protocol A) — NEW
#   3. Timer-driven p99 (M2 Protocol B) — NEW
#   4. RSS growth (M2 RSS samples) — NEW
#   5. Bridge tax (T4a/T4b vs T3 baseline) — NEW
#   6. ICP framings (data-driven from M1+M2)
# Save at: docs/benchmarks/2026-07-20-benchmark-v3.md
```

## Quality gates

After Task 5 produces results but BEFORE committing the report, run:

```bash
cargo fmt --check --all
cargo clippy --workspace --all-features \
  --exclude camel-cli \
  --exclude camel-component-kafka \
  --exclude security-keycloak \
  --exclude security-wasm-policy \
  -- -D warnings
cargo clippy -p camel-component-kafka --all-targets -- -D warnings
cargo clippy -p camel-cli -- -D warnings
cargo xtask lint-unwrap
cargo xtask lint-secrets
cargo xtask lint-log-levels
cargo xtask schema --check
cargo audit
```

No code changes are expected from Task 5 (measurement only). If a
gate fails, it's from the rebuild, not Task 5 — investigate before
proceeding.

## Disk + cleanup

- `/home` at 92% (6 GB free). The full run writes raw samples to
  `/tmp` (tmpfs), so disk usage stays roughly flat during the run.
- After the run, the results dir under `benchmarks/results/` is
  ~50–200 MB (mostly CSV + txt samples). Commit it.
- Optional cleanup: `find target -name 'incremental' -type d -exec rm -rf {} +`
  frees ~1 GB. Don't delete `target/` itself.

## Failure modes to watch

1. **`EADDRINUSE` on port 8080**: should not recur after `ea71de05`
   fix. If it does, check for orphan processes:
   ```bash
   ps -ef | grep -E 'camel run|http-server' | grep -v grep
   ss -lntp | grep :8080
   ```
   Kill orphans manually + retry.

2. **Bridge mTLS handshake failures** (T4a/T4b): bridge binary must
   exist at `bridges/xml/build/native/xml-bridge`. The harness checks
   this in pre-flight. If missing, rebuild with
   `cargo xtask build-xml-bridge`.

3. **Quarkus native runner missing**: harness pre-flight will exit
   with `runner not found`. Rebuild via the script at
   `/tmp/v3-verify/build-native.sh` (or similar).

4. **`warmup failed-stability: MessageBoundUnconverged`**: with
   `--warmup-msgs=100000` this should not occur. If it does, the
   cell's p99 is unstable — investigate (often GC pause in JVM cells).

## After the run completes

1. Inspect `m1-summary.csv` + `m2-summary.csv` for anomalies
   (negative values, missing cells, p99 > 10× p50).
2. Author `docs/benchmarks/2026-07-20-benchmark-v3.md` using the v2
   report as template (`docs/benchmarks/2026-07-18-benchmark-v2.md`).
3. Update `benchmarks/COVERAGE.md` with v3 cell states.
4. Update `benchmarks/CONTEXT.md` glossary if v3 introduced new terms
   (BCa CI, Protocol A/B, bridge tax).
5. Commit + close bd `rc-2vxg`:
   ```bash
   bd close rc-2vxg --reason "v3 complete: 5 scenarios × 2 metrics, N=50, BCa CI"
   ```

## Resuming after interruption

The harness does NOT support resume — each invocation starts from
scratch. If the run dies mid-way:

1. Check `/tmp/v3-task5-full.log` for the failure point.
2. Clean up any orphan processes (see failure mode #1).
3. Re-launch the same command.

For partial resume (skip already-measured scenarios), use
`--scenarios=<subset>` to limit which scenarios run.

## Reference

- Spec: `docs/superpowers/specs/2026-07-18-benchmark-v3-design.md`
- Plan: `docs/superpowers/plans/2026-07-18-benchmark-v3.md`
- Task 4.5 report (with bug verification section):
  `.superpowers/sdd/task-4.5-report.md`
- Progress ledger: `.superpowers/sdd/progress.md`
- v2 report (template): `docs/benchmarks/2026-07-18-benchmark-v2.md`
- COVERAGE.md: `benchmarks/COVERAGE.md`
