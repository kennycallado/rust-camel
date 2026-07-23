#!/usr/bin/env bash
# Startup benchmark harness v2 (bd rc-p9ki). Extends the v1 harness
# (bd rc-f3g9, closed) with: flag-based CLI, scenario iteration +
# auto-discovery, 16-cell shuffle (per spec §4.5 round-1 fix from
# e_gpt), fingerprint-based native-image caching, pre-flight checks,
# scenario-aware marker validation, and per-cell raw-sample retention.
# v1 protocol is inherited unchanged — see §"v1 protocol inheritance"
# below for the full preservation list.
#
# Usage:
#   ./run.sh [--scenarios=<comma-separated>] [--n=<count>] [--warmup=<count>] [--dry-run]
#
# Defaults: scenarios = auto-discover under benchmarks/scenarios/*/,
# n=50, warmup=3. --dry-run prints the 16-cell shuffled order and
# exits 0 without invoking any contender.
#
# Scenario auto-discovery excludes `benchmarks/spikes/` (lives outside
# scenarios/, so already excluded) AND any directory matching the
# `spike-*` glob — prevents future Task 1-style spike artifacts from
# polluting the measurement matrix.
#
# v1 protocol inheritance (no drift):
#   - n=50 + 3 warmup discarded; warmup failure is FATAL.
#   - 1ms polling for the marker.
#   - 30s startup deadline per cell.
#   - Exact marker-count validation: T1 expects `BENCH_ROUTE_READY`,
#     T2 expects `BENCH_ROUTE_READY body=pong-bench` (per spec §4.5;
#     the body suffix proves the choice/when branch executed). Zero,
#     multiple, or wrong-body markers are FATAL for that cell.
#   - Raw samples saved to benchmarks/results/<timestamp>/<cell>/samples.txt
#     for independent re-analysis.
#   - GNU time peak RSS via /run/current-system/sw/bin/time -v on
#     NixOS (parses `Maximum resident set size (kbytes)`).
#   - KILL immediately after the marker; cleanup is fatal-if-fails.
#   - Env-var hygiene: unset JAVA_TOOL_OPTIONS, _JAVA_OPTIONS,
#     JDK_JAVA_OPTIONS; LC_ALL=C.

set -euo pipefail

# =====================================================================
# Environment hygiene (v1, no drift)
# =====================================================================
unset JAVA_TOOL_OPTIONS _JAVA_OPTIONS JDK_JAVA_OPTIONS
export LC_ALL=C

# =====================================================================
# Constants
# =====================================================================
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCENARIOS_DIR="$REPO_ROOT/benchmarks/scenarios"

# Right-size the Serial GC max heap for every Quarkus/Mandrel native
# runner. Mandrel CE defaults the Serial GC max heap to 80% of physical
# RAM (~22 GiB on this host); each full GC single-threadedly traverses
# the whole heap, inflating throughput/latency tails for the T3/T4
# HTTP-server workloads. `-R:MaxHeapSize=512m` in application.properties
# is the build-time equivalent, but on Mandrel 23.1.11.0 that baked
# default is superseded at runtime by MaximumHeapSizePercent (verified:
# `runner -XX:PrintFlags=` reports MaxHeapSize=0 / MaximumHeapSizePercent=80
# even though `-R:MaxHeapSize=512m` is embedded in the binary). The
# GraalVM runtime `-Xmx` option unconditionally overrides the auto-sized
# default (GraalVM Native Image "Java Heap Size" — `-Xmx` is honored at
# image run time regardless of build-time defaults), and the harness owns
# the launch command, so setting it here is the deterministic fix. Placed
# BEFORE app args so the SubstrateVM runtime option parser consumes it.
NATIVE_HEAP_ARG="-Xmx512m"
RESULTS_ROOT="$REPO_ROOT/benchmarks/results"
TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="$RESULTS_ROOT/$TIMESTAMP"

# JAVA_HOME: required for host runs, but the Mandrel-based runner
# container sets it via the base image env. If unset, try common
# locations; if still unset, fail with a clear error pointing at
# both host and container paths.
if [[ -z "${JAVA_HOME:-}" ]]; then
    for candidate in "/opt/graalvm" "/usr/lib/jvm/java-21-graalvm" "/usr/lib/jvm/java-21-openjdk"; do
        if [[ -x "$candidate/bin/java" ]]; then
            JAVA_HOME="$candidate"
            break
        fi
    done
fi
if [[ -z "${JAVA_HOME:-}" ]]; then
    echo "error: JAVA_HOME is not set and no JDK 21 found at standard locations." >&2
    echo "       Host: export JAVA_HOME=/path/to/jdk21" >&2
    echo "       Container: use bash benchmarks/harness/run-all.sh (handles it)" >&2
    exit 1
fi
JAVA_BIN="$JAVA_HOME/bin/java"
NATIVE_IMAGE_BIN="$JAVA_HOME/bin/native-image"

# Gradle binary — honors GRADLE_BIN env (set by run-all.sh when running
# inside the Mandrel container). Host fallback: `command -v gradle` if
# on PATH, else /opt/gradle/bin/gradle (the container's install path).
GRADLE_BIN="${GRADLE_BIN:-$(command -v gradle 2>/dev/null || echo /opt/gradle/bin/gradle)}"

# Native build mode: prefer 'local' (direct native-image invocation)
# when native-image is callable — works inside the Mandrel container
# (native-image in $JAVA_HOME/bin) and on hosts with GraalVM CE 21+.
# Fall back to 'docker' container-runtime otherwise (host with plain JDK).
# Override explicitly via env var if needed.
if [[ -x "$NATIVE_IMAGE_BIN" ]] && "$NATIVE_IMAGE_BIN" --version >/dev/null 2>&1; then
    BENCH_NATIVE_MODE="${BENCH_NATIVE_MODE:-local}"
else
    BENCH_NATIVE_MODE="${BENCH_NATIVE_MODE:-docker}"
fi
QUARKUS_NATIVE_BUILDER_IMAGE="${QUARKUS_NATIVE_BUILDER_IMAGE:-quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21}"

# NATIVE_ZLIB_LINK: only needed on NixOS hosts (zlib lives in /nix/store/...).
# Inside the Mandrel container (UBI/FHS) or on a standard Linux host,
# zlib is at /usr/lib*/libz.so and the linker finds it without help.
# Dynamic resolution prevents the hardcoded hash from rotting.
if [[ -z "${NATIVE_ZLIB_LINK:-}" ]] \
   && [[ "$BENCH_NATIVE_MODE" == "local" ]] \
   && [[ -d /nix/store ]]; then
    ZLIB_LIB_DIR="$(nix path-info --quiet nixpkgs#zlib 2>/dev/null)/lib"
    if [[ -d "$ZLIB_LIB_DIR" ]]; then
        NATIVE_ZLIB_LINK="-H:NativeLinkerOption=-L$ZLIB_LIB_DIR"
    else
        # Fallback: probe the existing nix-store zlib path (slower but works
        # without nixpkgs#zlib installed in the profile).
        ZLIB_LIB_DIR="$(ls -d /nix/store/*-zlib-*/lib 2>/dev/null | head -1)"
        if [[ -d "$ZLIB_LIB_DIR" ]]; then
            NATIVE_ZLIB_LINK="-H:NativeLinkerOption=-L$ZLIB_LIB_DIR"
            echo "warning: using probed zlib path $ZLIB_LIB_DIR (fragile; install nixpkgs#zlib for stability)" >&2
        fi
    fi
fi
NATIVE_ZLIB_LINK="${NATIVE_ZLIB_LINK:-}"

# Per-scenario expected marker (spec §4.5). The T2 marker carries
# the post-choice body so a wrong-branch run (otherwise → "pong-other")
# is observable, not silent. T1 has no body, matching the v1 harness.
# T3 (http-server) emits a bare BENCH_ROUTE_READY — the same shape
# as T1's marker, with the unix_ms suffix parsed as a probe-only
# timestamp (not part of the marker contract).
declare -A SCENARIO_MARKER=(
    ["startup-minimal"]="BENCH_ROUTE_READY"
    ["t2-realistic-eip"]="BENCH_ROUTE_READY body=pong-bench"
    ["http-server"]="BENCH_ROUTE_READY"
    # T4a/T4b (bd rc-2vxg Task 4): bridge-tax scenarios. Marker is the
    # bare BENCH_ROUTE_READY token (same as T1/T3); emitted by the
    # rust-camel-lib main() / rust-camel-cli wrapper script / Java
    # EventNotifierSupport on RouteStarted.
    ["xslt-bridge"]="BENCH_ROUTE_READY"
    ["xsd-validation-bridge"]="BENCH_ROUTE_READY"
)

# Per-scenario artifact set (spec §4.3 asymmetric matrix).
# - "full" (default, also when entry is unset): 8 artifacts per scenario
#   (Pair A + Pair B × {standalone, quarkus-jvm, quarkus-native, rust}).
#   Used by T1/T2/T3.
# - "bridge": 4 artifacts per scenario (standalone-dsl, quarkus-dsl-native,
#   rust-camel-lib, rust-camel-cli). Bridge-tax measurement only needs the
#   DSL+native variants — the YAML variants add no bridge-tax signal since
#   the bridge is the same gRPC subprocess regardless of route authoring.
#   Used by T4a/T4b. The asymmetry (8 vs 4) is intentional and documented
#   in spec §4.3 / COVERAGE.md; do NOT expand T4a/T4b to 8 artifacts.
declare -A SCENARIO_ARTIFACT_SET=(
    ["xslt-bridge"]="bridge"
    ["xsd-validation-bridge"]="bridge"
)

# Protocol A URL for T3 http-server fixtures (Task 3 / spec §4.5).
# Client-side URL uses 127.0.0.1 (the server binds 0.0.0.0); the
# harness drives bench-loadgen against this URL for calibration +
# measure-a. Override via env var if a smoke run shifts the port.
BENCH_HTTP_URL="${BENCH_HTTP_URL:-http://127.0.0.1:8080/bench}"

# Per-scenario per-contender argv (set by resolve_scenario_artifacts).
# Each entry is one cell: scenario × contender. The 8 contenders per
# scenario are the same set for T1 and T2 (8 native-expanded artifacts
# from spec §4.3).
declare -a CELLS=()        # cell names: "<scenario>/<contender>"
declare -A CELL_SCENARIO=  # cell → scenario dir
declare -A CELL_ARGV=      # cell → space-separated argv (re-parsed on demand)
declare -A CELL_MARKER=    # cell → expected marker string
declare -A CELL_WONT_MEASURE=  # cell → human-readable skip reason (absent = measurable)
declare -a SCENARIOS=()    # ordered list of scenario names

# Protocol A state (T3 http-server): per-cell URL + the list of cells
# requiring Protocol A measurement. Populated by resolve_all_cells
# from SCENARIO_M2_PROTOCOL + the BENCH_HTTP_URL constant.
declare -A PROTOCOL_A_CELL_URL=()
declare -a PROTOCOL_A_CELLS=()

# =====================================================================
# Argument parsing (flag-based, v1 positional form REMOVED — v1 is
# closed, harness is internal tooling, no back-compat shim)
# =====================================================================
SCENARIOS_FILTER=""
N=50
WARMUP_N=3
DRY_RUN=false

# M2 defaults (spec §4.2 + §4.3 verbatim).
METRIC=m1+m2
M2_WARMUP_TIME=30
M2_WARMUP_MSGS=1000
M2_ROUNDS=5
M2_SAMPLES_PER_ROUND=10000
# BCa CI resamples (brief 2h: n=2000, 95% CI on per-round p50).
M2_BCI_RESAMPLES=2000

# M3/M4 defaults (sustained throughput + memory growth under load).
# Targets T3 http-server cells only (PROTOCOL_A_CELLS is the cell set;
# they are the contenders that bind 0.0.0.0:8080 and respond to HTTP).
M3_DURATION_SECS=50
M3_WARMUP_SECS=10
M3_ROUNDS=5

# First-success probe deadline (spec F2): how long to wait for the first
# BENCH_LATENCY record before declaring the cell unhealthy.
PROBE_DEADLINE_SECS=30

usage() {
    cat <<'EOF'
usage: run.sh [--scenarios=<csv>] [--n=<count>] [--warmup=<count>]
              [--metric=m1|m2|m1+m2|m3|m3+m4|m1+m2+m3+m4]
              [--warmup-time=<sec>] [--warmup-msgs=<n>]
              [--rounds=<n>] [--samples-per-round=<n>]
              [--dry-run]
  --scenarios         comma-separated list of scenario names under
                      benchmarks/scenarios/; default = auto-discover all
                      (excludes benchmarks/spikes/ and any `spike-*` pattern).
  --n                 M1 measured runs per cell (default 50).
  --warmup            M1 warmup runs per cell, discarded (default 3).
  --metric            m1, m2, m1+m2, m3, m3+m4, or m1+m2+m3+m4
                      (default m1+m2). m1 = v2 cold-start measurement;
                      m2 = v3 warm p99/service-time (spec §4.1);
                      m3 = sustained throughput (T3 http-server only);
                      m4 = memory growth under load (runs with m3).
  --warmup-time       M2 warmup wall-clock seconds per cell (default 30).
  --warmup-msgs       M2 warmup message bound per cell (default 1000).
  --rounds            M2 measurement rounds per cell (default 5).
  --samples-per-round M2 samples per round per cell (default 10000).
  --dry-run           print the 16-cell shuffled order and exit 0; do not
                      invoke any contender.

Positional arguments are NOT accepted (v1 positional form is closed).
EOF
}

# Reject any positional arg with an actionable error.
if [[ $# -gt 0 ]]; then
    first="$1"
    case "$first" in
        --scenarios=*|--n=*|--warmup=*|--metric=*|--warmup-time=*|--warmup-msgs=*|--rounds=*|--samples-per-round=*) : ;;  # flag, parse below
        --dry-run) : ;;                      # flag, parse below
        --help|-h) usage; exit 0 ;;
        --*) echo "error: unknown flag '$first'" >&2; usage >&2; exit 2 ;;
        *) echo "error: positional argument '$first' is not supported. The v1 positional form (./run.sh <scenario> [runs] [warmup-runs]) is closed. Use --scenarios=<name> --n=<n> --warmup=<n>." >&2; usage >&2; exit 2 ;;
    esac
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scenarios=*)         SCENARIOS_FILTER="${1#*=}" ;;
        --n=*)                 N="${1#*=}" ;;
        --warmup=*)            WARMUP_N="${1#*=}" ;;
        --metric=*)            METRIC="${1#*=}" ;;
        --warmup-time=*)       M2_WARMUP_TIME="${1#*=}" ;;
        --warmup-msgs=*)       M2_WARMUP_MSGS="${1#*=}" ;;
        --rounds=*)            M2_ROUNDS="${1#*=}" ;;
        --samples-per-round=*) M2_SAMPLES_PER_ROUND="${1#*=}" ;;
        --dry-run)             DRY_RUN=true ;;
        --help|-h)             usage; exit 0 ;;
        *)                     echo "error: unknown flag '$1'" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

# Validate numeric args (must be positive integers)
if ! [[ "$N" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: --n must be a positive integer (got '$N')" >&2; exit 2
fi
if ! [[ "$WARMUP_N" =~ ^[0-9]+$ ]]; then
    echo "error: --warmup must be a non-negative integer (got '$WARMUP_N')" >&2; exit 2
fi

# Validate --metric (brief 2j).
case "$METRIC" in
    m1|m2|m1+m2|m3|m3+m4|m1+m2+m3+m4) : ;;
    *) echo "error: --metric must be one of: m1, m2, m1+m2, m3, m3+m4, m1+m2+m3+m4 (got '$METRIC')" >&2; exit 2 ;;
esac

# Validate M2 numeric args.
if ! [[ "$M2_WARMUP_TIME" =~ ^[0-9]+$ ]]; then
    echo "error: --warmup-time must be a non-negative integer (got '$M2_WARMUP_TIME')" >&2; exit 2
fi
if ! [[ "$M2_WARMUP_MSGS" =~ ^[0-9]+$ ]]; then
    echo "error: --warmup-msgs must be a non-negative integer (got '$M2_WARMUP_MSGS')" >&2; exit 2
fi
if ! [[ "$M2_ROUNDS" =~ ^[0-9]+$ ]]; then
    echo "error: --rounds must be a non-negative integer (got '$M2_ROUNDS')" >&2; exit 2
fi
if ! [[ "$M2_SAMPLES_PER_ROUND" =~ ^[0-9]+$ ]]; then
    echo "error: --samples-per-round must be a non-negative integer (got '$M2_SAMPLES_PER_ROUND')" >&2; exit 2
fi

# Flags triggering per-cell T4a/T4b features (off by default; only
# activated for scenarios that register as bridge artifacts when those
# land in Task 3+).
METRIC_BRIDGE_TRACKING=false
METRIC_RSS_SAMPLE=false

# =====================================================================
# GNU time resolution (v1, no drift — NixOS lives at
# /run/current-system/sw/bin/time, FHS at /usr/bin/time; bash builtin
# time does NOT support -v)
# =====================================================================
TIME_BIN=""
for candidate in /usr/bin/time /run/current-system/sw/bin/time "$(command -v time 2>/dev/null)"; do
    [[ -z "$candidate" ]] && continue
    if "$candidate" -v true > /dev/null 2>&1; then
        TIME_BIN="$candidate"
        break
    fi
done
if [[ -z "$TIME_BIN" ]]; then
    echo "error: GNU time with -v support not found (tried /usr/bin/time, /run/current-system/sw/bin/time, \$(command -v time))" >&2
    echo "On NixOS: nix-shell -p time, or install the 'time' package." >&2
    exit 1
fi
echo "using GNU time: $TIME_BIN"

# =====================================================================
# Process-lifecycle state (v1, extended with CURRENT_BUILD_PID for the
# gradle process which is a long-running child of the harness)
# =====================================================================
CURRENT_TIME_PID=""
CURRENT_CHILD_PID=""
CURRENT_BUILD_PID=""
CURRENT_RSS_PID=""

# CPU topology / taskset pinning (T2: isolate server vs loadgen cores so
# a saturated contender cannot starve the loadgen and cap throughput).
# SERVER_AFFINITY / LOADGEN_AFFINITY are empty arrays by default — every
# launch site expands them, so they are no-ops until detect_cpu_topology
# populates them (only for M3 throughput runs).
SERVER_CPUSET=""
LOADGEN_CPUSET=""
LOADGEN_WORKERS=0
SERVER_AFFINITY=()
LOADGEN_AFFINITY=()
HOST_CPU_MODEL=""
HOST_CORE_COUNT=0
HOST_RAM_GB=0
DOCKER_IMAGE_ID=""

cleanup() {
    # v1 R2-Fix 2: always wait the time_pid, even if kill -0 says the
    # process is gone — avoids zombie/defunct entries that distort RSS
    # on later runs. Swallow wait's exit status (KILLed children return
    # non-zero, which is expected).
    if [[ -n "${CURRENT_CHILD_PID:-}" ]] && kill -0 "$CURRENT_CHILD_PID" 2>/dev/null; then
        # Tree-aware kill (bd rc-8ysn): the rust-camel-cli wrapper uses
        # `setsid`, so a bare TERM→KILL on the wrapper alone can orphan
        # the camel run child. The recursive walk reaches all descendants
        # while /proc children entries are still intact.
        _kill_process_tree_recursive "$CURRENT_CHILD_PID"
        wait "$CURRENT_CHILD_PID" 2>/dev/null || true
    fi
    if [[ -n "${CURRENT_TIME_PID:-}" ]]; then
        kill -KILL "$CURRENT_TIME_PID" 2>/dev/null || true
        wait "$CURRENT_TIME_PID" 2>/dev/null || true
    fi
    # Also reap the gradle build (long-running child of the harness
    # during native builds). Gradle has its own trap for child JVMs;
    # we only need to TERM the gradle process itself, not its daemon.
    if [[ -n "${CURRENT_BUILD_PID:-}" ]] && kill -0 "$CURRENT_BUILD_PID" 2>/dev/null; then
        kill -TERM "$CURRENT_BUILD_PID" 2>/dev/null || true
        sleep 0.5
        kill -KILL "$CURRENT_BUILD_PID" 2>/dev/null || true
        wait "$CURRENT_BUILD_PID" 2>/dev/null || true
    fi
    # Reap the M3/M4 rss-sample background subshell if it's still
    # running (interactive Ctrl-C during a measurement round).
    if [[ -n "${CURRENT_RSS_PID:-}" ]] && kill -0 "$CURRENT_RSS_PID" 2>/dev/null; then
        kill -KILL "$CURRENT_RSS_PID" 2>/dev/null || true
        wait "$CURRENT_RSS_PID" 2>/dev/null || true
    fi
    # C6: reap any bridge subprocess whose PID file exists. Ctrl-C
    # mid-measurement would otherwise leave the bridge (NOT a child of
    # the time wrapper) running on the host. Glob all known PID files
    # since we can't know which cell was active when the trap fired.
    local _pid_file
    for _pid_file in /tmp/v3-bridge-pid-*.txt; do
        [[ -f "$_pid_file" ]] || continue
        local _bpid
        _bpid="$(cat "$_pid_file" 2>/dev/null || true)"
        if [[ -n "$_bpid" ]] && kill -0 "$_bpid" 2>/dev/null; then
            _kill_process_tree_recursive "$_bpid"
        fi
        rm -f "$_pid_file"
    done
}
# v1 R2-Fix 2: INT/TERM must clean up AND exit; EXIT-only cleanup
# left the harness running after Ctrl-C with an orphaned child. The
# BUILD_PID guard is a no-op when no build is in flight.
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# Per-run scratch dir (v1: kept ephemeral; final results go to RUN_DIR
# only at the end so a Ctrl-C during measurement doesn't half-write).
SCRATCH_DIR="$(mktemp -d)"
echo "scratch dir: $SCRATCH_DIR"

# =====================================================================
# CPU topology detection + taskset pinning (T2)
# =====================================================================

# Expand a cpuset spec ("0-3,6,8-9") into the array named by $2.
_expand_cpuset() {
    local spec="$1"
    local -n _expand_arr="$2"
    _expand_arr=()
    [[ -z "$spec" ]] && return
    local part lo hi i
    local -a _parts=()
    IFS=',' read -ra _parts <<< "$spec"
    for part in "${_parts[@]}"; do
        [[ -z "$part" ]] && continue
        if [[ "$part" == *-* ]]; then
            lo="${part%-*}"; hi="${part#*-}"
            for ((i = lo; i <= hi; i++)); do _expand_arr+=("$i"); done
        else
            _expand_arr+=("$part")
        fi
    done
}

# Render a single "lo" / "lo-hi" range token.
_range_token() {
    if [[ "$1" == "$2" ]]; then echo "$1"; else echo "$1-$2"; fi
}

# Compress a list of integer CPU ids (argv) into a compact range string
# ("0 1 2 6 7 8" → "0-2,6-8"). Sorts + dedupes numerically.
_compress_cpuset() {
    local -a ids=("$@")
    [[ ${#ids[@]} -eq 0 ]] && { echo ""; return; }
    local -a sorted=()
    IFS=$'\n' sorted=($(printf '%s\n' "${ids[@]}" | sort -un))
    unset IFS
    local out="" start="${sorted[0]}" prev="${sorted[0]}" v i
    for ((i = 1; i < ${#sorted[@]}; i++)); do
        v="${sorted[$i]}"
        if (( v == prev + 1 )); then
            prev="$v"
            continue
        fi
        out+="${out:+,}$(_range_token "$start" "$prev")"
        start="$v"; prev="$v"
    done
    out+="${out:+,}$(_range_token "$start" "$prev")"
    echo "$out"
}

# detect_cpu_topology [server_physical_cores]
#
# Parse lscpu to get the (cpu, core, socket) mapping for online CPUs,
# intersect with the container's allowed CPU list, group by (socket,core)
# to find physical cores, then split: first half → SERVER, rest →
# LOADGEN. Optional $1 overrides the server physical-core count (used by
# the baseline-validation re-split). Populates SERVER_CPUSET,
# LOADGEN_CPUSET, LOADGEN_WORKERS, SERVER_AFFINITY, LOADGEN_AFFINITY and
# the HOST_* provenance facts. Fails closed on <6 physical cores or
# asymmetric SMT.
detect_cpu_topology() {
    local server_n="${1:-}"
    command -v lscpu >/dev/null 2>&1 || {
        echo "error: lscpu not found (util-linux); cannot detect CPU topology" >&2
        exit 1
    }
    command -v taskset >/dev/null 2>&1 || {
        echo "error: taskset not found (util-linux); cannot pin cores" >&2
        exit 1
    }

    # 1. Container-allowed CPUs (cgroup v2 first, v1 fallback, then this
    #    process's own affinity).
    local allowed_cpus=""
    if [[ -r /sys/fs/cgroup/cpuset.cpus.effective ]]; then
        allowed_cpus="$(cat /sys/fs/cgroup/cpuset.cpus.effective)"
    fi
    if [[ -z "$allowed_cpus" ]]; then
        allowed_cpus="$(cat /sys/fs/cgroup/cpuset/cpuset.cpus 2>/dev/null || true)"
    fi
    if [[ -z "$allowed_cpus" ]]; then
        allowed_cpus="$(taskset -cp $$ 2>/dev/null | sed 's/.*: //')"
    fi
    local -a allowed=()
    _expand_cpuset "$allowed_cpus" allowed
    if [[ ${#allowed[@]} -eq 0 ]]; then
        echo "error: detect_cpu_topology: no allowed CPUs resolved (cpuset.cpus.effective empty, taskset fallback failed)" >&2
        exit 1
    fi
    local -A allowed_set=()
    local c
    for c in "${allowed[@]}"; do allowed_set[$c]=1; done

    # 2. Parse lscpu -p (skip comment lines beginning with '#').
    local lscpu_out
    lscpu_out="$(lscpu -p=CPU,CORE,SOCKET,ONLINE 2>/dev/null || true)"
    if [[ -z "$lscpu_out" ]]; then
        echo "error: detect_cpu_topology: lscpu -p produced no output" >&2
        exit 1
    fi

    # core_map[key] → space-separated logical CPU ids (SMT siblings).
    # all_keys[] preserves every discovered (socket:core) physical core.
    local -A core_map=()
    local -a all_keys=()
    local line cpu core socket online key
    while IFS= read -r line; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        IFS=',' read -r cpu core socket online <<< "$line"
        [[ "$online" == "Y" || "$online" == "1" ]] || continue
        [[ -n "${allowed_set[$cpu]:-}" ]] || continue
        key="$socket:$core"
        if [[ -z "${core_map[$key]:-}" ]]; then
            core_map[$key]="$cpu"
            all_keys+=("$key")
        else
            core_map[$key]+=" ${cpu}"
        fi
    done <<< "$lscpu_out"

    local n_physical=${#all_keys[@]}
    if (( n_physical < 6 )); then
        echo "error: detect_cpu_topology: only $n_physical physical cores available (need >=6 to split server/loadgen)" >&2
        echo "       allowed cpuset: $allowed_cpus" >&2
        exit 1
    fi

    # 3. Asymmetric-SMT guard: every physical core must have the same
    #    number of logical siblings, otherwise the split is unbalanced.
    local first_sibs="" sibs
    for key in "${all_keys[@]}"; do
        sibs="$(wc -w <<< "${core_map[$key]}")"
        if [[ -z "$first_sibs" ]]; then
            first_sibs="$sibs"
        elif [[ "$sibs" != "$first_sibs" ]]; then
            echo "error: detect_cpu_topology: asymmetric SMT topology (core $key has $sibs siblings, expected $first_sibs)" >&2
            exit 1
        fi
    done

    # 4. Deterministic split: sort physical cores by (socket, core).
    local -a sorted_keys=()
    IFS=$'\n' sorted_keys=($(printf '%s\n' "${all_keys[@]}" | sort -t: -k1,1n -k2,2n))
    unset IFS

    local half
    if [[ -n "$server_n" ]]; then
        half="$server_n"
    else
        half=$(( n_physical / 2 ))
    fi
    if (( half < 1 || (n_physical - half) < 2 )); then
        echo "error: detect_cpu_topology: invalid split (server=$half, loadgen=$((n_physical - half)))" >&2
        exit 1
    fi

    local -a server_cores=() loadgen_cores=()
    local idx
    for ((idx = 0; idx < n_physical; idx++)); do
        key="${sorted_keys[$idx]}"
        if (( idx < half )); then
            server_cores+=(${core_map[$key]})
        else
            loadgen_cores+=(${core_map[$key]})
        fi
    done

    SERVER_CPUSET="$(_compress_cpuset "${server_cores[@]}")"
    LOADGEN_CPUSET="$(_compress_cpuset "${loadgen_cores[@]}")"
    LOADGEN_WORKERS="${#loadgen_cores[@]}"

    HOST_CPU_MODEL="$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -1)"
    HOST_CPU_MODEL="${HOST_CPU_MODEL:-unknown}"
    HOST_CORE_COUNT="$n_physical"
    HOST_RAM_GB="$(free -g 2>/dev/null | awk '/^Mem:/{print $2; exit}')"
    HOST_RAM_GB="${HOST_RAM_GB:-0}"
    DOCKER_IMAGE_ID="${BENCH_DOCKER_IMAGE:-}"
    if [[ -z "$DOCKER_IMAGE_ID" ]]; then
        if [[ -f /.dockerenv ]]; then
            DOCKER_IMAGE_ID="container:$(uname -n)"
        else
            DOCKER_IMAGE_ID="host:$(uname -n)"
        fi
    fi

    SERVER_AFFINITY=(taskset -c "$SERVER_CPUSET")
    LOADGEN_AFFINITY=(taskset -c "$LOADGEN_CPUSET")

    echo "topology: $n_physical physical cores (SMT×$first_sibs) → server=$SERVER_CPUSET ($half cores) loadgen=$LOADGEN_CPUSET ($((n_physical - half)) cores, $LOADGEN_WORKERS workers)"
}

# Write $RUN_DIR/provenance.json (T2: full reproducibility record).
write_provenance() {
    local git_commit git_dirty iso kernel_version loadgen_ver bash_ver
    # Prefer the host-supplied commit/dirty state (the container may not
    # have the .git dir); fall back to querying the repo directly.
    git_commit="${BENCH_GIT_COMMIT:-}"
    if [[ -z "$git_commit" ]]; then
        git_commit="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
    fi
    if [[ -n "${BENCH_GIT_DIRTY:-}" ]]; then
        git_dirty="$BENCH_GIT_DIRTY"
    elif git -C "$REPO_ROOT" diff --quiet HEAD 2>/dev/null; then
        git_dirty=false
    else
        git_dirty=true
    fi
    iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    kernel_version="$(uname -r 2>/dev/null || echo unknown)"
    bash_ver="${BASH_VERSION:-unknown}"
    # Best-effort loadgen version (no hard dependency — provenance must
    # never fail the run). The bench-loadgen CLI has no --version, so we
    # record the resolved binary path as the version identifier.
    loadgen_ver="unknown"
    local _lg_bin
    _lg_bin="$(resolve_loadgen_bin 2>/dev/null || true)"
    if [[ -n "$_lg_bin" && -x "$_lg_bin" ]]; then
        loadgen_ver="$_lg_bin"
    fi
    jq -n \
        --arg     git_commit      "$git_commit" \
        --argjson git_dirty       "$git_dirty" \
        --arg     date            "$iso" \
        --arg     host_cpu        "$HOST_CPU_MODEL" \
        --argjson host_cores      "$HOST_CORE_COUNT" \
        --argjson host_ram        "$HOST_RAM_GB" \
        --arg     docker_image    "$DOCKER_IMAGE_ID" \
        --arg     server_cpuset   "$SERVER_CPUSET" \
        --arg     loadgen_cpuset  "$LOADGEN_CPUSET" \
        --argjson loadgen_workers "$LOADGEN_WORKERS" \
        --argjson smt_factor      "$HOST_CORE_COUNT" \
        --arg     kernel_version  "$kernel_version" \
        --arg     bash_version    "$bash_ver" \
        --arg     loadgen_version "$loadgen_ver" \
        --arg     raw_results_dir "$RUN_DIR" \
        '{
            git_commit: $git_commit,
            git_dirty: $git_dirty,
            date: $date,
            host_cpu: $host_cpu,
            host_cores: $host_cores,
            host_ram_gb: $host_ram,
            docker_image: $docker_image,
            kernel_version: $kernel_version,
            tool_versions: {
                bash: $bash_version,
                loadgen: $loadgen_version
            },
            topology: {
                physical_cores: $host_cores,
                server_cores: $server_cpuset,
                loadgen_cores: $loadgen_cpuset
            },
            server_cpuset: $server_cpuset,
            loadgen_cpuset: $loadgen_cpuset,
            loadgen_workers: $loadgen_workers,
            raw_results_note: ("raw result artifacts are gitignored and live only on the measurement host at " + $raw_results_dir)
        }' > "$RUN_DIR/provenance.json"
    echo "provenance: $RUN_DIR/provenance.json"
}

# Loopback baseline (T2): run bench-devnull pinned to SERVER_CPUSET, drive
# it with measure-throughput pinned to LOADGEN_CPUSET for ~10s. Echoes the
# resulting throughput (req/s) on stdout. This is the upper bound on what
# the loadgen can push through the kernel TCP stack alone — if a real
# contender approaches it, the loadgen is saturated and the measurement
# is loadgen-capped rather than contender-capped.
bench_devnull_baseline() {
    local devnull_bin loadgen_bin out_file dn_pid port
    devnull_bin="$(resolve_devnull_bin)"
    loadgen_bin="$(resolve_loadgen_bin)"
    out_file="$SCRATCH_DIR/devnull-baseline.$(date +%s%N).json"
    port="$(printf '%s' "$BENCH_HTTP_URL" | sed -n 's|.*://[^/:]*:\([0-9][0-9]*\).*|\1|p')"
    port="${port:-8080}"

    BENCH_DEVNULL_PORT="$port" "${SERVER_AFFINITY[@]}" "$devnull_bin" \
        > "$SCRATCH_DIR/devnull-baseline.out" 2>&1 &
    dn_pid=$!

    local deadline=$((SECONDS + 15))
    until grep -q 'BENCH_DEVNULL_READY' "$SCRATCH_DIR/devnull-baseline.out" 2>/dev/null; do
        if ! kill -0 "$dn_pid" 2>/dev/null; then
            echo "error: bench_devnull_baseline: server exited before ready" >&2
            echo "0"
            return 1
        fi
        if (( SECONDS >= deadline )); then
            echo "error: bench_devnull_baseline: server not ready within 15s" >&2
            kill -KILL "$dn_pid" 2>/dev/null || true
            wait "$dn_pid" 2>/dev/null || true
            echo "0"
            return 1
        fi
        sleep 0.05
    done

    "${LOADGEN_AFFINITY[@]}" "$loadgen_bin" measure-throughput \
        --url="$BENCH_HTTP_URL" --duration-secs=10 --warmup-secs=2 \
        --out="$out_file" 2>/dev/null || true

    kill -KILL "$dn_pid" 2>/dev/null || true
    wait "$dn_pid" 2>/dev/null || true
    sleep 0.3   # release the listener port

    local rps
    rps="$(jq -r '.mean_msgs_per_sec // 0' "$out_file" 2>/dev/null || echo 0)"
    echo "$rps"
}

# Max median throughput across all measured m3-summary.json cells that did
# not fail. Used by baseline validation to size the loadgen headroom.
fastest_contender_throughput() {
    local best=0 f v status
    for f in "$RUN_DIR"/*/m3-summary.json; do
        [[ -f "$f" ]] || continue
        status="$(jq -r '.status // "ok"' "$f" 2>/dev/null || echo ok)"
        [[ "$status" == "failed" ]] && continue
        v="$(jq -r '.median_mean_msgs_per_sec // 0' "$f" 2>/dev/null || echo 0)"
        if awk -v a="$v" -v b="$best" 'BEGIN{exit !(a>b)}'; then
            best="$v"
        fi
    done
    echo "$best"
}

# baseline_below <baseline> <fastest>: returns 0 if baseline < 1.25×fastest.
baseline_below() {
    awk -v b="$1" -v f="$2" 'BEGIN{ exit !(b < 1.25*f) }'
}

# =====================================================================
# Scenario discovery (4a: auto-discover + spike exclusion)
# =====================================================================
discover_scenarios() {
    local -a found=()
    if [[ -n "$SCENARIOS_FILTER" ]]; then
        # Explicit list — each name must resolve under SCENARIOS_DIR.
        IFS=',' read -ra wanted <<< "$SCENARIOS_FILTER"
        for name in "${wanted[@]}"; do
            # Trim whitespace from CSV tokens.
            name="${name// /}"
            if [[ -z "$name" ]]; then continue; fi
            # Reject spike-* patterns even in explicit lists (defence
            # in depth — a --scenarios=spike-foo run should fail, not
            # silently include spike artifacts).
            if [[ "$name" == spike-* ]]; then
                echo "error: scenario '$name' is excluded (matches spike-* pattern)" >&2
                exit 1
            fi
            if [[ ! -d "$SCENARIOS_DIR/$name" ]]; then
                echo "error: no such scenario directory: $SCENARIOS_DIR/$name" >&2
                exit 1
            fi
            found+=("$name")
        done
    else
        # Auto-discover: list immediate child dirs of SCENARIOS_DIR,
        # exclude `spike-*` and any other known noise.
        for path in "$SCENARIOS_DIR"/*/; do
            [[ -d "$path" ]] || continue
            local name
            name="$(basename "$path")"
            # Exclusion rules (plan §4a + spike-results.md "Next steps"):
            if [[ "$name" == spike-* ]]; then continue; fi
            found+=("$name")
        done
        if [[ ${#found[@]} -eq 0 ]]; then
            echo "error: no scenarios found under $SCENARIOS_DIR (auto-discovery yielded 0)" >&2
            echo "       pass --scenarios=<name> to specify, or create a scenario dir." >&2
            exit 1
        fi
    fi
    # Sort for deterministic ordering across runs (the shuffle is then
    # applied to the per-cell sequence, not the scenario list, so
    # sort is purely cosmetic).
    IFS=$'\n' sorted=($(printf '%s\n' "${found[@]}" | sort))
    unset IFS
    SCENARIOS=("${sorted[@]}")
}

# =====================================================================
# Per-scenario artifact resolution (4a + 4c)
#
# For each scenario, resolve the 8 artifacts (spec §4.3): 6 existing +
# 2 native. The fixture LAYOUT is shared across scenarios (same
# subdir names); per-scenario variables capture the differences
# (e.g. rust-camel-lib bin name = scenario name).
# =====================================================================

# Resolve the rust-camel-lib binary: by convention the bin name
# matches the scenario name (startup-minimal → bin `startup-minimal`,
# t2-realistic-eip → bin `t2-realistic-eip`). Verified against
# Cargo.toml [[bin]] name in the v1 and T2 fixtures.
resolve_rust_lib_bin() {
    local scenario_dir="$1" scenario_name="$2"
    local bin="$scenario_dir/rust-camel-lib/target/release/$scenario_name"
    if [[ ! -x "$bin" ]]; then
        echo "error: rust-camel-lib binary not found or not executable: $bin" >&2
        echo "       (expected bin name = scenario name per fixture convention)" >&2
        exit 1
    fi
    echo "$bin"
}

# Resolve the camel-standalone jar: glob target/*-jar-with-dependencies.jar
# (the assembly plugin names it <artifactId>-<version>-jar-with-dependencies.jar,
# where artifactId varies per scenario — e.g. T1 has `camel-standalone-dsl`,
# T2 has `t2-realistic-eip-camel-standalone-dsl`).
resolve_standalone_jar() {
    local subdir="$1"
    local jar
    jar="$(ls "$subdir"/target/*-jar-with-dependencies.jar 2>/dev/null | head -1 || true)"
    if [[ -z "$jar" || ! -f "$jar" ]]; then
        echo "error: camel-standalone jar not found under $subdir/target/ (expected exactly one *-jar-with-dependencies.jar)" >&2
        exit 1
    fi
    echo "$jar"
}

# Resolve cargo target dir for the root workspace (Pair B rust-camel-cli
# binary lives here). v1 noted: CARGO_TARGET_DIR may be set on the
# host (NixOS shared CI cache); we unset it before invoking
# `cargo metadata` to get the workspace's true target_directory.
resolve_cargo_target_dir() {
    local dir="$1"
    (cd "$dir" && env -u CARGO_TARGET_DIR cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | jq -r '.target_directory')
}

# Resolve the shared JVM sibling src/ for a native Gradle subproject.
# Parses the `java.srcDir("../<sibling>/src/main/java")` line out of
# the native subproject's build.gradle.kts. Returns the absolute path
# of the JVM sibling's src/main/ (java + resources merged).
resolve_shared_src_main() {
    local native_subproject_dir="$1"
    local build_gradle="$native_subproject_dir/build.gradle.kts"
    if [[ ! -f "$build_gradle" ]]; then
        echo "error: native subproject has no build.gradle.kts: $build_gradle" >&2
        exit 1
    fi
    # Look for the first `srcDir("../<sibling>/src/main/java")`
    # line and derive the JVM sibling's src/main/ from it. Handles both
    # `java.srcDir("...")` (single-line) and `srcDir("...")` (inside a
    # `java { }` block with exclude filters) sourceSets formats.
    local rel
    rel="$(grep -oP 'srcDir\("\.\./[^"]+src/main/java"\)' "$build_gradle" | head -1 \
            | sed -E 's|srcDir\("\.\./([^"]+)/src/main/java"\)|\1|')"
    if [[ -z "$rel" ]]; then
        # No srcDir found — the subproject is self-contained (all
        # sources are local). Return the local src/main directory.
        if [[ -d "$native_subproject_dir/src/main" ]]; then
            abs="$(cd "$native_subproject_dir/src/main" && pwd)"
            echo "$abs"
            return
        fi
        echo "error: could not parse srcDir and no local src/main found in $build_gradle" >&2
        exit 1
    fi
    local abs
    abs="$(cd "$native_subproject_dir/../$rel/src/main" && pwd)"
    echo "$abs"
}

# Resolve the JVM sibling's build.gradle.kts path (sibling of the
# shared src/main/). Returns the absolute path of the file. Note
# shared_src_main ends in /src/main, so two levels up is the
# sibling subproject root.
resolve_sibling_gradle() {
    local shared_src_main="$1"
    local sibling_dir
    sibling_dir="$(cd "$shared_src_main/../.." && pwd)"
    local sibling_gradle="$sibling_dir/build.gradle.kts"
    if [[ ! -f "$sibling_gradle" ]]; then
        echo "error: JVM sibling's build.gradle.kts not found: $sibling_gradle" >&2
        exit 1
    fi
    echo "$sibling_gradle"
}

# Portable glob-existence check (replacement for bash's `compgen -G`,
# which is missing in the NixOS bash 5.3 build). Populates the named
# array variable passed as $1 with the glob matches (or empty).
# Returns 0 if at least one match, 1 otherwise.
glob_exists() {
    local -n _out_arr=$1
    shift
    # Use nullglob so unmatched globs expand to nothing, not the
    # literal pattern. Disable the nullglob side-effect after the
    # expansion so the rest of the script is unaffected.
    local prev_nullglob
    prev_nullglob="$(shopt -p nullglob 2>/dev/null || true)"
    shopt -s nullglob
    _out_arr=( "$@" )
    eval "$prev_nullglob" 2>/dev/null || shopt -u nullglob
    [[ ${#_out_arr[@]} -gt 0 ]]
}

# Compute fingerprint for a native artifact (4c). Hashes all inputs
# that could change the resulting native binary, in a deterministic
# order, and writes the hash to <native-subproject>/.bench-fingerprint.
# Returns 0 if fingerprint matches the existing one (cache hit), 1
# if it doesn't (cache miss — rebuild needed).
compute_and_check_fingerprint() {
    local native_dir="$1" shared_src_main="$2" sibling_gradle="$3" \
          settings_gradle="$4" gradle_dir="$5" \
          app_props="$6" \
          fp_file="$native_dir/.bench-fingerprint"

    # Build a manifest of (path, content) for every input that could
    # change the native binary. The manifest is hashed with sha256sum;
    # the digest becomes the fingerprint. Sources are sorted so the
    # fingerprint is stable across runs.
    local manifest=""
    manifest+="=== JAVA_HOME === $JAVA_HOME"$'\n'
    manifest+="=== build mode === $BENCH_NATIVE_MODE"$'\n'
    manifest+="=== builder image === $QUARKUS_NATIVE_BUILDER_IMAGE"$'\n'
    if [[ "$BENCH_NATIVE_MODE" == "local" ]]; then
        manifest+="=== native-image --version ==="$'\\n'
        manifest+="$("$NATIVE_IMAGE_BIN" --version 2>&1 || echo MISSING)"$'\n'
    else
        # Container build — native-image version lives inside the builder
        # image, captured implicitly via the image digest. Including the
        # image tag in the manifest (above) is sufficient to invalidate
        # the cache when the image is upgraded.
        manifest+="=== native-image --version === container (skipped)"$'\n'
    fi

    # 1) All files under the shared JVM sibling src/main/ (recursive).
    if [[ -d "$shared_src_main" ]]; then
        manifest+="=== shared src/main ==="$'\\n'
        while IFS= read -r f; do
            manifest+="$f"$'\t'
            manifest+="$(sha256sum "$f" 2>/dev/null | awk '{print $1}')"$'\n'
        done < <(find "$shared_src_main" -type f | LC_ALL=C sort)
    fi

    # 2) Native subproject's own build.gradle.kts.
    if [[ -f "$native_dir/build.gradle.kts" ]]; then
        manifest+="=== native build.gradle.kts ==="$'\\n'
        manifest+="$(sha256sum "$native_dir/build.gradle.kts" | awk '{print $1}')"$'\n'
    fi

    # 3) Parent settings.gradle.kts.
    if [[ -f "$settings_gradle" ]]; then
        manifest+="=== settings.gradle.kts ==="$'\\n'
        manifest+="$(sha256sum "$settings_gradle" | awk '{print $1}')"$'\n'
    fi

    # 4) JVM sibling's build.gradle.kts.
    if [[ -f "$sibling_gradle" ]]; then
        manifest+="=== sibling build.gradle.kts ==="$'\\n'
        manifest+="$(sha256sum "$sibling_gradle" | awk '{print $1}')"$'\n'
    fi

    # 5) Native subproject's own application.properties.
    if [[ -f "$app_props" ]]; then
        manifest+="=== application.properties ==="$'\\n'
        manifest+="$(sha256sum "$app_props" | awk '{print $1}')"$'\n'
    fi

    # 6) gradle/ wrapper directory contents.
    if [[ -d "$gradle_dir" ]]; then
        manifest+="=== gradle/ ==="$'\\n'
        while IFS= read -r f; do
            manifest+="$f"$'\t'
            manifest+="$(sha256sum "$f" 2>/dev/null | awk '{print $1}')"$'\n'
        done < <(find "$gradle_dir" -type f | LC_ALL=C sort)
    fi

    # Hash the manifest. Print the digest to stdout (caller captures).
    local fp
    fp="$(printf '%s' "$manifest" | sha256sum | awk '{print $1}')"

    if [[ -f "$fp_file" ]] && [[ "$(cat "$fp_file")" == "$fp" ]]; then
        return 0   # cache hit
    fi
    # Cache miss — write new fingerprint for next time. The runner
    # existence is checked separately (post-build).
    printf '%s' "$fp" > "$fp_file"
    return 1
}

# Build a native Quarkus artifact (4c + 4d). Skips the build if the
# fingerprint is unchanged AND the runner exists. Runs the harness
# gradle binary directly (wrapper download fails on this network —
# see spike-results.md "Toolchain" section).
build_native_artifact() {
    local subproject="$1" camel_quarkus_dir="$2" runner_glob="$3"

    local native_dir="$camel_quarkus_dir/$subproject"
    local settings_gradle="$camel_quarkus_dir/settings.gradle.kts"
    local gradle_dir="$camel_quarkus_dir/gradle"
    local app_props="$native_dir/src/main/resources/application.properties"
    local shared_src_main
    shared_src_main="$(resolve_shared_src_main "$native_dir")"
    local sibling_gradle
    sibling_gradle="$(resolve_sibling_gradle "$shared_src_main")"

    # Pre-flight (4d): YAML-native's application.properties must
    # contain quarkus.native.resources.includes=camel/routes.yaml.
    # Also must NOT contain empty -Dquarkus.native.builder-image= or
    # -Dquarkus.native.container-runtime= (those are Quarkus 3.20
    # errors per spike-results.md).
    if [[ "$subproject" == *yaml* ]]; then
        if [[ ! -f "$app_props" ]] \
            || ! grep -q '^quarkus\.native\.resources\.includes=.*routes\.yaml' "$app_props"; then
            echo "error: pre-flight failed: $app_props must contain 'quarkus.native.resources.includes=...routes.yaml' (substrate VM cannot enumerate camel/* wildcard)" >&2
            exit 1
        fi
    fi

    # Compute fingerprint and decide whether to skip the build.
    local cached=false
    if compute_and_check_fingerprint "$native_dir" "$shared_src_main" \
                                     "$sibling_gradle" "$settings_gradle" \
                                     "$gradle_dir" "$app_props"; then
        # Fingerprint matched. Verify the runner binary exists; if
        # not (partial state from a previous failed build), rebuild.
        local matches=()
        if glob_exists matches $runner_glob; then
            echo "  fingerprint match + runner present, skipping build for $subproject"
            cached=true
        else
            echo "  fingerprint match but runner missing at $runner_glob — rebuilding"
        fi
    else
        echo "  fingerprint changed, building $subproject..."
    fi

    if [[ "$cached" == "false" ]]; then
        # Run the build in the foreground (sequential — builds share
        # the same gradle daemon, so backgrounding across artifacts
        # would race on .gradle/). Tracked via CURRENT_BUILD_PID so
        # Ctrl-C kills it cleanly.
        if [[ "$BENCH_NATIVE_MODE" == "local" ]]; then
            ( cd "$camel_quarkus_dir" \
                && JAVA_HOME="$JAVA_HOME" PATH="$JAVA_HOME/bin:$PATH" \
                   "$GRADLE_BIN" ":$subproject:build" \
                       -Dquarkus.native.enabled=true \
                       -Dquarkus.package.jar.enabled=false \
                       -Dquarkus.native.additional-build-args="$NATIVE_ZLIB_LINK" \
                       --no-daemon \
                ) &
        else
            # Container build (default since v3 spike 2026-07-20).
            # The native-image step runs inside $QUARKUS_NATIVE_BUILDER_IMAGE;
            # host JDK only needs to be JDK 21 (any vendor) for the
            # gradle invocation itself.
            ( cd "$camel_quarkus_dir" \
                && JAVA_HOME="$JAVA_HOME" PATH="$JAVA_HOME/bin:$PATH" \
                   "$GRADLE_BIN" ":$subproject:build" \
                       -Dquarkus.native.enabled=true \
                       -Dquarkus.package.jar.enabled=false \
                       -Dquarkus.native.container-runtime="$BENCH_NATIVE_MODE" \
                       -Dquarkus.native.builder-image="$QUARKUS_NATIVE_BUILDER_IMAGE" \
                       --no-daemon \
                ) &
        fi
        CURRENT_BUILD_PID=$!
        # `wait $!` returns the exit code of the build.
        local rc=0
        wait "$CURRENT_BUILD_PID" || rc=$?
        CURRENT_BUILD_PID=""
        if [[ $rc -ne 0 ]]; then
            echo "error: native build for $subproject failed with exit code $rc" >&2
            exit 1
        fi

        # Pre-flight post-build (4d check 3): output glob resolvable.
        local post_matches=()
        if ! glob_exists post_matches $runner_glob; then
            echo "error: pre-flight failed: native build for $subproject did not produce any file matching $runner_glob" >&2
            exit 1
        fi
    fi
}

# Pre-flight (4d, pre-build phase): toolchain sanity. With container
# builds (default since v3 spike 2026-07-20), the host JDK only needs
# to be JDK 21 (any vendor) for gradle invocation; the native-image
# step runs inside $QUARKUS_NATIVE_BUILDER_IMAGE. With BENCH_NATIVE_MODE=local,
# the host JDK MUST be GraalVM CE 21.0.2 (the build will silently fall
# back to JVM mode otherwise).
preflight_toolchain() {
    if [[ ! -x "$JAVA_BIN" ]]; then
        echo "error: pre-flight failed: JAVA_HOME/bin/java not found at $JAVA_BIN" >&2
        echo "       set JAVA_HOME to a JDK 21 install (GraalVM CE 21.0.2 for local native builds," >&2
        echo "       any JDK 21 for container builds)" >&2
        exit 1
    fi
    local java_version_output
    java_version_output="$("$JAVA_BIN" -version 2>&1 || true)"
    if [[ "$BENCH_NATIVE_MODE" == "local" ]]; then
        # Local native build mode: rely on $NATIVE_IMAGE_BIN being callable
        # as the capability gate (checked below). The java -version output
        # is not a reliable indicator — Mandrel-23.x JDK binary reports as
        # "Temurin" (Eclipse Adoptium), only native-image --version says Mandrel.
        # Old regex check on java -version output deleted (round-2 review
        # critical #1 follow-up: the regex 'GraalVM|Mandrel|Red.Hat' still
        # failed because Temurin matches none of those tokens).
        if [[ ! -x "$NATIVE_IMAGE_BIN" ]]; then
            echo "error: pre-flight failed: native-image not found at $NATIVE_IMAGE_BIN" >&2
            echo "       (GraalVM/Mandrel 21+ ships native-image in \$JAVA_HOME/bin; if missing, the install is incomplete)" >&2
            exit 1
        fi
        if ! "$NATIVE_IMAGE_BIN" --version > /dev/null 2>&1; then
            echo "error: pre-flight failed: native-image --version failed (substrate VM is broken or missing)" >&2
            exit 1
        fi
    else
        # Container build: host JDK just needs to be JDK 21 (for gradle).
        if ! grep -qE 'version "(2[1-9]|[3-9][0-9])\.' <<< "$java_version_output"; then
            echo "error: pre-flight failed: \$JAVA_HOME is not JDK 21+:" >&2
            echo "$java_version_output" >&2
            echo "       container builds still need a host JDK 21 for gradle invocation" >&2
            exit 1
        fi
        # Verify container runtime is available.
        if ! command -v "$BENCH_NATIVE_MODE" > /dev/null 2>&1; then
            echo "error: pre-flight failed: BENCH_NATIVE_MODE='$BENCH_NATIVE_MODE' not found in PATH" >&2
            echo "       install $BENCH_NATIVE_MODE or set BENCH_NATIVE_MODE=local" >&2
            exit 1
        fi
    fi
    if [[ ! -x "$GRADLE_BIN" ]]; then
        echo "error: pre-flight failed: gradle binary not found at $GRADLE_BIN" >&2
        echo "       override with GRADLE_BIN=/path/to/gradle if the local distribution moved" >&2
        exit 1
    fi
}

# Resolve the 4-artifact bridge scenario cell set (T4a xslt-bridge +
# T4b xsd-validation-bridge). Per spec §4.3 the bridge-tax matrix is
# asymmetric: only standalone-dsl + quarkus-dsl-native + rust-camel-lib
# + rust-camel-cli are measured. The bridge binary (bridges/xml) is
# shared across all 4 cells; rust-camel fixtures spawn it lazily on the
# first xslt:/validator: tick via gRPC mTLS (the "bridge tax" surface).
#
# The argv for each cell embeds the per-cell latency + PID file paths
# so the wrapper / fixture writes to the same /tmp/v3-* paths the M2
# protocol-B reader expects (cell-safe name = scenario_contender, the
# harness's existing convention).
#
# Args: scenario scenario_dir marker rust_cli_bin
resolve_bridge_scenario_cells() {
    local scenario="$1" scenario_dir="$2" marker="$3" rust_cli_bin="$4"

    # -- Shared assets (bridge binary + per-scenario shared/* files) --
    local bridge_binary="$REPO_ROOT/bridges/xml/build/native/xml-bridge"
    if [[ ! -x "$bridge_binary" ]]; then
        echo "error: bridge binary not found at $bridge_binary" >&2
        echo "       build with: cargo xtask build-xml-bridge" >&2
        exit 1
    fi
    local bridge_wrapper="$scenario_dir/shared/bridge-wrapper.sh"
    if [[ ! -x "$bridge_wrapper" ]]; then
        echo "error: bridge wrapper script not found or not executable: $bridge_wrapper" >&2
        exit 1
    fi
    local shared_payload="$scenario_dir/shared/bench-payload.xml"
    local shared_stylesheet="$scenario_dir/shared/identity-transform.xsl"
    local shared_schema="$scenario_dir/shared/schema.xsd"
    if [[ ! -f "$shared_payload" ]]; then
        echo "error: shared payload not found: $shared_payload" >&2
        exit 1
    fi
    # Scenario distinguishes the "secondary asset" (stylesheet vs schema).
    # T4a = XSLT transform (stylesheet); T4b = XSD validation (schema).
    local rcl_asset_env       # for rust-camel-lib
    local java_asset_prop     # for camel-standalone-dsl + quarkus-native
    case "$scenario" in
        xslt-bridge)
            if [[ ! -f "$shared_stylesheet" ]]; then
                echo "error: xslt-bridge shared stylesheet not found: $shared_stylesheet" >&2
                exit 1
            fi
            rcl_asset_env="BENCH_STYLESHEET=$shared_stylesheet"
            java_asset_prop="-Dbench.stylesheet=$shared_stylesheet"
            ;;
        xsd-validation-bridge)
            if [[ ! -f "$shared_schema" ]]; then
                echo "error: xsd-validation-bridge shared schema not found: $shared_schema" >&2
                exit 1
            fi
            rcl_asset_env="BENCH_SCHEMA=$shared_schema"
            java_asset_prop="-Dbench.schema=$shared_schema"
            ;;
        *)
            echo "error: resolve_bridge_scenario_cells: unknown bridge scenario '$scenario'" >&2
            exit 1
            ;;
    esac

    # -- 1. camel-standalone-dsl --
    # Java EventNotifierSupport emits BENCH_ROUTE_READY on RouteStarted.
    local standalone_dsl_jar
    standalone_dsl_jar="$(resolve_standalone_jar "$scenario_dir/camel-standalone/camel-standalone-dsl")"
    local sds_cell_safe="${scenario}_camel-standalone-dsl"
    local sds_latency="/tmp/v3-protocol-b-${sds_cell_safe}.log"
    add_cell "$scenario" "camel-standalone-dsl" \
        "$JAVA_BIN -Dbench.payload=$shared_payload $java_asset_prop -Dbench.latency_file=$sds_latency -jar $standalone_dsl_jar" \
        "$marker"

    # -- 2. camel-quarkus-dsl-native --
    # T4a + T4b: both build the native-image variant of the JVM
    # sibling's route. Source/resources are shared via sourceSets
    # (see camel-quarkus-dsl-native/build.gradle.kts).
    local q_dir="$scenario_dir/camel-quarkus"
    local q_dsl_native_glob="$q_dir/camel-quarkus-dsl-native/build/*-runner"
    local qdn_cell_safe="${scenario}_camel-quarkus-dsl-native"
    local qdn_latency="/tmp/v3-protocol-b-${qdn_cell_safe}.log"
    if [[ "$DRY_RUN" == "false" ]]; then
        build_native_artifact "camel-quarkus-dsl-native" "$q_dir" "$q_dsl_native_glob"
    fi
    local qdn_bin
    qdn_bin="$(ls $q_dsl_native_glob 2>/dev/null | head -1 || true)"
    if [[ -z "$qdn_bin" ]]; then
        if [[ "$DRY_RUN" == "true" ]]; then
            qdn_bin="<would-build:camel-quarkus-dsl-native>"
        else
            echo "error: native runner not resolvable after build: $q_dsl_native_glob" >&2
            exit 1
        fi
    fi
    add_cell "$scenario" "camel-quarkus-dsl-native" \
        "$qdn_bin $NATIVE_HEAP_ARG -Dbench.payload=$shared_payload $java_asset_prop -Dbench.latency_file=$qdn_latency" \
        "$marker"

    # -- 3. rust-camel-lib (Pair A) --
    # Fixture sets CAMEL_XML_BRIDGE_BINARY_PATH internally from
    # BENCH_BRIDGE_WRAPPER (main.rs:105-111). Latency file + PID file
    # paths are passed via env vars so the harness's M2 protocol B
    # reader finds them at the canonical /tmp/v3-* paths.
    #
    # The argv uses `env` as the first word because GNU time re-parses
    # the argv list and would otherwise treat `VAR=value` tokens as
    # positional args to time itself (not bash prefix-assignments).
    # `env VAR=value /bin` is portable across time/cmd wrappers.
    local rust_lib_bin
    rust_lib_bin="$(resolve_rust_lib_bin "$scenario_dir" "$scenario")"
    local rll_cell_safe="${scenario}_rust-camel-lib"
    local rll_latency="/tmp/v3-protocol-b-${rll_cell_safe}.log"
    local rll_pid="/tmp/v3-bridge-pid-${rll_cell_safe}.txt"
    add_cell "$scenario" "rust-camel-lib" \
        "env BENCH_PAYLOAD=$shared_payload $rcl_asset_env BENCH_LATENCY_FILE=$rll_latency BENCH_BRIDGE_WRAPPER=$bridge_wrapper CAMEL_XML_BRIDGE_REAL_BINARY=$bridge_binary V3_BRIDGE_PID_FILE=$rll_pid $rust_lib_bin" \
        "$marker"

    # -- 4. rust-camel-cli (Pair B) --
    # Invoked via the per-scenario wrapper script (sets bridge env vars
    # + extracts BENCH_LATENCY from child stdout + writes the PID file
    # before exec'ing the real `camel run` binary). Wrapper convention
    # is `${scenario}-cli-wrapper.sh`; route file globbed (T4a uses
    # xslt-bench.yaml, T4b uses xsd-bench.yaml — name doesn't match
    # scenario exactly).
    local rust_cli_wrapper="$scenario_dir/rust-camel-cli/${scenario}-cli-wrapper.sh"
    if [[ ! -x "$rust_cli_wrapper" ]]; then
        rust_cli_wrapper="$(ls "$scenario_dir/rust-camel-cli/"*-cli-wrapper.sh 2>/dev/null | head -1 || true)"
        if [[ -z "$rust_cli_wrapper" ]]; then
            echo "error: rust-camel-cli wrapper script not found under $scenario_dir/rust-camel-cli/" >&2
            exit 1
        fi
    fi
    local rust_cli_route
    rust_cli_route="$(ls "$scenario_dir/rust-camel-cli/routes/"*.yaml 2>/dev/null | head -1 || true)"
    if [[ -z "$rust_cli_route" ]]; then
        echo "error: no route YAML found under $scenario_dir/rust-camel-cli/routes/" >&2
        exit 1
    fi
    local rust_cli_camel_toml="$scenario_dir/rust-camel-cli/Camel.toml"
    if [[ ! -f "$rust_cli_camel_toml" ]]; then
        echo "error: rust-camel-cli Camel.toml not found: $rust_cli_camel_toml" >&2
        exit 1
    fi
    local rcl_cell_safe="${scenario}_rust-camel-cli"
    local rcl_latency="/tmp/v3-protocol-b-${rcl_cell_safe}.log"
    local rcl_pid="/tmp/v3-bridge-pid-${rcl_cell_safe}.txt"
    add_cell "$scenario" "rust-camel-cli" \
        "$rust_cli_wrapper --camel-bin $rust_cli_bin --config $rust_cli_camel_toml --routes $rust_cli_route --bridge-binary $bridge_binary --bridge-wrapper $bridge_wrapper --bridge-pid-file $rcl_pid --latency-file $rcl_latency" \
        "$marker"
}

# Resolve all per-cell artifacts and populate the CELLS / CELL_ARGV
# maps. Iterates the 2 scenarios × 8 contenders = 16 cells.
resolve_all_cells() {
    # Workspace root target dir (rust-camel-cli binary, shared across
    # scenarios — same binary runs both T1 and T2 routes via --routes).
    local root_target
    root_target="$(resolve_cargo_target_dir "$REPO_ROOT")"
    if [[ -z "$root_target" || "$root_target" == "null" ]]; then
        echo "error: could not resolve root target_directory via cargo metadata" >&2
        exit 1
    fi
    local rust_cli_bin="$root_target/release/camel"
    if [[ ! -x "$rust_cli_bin" ]]; then
        echo "error: rust-camel-cli binary not found at $rust_cli_bin (cargo build --release -p camel-cli)" >&2
        exit 1
    fi

    for scenario in "${SCENARIOS[@]}"; do
        local scenario_dir="$SCENARIOS_DIR/$scenario"
        if [[ ! -d "$scenario_dir" ]]; then
            echo "error: scenario directory disappeared during iteration: $scenario_dir" >&2
            exit 1
        fi
        local marker="${SCENARIO_MARKER[$scenario]:-}"
        if [[ -z "$marker" ]]; then
            echo "error: unknown scenario '$scenario' — no expected marker registered (known: ${!SCENARIO_MARKER[*]})" >&2
            echo "       to add a scenario, declare its marker in SCENARIO_MARKER + create the fixture" >&2
            exit 1
        fi

        # -- Dispatch bridge scenarios to the 4-artifact resolver --
        # T4a/T4b have an asymmetric matrix (4 artifacts, not 8); the
        # bridge resolver handles their per-cell argv construction
        # (wrapper script, bridge binary env vars, latency file paths).
        # Gap 4 of bd rc-alhy.
        case "${SCENARIO_ARTIFACT_SET[$scenario]:-full}" in
            bridge)
                resolve_bridge_scenario_cells "$scenario" "$scenario_dir" "$marker" "$rust_cli_bin"
                continue
                ;;
            full|*) : ;;  # fall through to 8-artifact resolution below
        esac

        # -- Per-scenario artifact resolution --
        # Camel standalone (Maven, two jars).
        local standalone_dsl_jar standalone_yaml_jar
        standalone_dsl_jar="$(resolve_standalone_jar "$scenario_dir/camel-standalone/camel-standalone-dsl")"
        standalone_yaml_jar="$(resolve_standalone_jar "$scenario_dir/camel-standalone/camel-standalone-yaml")"

        # v3.5: Camel Quarkus JVM mode dropped (camel-quarkus-dsl +
        # camel-quarkus-yaml) — redundant with camel-standalone-* JVM
        # baseline. Only the -native variants remain measured.

        # Camel Quarkus native (globs resolved post-build; pre-check
        # is a soft existence check, the real check is in preflight).
        local q_dir="$scenario_dir/camel-quarkus"
        local q_dsl_native_glob="$q_dir/camel-quarkus-dsl-native/build/*-runner"
        local q_yaml_native_glob="$q_dir/camel-quarkus-yaml-native/build/*-runner"

        # rust-camel-lib: bin name = scenario name (per fixture
        # convention; see T1 main.rs `[[bin]] name = "startup-minimal"`,
        # T2 `[[bin]] name = "t2-realistic-eip"`).
        local rust_lib_bin
        rust_lib_bin="$(resolve_rust_lib_bin "$scenario_dir" "$scenario")"

        # rust-camel-cli: shared binary, scenario-specific route YAML
        # (convention: <scenario-name>.yaml under routes/).
        local rust_cli_route="$scenario_dir/rust-camel-cli/routes/$scenario.yaml"
        if [[ ! -f "$rust_cli_route" ]]; then
            # Fallback: glob for any single yaml under routes/.
            local fallback
            fallback="$(ls "$scenario_dir/rust-camel-cli/routes/"*.yaml 2>/dev/null | head -1 || true)"
            if [[ -z "$fallback" ]]; then
                echo "error: no route YAML found under $scenario_dir/rust-camel-cli/routes/" >&2
                exit 1
            fi
            rust_cli_route="$fallback"
        fi
        local rust_cli_camel_toml="$scenario_dir/rust-camel-cli/Camel.toml"
        if [[ ! -f "$rust_cli_camel_toml" ]]; then
            echo "error: rust-camel-cli Camel.toml not found: $rust_cli_camel_toml" >&2
            exit 1
        fi

        # -- Build (or skip via fingerprint) the 2 native artifacts --
        # Both builds run sequentially — they share the same gradle
        # .gradle/ cache and daemon, and parallel native builds OOM
        # on this host (spike observed ~3-4.5 GiB each).
        if [[ "$DRY_RUN" == "false" ]]; then
            build_native_artifact "camel-quarkus-dsl-native" "$q_dir" "$q_dsl_native_glob"
            build_native_artifact "camel-quarkus-yaml-native" "$q_dir" "$q_yaml_native_glob"
        fi

        # Resolve native runner paths (post-build — may have just
        # been built). Use the first glob match as the resolved path.
        # In --dry-run mode, the runner may not exist yet (builds are
        # skipped); surface a placeholder so the 16-cell list still
        # resolves and the shuffle is testable pre-build.
        local q_dsl_native_bin q_yaml_native_bin
        q_dsl_native_bin="$(ls $q_dsl_native_glob 2>/dev/null | head -1 || true)"
        q_yaml_native_bin="$(ls $q_yaml_native_glob 2>/dev/null | head -1 || true)"
        if [[ -z "$q_dsl_native_bin" ]]; then
            if [[ "$DRY_RUN" == "true" ]]; then
                q_dsl_native_bin="<would-build:camel-quarkus-dsl-native>"
            else
                echo "error: native runner not resolvable after build: $q_dsl_native_glob" >&2
                exit 1
            fi
        fi
        if [[ -z "$q_yaml_native_bin" ]]; then
            if [[ "$DRY_RUN" == "true" ]]; then
                q_yaml_native_bin="<would-build:camel-quarkus-yaml-native>"
            else
                echo "error: native runner not resolvable after build: $q_yaml_native_glob" >&2
                exit 1
            fi
        fi

        # -- Register the 6 cells for this scenario --
        # v3.5 matrix: camel-standalone-{dsl,yaml} (JVM baseline) +
        # camel-quarkus-{dsl,yaml}-native (Apache native answer) +
        # rust-camel-{lib,cli}. Quarkus JVM variants dropped (see
        # resolve_all_cells pre-check comment above).
        add_cell "$scenario" "camel-standalone-dsl" \
            "$JAVA_BIN -jar $standalone_dsl_jar" "$marker"
        add_cell "$scenario" "camel-quarkus-dsl-native" \
            "$q_dsl_native_bin $NATIVE_HEAP_ARG" "$marker"
        add_cell "$scenario" "rust-camel-lib" \
            "$rust_lib_bin" "$marker"
        add_cell "$scenario" "camel-standalone-yaml" \
            "$JAVA_BIN -jar $standalone_yaml_jar" "$marker"
        add_cell "$scenario" "camel-quarkus-yaml-native" \
            "$q_yaml_native_bin $NATIVE_HEAP_ARG" "$marker"
        # rust-camel-cli: prefer the per-scenario wrapper script if it
        # exists (T3 http-server ships one because the bare `camel run`
        # binary doesn't print BENCH_ROUTE_READY — the wrapper detects
        # the child's "CamelContext started" stdout line and emits the
        # marker per the rc-w1u9 handshake). Bridge scenarios also use
        # wrappers; those are dispatched via resolve_bridge_scenario_cells.
        local rcli_wrapper="$scenario_dir/rust-camel-cli/${scenario}-cli-wrapper.sh"
        if [[ -x "$rcli_wrapper" ]]; then
            add_cell "$scenario" "rust-camel-cli" \
                "$rcli_wrapper --camel-bin $rust_cli_bin --config $rust_cli_camel_toml --routes $rust_cli_route" \
                "$marker"
        else
            add_cell "$scenario" "rust-camel-cli" \
                "$rust_cli_bin run --config $rust_cli_camel_toml --routes $rust_cli_route --no-watch" \
                "$marker"
        fi
    done

    # After cell registration: populate Protocol A URL + cell list for
    # any scenario whose SCENARIO_M2_PROTOCOL entry is "A" (currently
    # only http-server). All such cells share the BENCH_HTTP_URL
    # constant (127.0.0.1:8080/bench on the client side) — they are
    # mutually exclusive on the port, so calibration + measurement
    # launch + kill one cell at a time.
    for cell in "${CELLS[@]}"; do
        local _pa_scenario="${CELL_SCENARIO[$cell]}"
        if [[ "${SCENARIO_M2_PROTOCOL[$_pa_scenario]:-}" == "A" ]]; then
            PROTOCOL_A_CELL_URL["$cell"]="$BENCH_HTTP_URL"
            if [[ -z "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
                PROTOCOL_A_CELLS+=("$cell")
            fi
        fi
    done
}

# Helper: register a cell (scenario/contender pair) with its argv
# (space-separated string, re-parsed by measure_once) and marker.
# Optional 5th arg registers the cell as won't-measure with a
# human-readable reason — the cell appears in dry-run output with a
# SKIP marker and is skipped by M1/M2 measurement loops (Gap 5).
add_cell() {
    local scenario="$1" contender="$2" argv_str="$3" marker="$4"
    local wont_measure_reason="${5:-}"
    local cell="$scenario/$contender"
    CELLS+=("$cell")
    CELL_SCENARIO["$cell"]="$scenario"
    CELL_ARGV["$cell"]="$argv_str"
    CELL_MARKER["$cell"]="$marker"
    if [[ -n "$wont_measure_reason" ]]; then
        CELL_WONT_MEASURE["$cell"]="$wont_measure_reason"
    fi
}

# =====================================================================
# 16-cell shuffle (4b): Fisher-Yates over the full cell list, ONCE
# per measurement round. NOT 8 artifacts shuffled within 2 sequential
# scenario blocks — that would let scenario-order drift between T1
# and T2. Shuffle is applied to the complete {scenario × artifact}
# list.
# =====================================================================
# Generic Fisher-Yates shuffle over the array named by $1. Optional $2
# seeds bash's $RANDOM so the order is reproducible (T5 randomized-block
# design); when omitted the default time-entropy $RANDOM is retained.
_shuffle_seeded() {
    local -n _fy_arr="$1"
    local seed="${2:-}"
    [[ -n "$seed" ]] && RANDOM="$seed"
    local i j tmp n=${#_fy_arr[@]}
    for ((i = n - 1; i > 0; i--)); do
        j=$((RANDOM % (i + 1)))
        tmp="${_fy_arr[i]}"; _fy_arr[i]="${_fy_arr[j]}"; _fy_arr[j]="$tmp"
    done
}

shuffle_cells() {
    # Optional $1 = seed for reproducible ordering (T5).
    _shuffle_seeded CELLS "$1"
}

# =====================================================================
# measurement: single-clock cold-start + peak-RSS (v1, no drift)
#
# Args: <cell> <results-file>  (cell determines the argv via CELL_ARGV
# and the expected marker via CELL_MARKER)
#
# Per FINAL arbitrated design (v1 plan "Measurement design
# correction"): no self-instrumentation, no docker in the timed
# path. Harness times from immediately before launching the artifact
# directly (bare-metal, via /usr/bin/time -v which forks the real
# target as its child) to the BENCH_ROUTE_READY[ body=...] marker
# observed in captured stdout — a single clock, capturing full
# JVM/process bootstrap without container overhead.
# =====================================================================
measure_once() {
    local cell="$1" results_file="$2"
    local argv_str="${CELL_ARGV[$cell]}"
    # Re-parse the space-separated argv. word-splitting is intentional
    # here (the cell argv is a controlled list of paths + flags).
    # shellcheck disable=SC2206
    local argv=($argv_str)
    local expected_marker="${CELL_MARKER[$cell]}"
    local run_id out_file time_file start_ns elapsed_ms rss_kb marker_count

    run_id="$(date +%s%N)-$$-$RANDOM"
    out_file="$SCRATCH_DIR/${cell//\//_}.${run_id}.out"
    time_file="$SCRATCH_DIR/${cell//\//_}.${run_id}.time"

    start_ns=$(date +%s%N)
    "${SERVER_AFFINITY[@]}" "$TIME_BIN" -v -o "$time_file" "${argv[@]}" > "$out_file" 2>&1 &
    CURRENT_TIME_PID=$!

    # Resolve the actual contender PID — /usr/bin/time's direct child.
    # Poll at 2ms granularity (up to 500ms total) since time may not
    # have forked the target yet at the instant we read /proc.
    # Non-fatal: if the contender is fast enough to start AND exit
    # within the polling window (e.g. startup-minimal rust-camel-cli
    # at ~8-13ms), we may never observe it in /proc/<pid>/children.
    # That's OK — the marker polling loop below detects the marker
    # via the output file regardless, and time -v captures the
    # measurement data before the child exits.
    local child_pid=""
    for _ in {1..250}; do
        child_pid=$(awk '{print $1}' /proc/$CURRENT_TIME_PID/task/$CURRENT_TIME_PID/children 2>/dev/null || true)
        [[ -n "$child_pid" ]] && break
        sleep 0.002
    done
    if [[ -z "$child_pid" ]]; then
        # Non-fatal: fast-exiting cells (startup-minimal) may complete
        # before we can observe the child PID. Continue with empty
        # CURRENT_CHILD_PID; marker polling + time output still work.
        echo "warning: could not resolve contender PID for $cell (time_pid=$CURRENT_TIME_PID) — fast-exiting cell, continuing" >&2
        CURRENT_CHILD_PID=""
    else
        CURRENT_CHILD_PID="$child_pid"
    fi

    # Poll for the marker at 1ms granularity (v1 Fix 2) with a
    # wall-clock deadline (30s). 10ms polling quantizes fast Rust
    # results (a 3ms startup rounds up to 10ms; a 12ms startup looks
    # identical).
    local deadline_ns=$((start_ns + 30000000000))
    while ! grep -qF "$expected_marker" "$out_file" 2>/dev/null; do
        if ! kill -0 "$CURRENT_TIME_PID" 2>/dev/null; then
            # Process exited — do one final marker check. Fast-exiting
            # cells (e.g. startup-minimal rust-camel-cli at ~8-13ms)
            # buffer stdout and flush on exit; the marker may appear
            # in the file only AFTER the process is gone.
            if grep -qF "$expected_marker" "$out_file" 2>/dev/null; then
                break  # marker found post-exit — valid measurement
            fi
            echo "error: process exited before marker for $cell (expected '$expected_marker'; see $out_file)" >&2
            wait "$CURRENT_TIME_PID" 2>/dev/null || true
            CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""
            return 1
        fi
        if [[ $(date +%s%N) -gt $deadline_ns ]]; then
            echo "error: marker not observed within 30s for $cell — killing" >&2
            if [[ -n "$CURRENT_CHILD_PID" ]]; then
                _kill_process_tree_recursive "$CURRENT_CHILD_PID"
            fi
            wait "$CURRENT_TIME_PID" 2>/dev/null || true
            CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""
            return 1
        fi
        sleep 0.001
    done

    elapsed_ms=$(( ($(date +%s%N) - start_ns) / 1000000 ))

    # KILL the contender immediately after the marker so peak RSS
    # is recorded before any graceful-shutdown teardown allocations
    # (v1 Fix 7). /usr/bin/time observes the child's exit and writes
    # the time_file; we then reap time itself.
    # Tree-aware kill (C6 follow-up — bd rc-8ysn): the rust-camel-cli
    # wrapper uses `setsid`, so SIGKILL on the wrapper alone would
    # orphan the camel run child and leak its listener port into the
    # next cell's M1 launch. The recursive walk reaches the setsid
    # child while the wrapper's /proc/<pid>/task/.../children entries
    # are still intact.
    if [[ -n "$CURRENT_CHILD_PID" ]]; then
        _kill_process_tree_recursive "$CURRENT_CHILD_PID"
    fi
    wait "$CURRENT_TIME_PID" 2>/dev/null || true
    CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""

    # Scenario-aware exact marker-count validation (spec §4.5):
    # the expected marker string is the FULL token (T1: bare
    # BENCH_ROUTE_READY; T2: with body suffix). grep -F counts lines
    # containing the fixed string, so a T2 fixture that emits BOTH
    # the static `BENCH_ROUTE_READY` line and the dynamic
    # `BENCH_ROUTE_READY body=pong-bench` line is counted correctly
    # (only the dynamic one matches the fixed string for T2).
    marker_count=$(grep -cF "$expected_marker" "$out_file" 2>/dev/null || echo 0)
    if [[ "$marker_count" -ne 1 ]]; then
        echo "error: expected exactly 1 '$expected_marker' marker for $cell, got $marker_count (see $out_file)" >&2
        return 1
    fi

    rss_kb=$(grep -oP 'Maximum resident set size \(kbytes\): \K[0-9]+' "$time_file" 2>/dev/null || true)
    if [[ -z "$rss_kb" || "$rss_kb" -eq 0 ]]; then
        echo "error: invalid RSS measurement for $cell (see $time_file)" >&2
        return 1
    fi

    echo "$elapsed_ms $rss_kb" >> "$results_file"
}

# =====================================================================
# summarize: median + nearest-rank p95 (v1, no drift)
# =====================================================================
summarize() {
    local col="$1" file="$2"
    awk -v col="$col" '
        { arr[NR] = $col }
        END {
            if (NR == 0) { printf "n=0 (no data)\n"; exit }
            n = asort(arr)
            median = (n % 2 == 1) ? arr[(n+1)/2] : (arr[n/2] + arr[n/2+1]) / 2
            # Nearest-rank p95: ceil(0.95 * n), clamped to [1, n].
            p95_idx = int(0.95 * n)
            if (p95_idx < 0.95 * n) p95_idx++
            if (p95_idx < 1) p95_idx = 1
            if (p95_idx > n) p95_idx = n
            printf "median=%.1f p95=%s n=%d\n", median, arr[p95_idx], n
        }
    ' "$file"
}

# =====================================================================
# M2 harness extension (spec §4.1–4.11, brief task 2c–2k).
#
# All v2 M1 invariants are preserved — the M1 path above is unchanged.
# The M2 path runs conditionally on `--metric=m2` or `--metric=m1+m2`
# and adds:
#
# - Protocol A (T3 HTTP server, request-response): bench-loadgen drives
#   the contender at a calibrated fixed rate (MIN across contenders).
#   Per spec §4.4 contradiction #1: baseline-subtraction applies ONLY
#   to p50 location; higher percentiles published raw side-by-side.
# - Protocol B (T1/T2/T4a/T4b, service-time): contender emits per-tick
#   records to /tmp/v3-protocol-b-<cell>.log; parsed post-run.
#   Per spec §4.4 contradiction #2: timer period held at 10ms across
#   all contenders; saturation escalates via won't-measure, not period
#   adaptation.
# - Bridge PID tracking (T4a/T4b only): PID at round boundaries; per
#   spec §4.11 contradiction #3, NO per-message /proc reads on hot path.
# - RSS sampling (T4a/T4b only): process-tree Σ VmRSS at 10ms cadence
#   during M1 measurement window.
# - BCa CI on per-round p50 (e_opus deferral + spec §4.2).
#
# The current T1/T2 fixtures do NOT emit per-tick records (their timer
# is repeatCount=1); T1/T2 M2 runs will parse an empty Protocol B log
# and report n=0 (cell marked "not-measured: pending fixture emission").
# This is the expected state until Task 3+ ships the per-tick fixtures.
# =====================================================================

# Resolve the bench-loadgen binary (workspace member). Built into the
# root workspace's target dir (not the spike's standalone target).
resolve_loadgen_bin() {
    local root_target
    root_target="$(resolve_cargo_target_dir "$REPO_ROOT")"
    local bin="$root_target/release/bench-loadgen"
    if [[ ! -x "$bin" ]]; then
        echo "error: bench-loadgen binary not found at $bin" >&2
        echo "       run: env -u CARGO_TARGET_DIR cargo build --release -p bench-loadgen" >&2
        exit 1
    fi
    echo "$bin"
}

# Resolve the bench-devnull binary (loopback baseline for Protocol A
# calibration, spec §4.4).
resolve_devnull_bin() {
    local root_target
    root_target="$(resolve_cargo_target_dir "$REPO_ROOT")"
    local bin="$root_target/release/bench-devnull"
    if [[ ! -x "$bin" ]]; then
        echo "error: bench-devnull binary not found at $bin" >&2
        echo "       run: env -u CARGO_TARGET_DIR cargo build --release -p bench-loadgen" >&2
        exit 1
    fi
    echo "$bin"
}

# Per-scenario M2 protocol (A or B). T1/T2/T4a/T4b are Protocol B
# (timer → set_body → component → log; harness measures per-tick
# service-time by parsing /tmp/v3-protocol-b-<cell>.log). T3 is
# Protocol A (HTTP request/response; harness drives the loadgen).
declare -A SCENARIO_M2_PROTOCOL=(
    ["startup-minimal"]="B"
    ["t2-realistic-eip"]="B"
    ["xslt-bridge"]="B"
    ["xsd-validation-bridge"]="B"
    # T3 http-server uses Protocol A (HTTP request-response). The URL
    # is the shared BENCH_HTTP_URL constant (127.0.0.1:8080/bench on
    # the client side; the contender binds 0.0.0.0:8080). All 8 T3
    # cells share the same URL — they are mutually exclusive on the
    # port, so calibration + measurement launch + kill one cell at a
    # time.
    ["http-server"]="A"
)

# Calibrate Protocol A common rate across a set of contender URLs.
# Sets the global PROTOCOL_A_COMMON_RATE to the MIN sustainable rate
# (spec §4.4 fairness invariant). Iterates PROTOCOL_A_CELLS (populated
# by resolve_all_cells from SCENARIO_M2_PROTOCOL).
PROTOCOL_A_COMMON_RATE=0

# Contender launch + cleanup helpers (shared by Protocol A + B paths).
#
# launch_contender_wait_marker: launches the contender under GNU time,
# resolves its PID, polls the captured stdout for the expected marker
# (BENCH_ROUTE_READY), and returns once observed. Sets CURRENT_TIME_PID
# + CURRENT_CHILD_PID on success. The captured stdout path is stored
# in CURRENT_M2_OUT_FILE for caller inspection.
#
# kill_contender: KILLs both the contender (child) and the time wrapper,
# waits for reaping. Safe to call when CURRENT_* are empty.
#
# cleanup_bridge_subprocess: KILLs the bridge subprocess recorded in
# /tmp/v3-bridge-pid-<cell>.txt (if present) plus its descendant process
# tree, then removes the PID file. C6: prevents bridge subprocess leaks
# between M2 cells (the bridge spawns lazily post-marker and is NOT a
# direct child of the time wrapper, so the standard contender kill does
# not reach it).
CURRENT_M2_OUT_FILE=""
CURRENT_M2_TIME_FILE=""

launch_contender_wait_marker() {
    local cell="$1"
    local argv_str="${CELL_ARGV[$cell]}"
    # shellcheck disable=SC2206
    local argv=($argv_str)
    local expected_marker="${CELL_MARKER[$cell]}"
    local cell_safe="${cell//\//_}"
    local out_file="$SCRATCH_DIR/m2.${cell_safe}.$(date +%s%N).out"
    local time_file="$SCRATCH_DIR/m2.${cell_safe}.$(date +%s%N).time"

    "${SERVER_AFFINITY[@]}" "$TIME_BIN" -v -o "$time_file" "${argv[@]}" > "$out_file" 2>&1 &
    CURRENT_TIME_PID=$!

    # Resolve the actual contender PID — /usr/bin/time's direct child.
    # Poll at 2ms granularity (up to 500ms total). Non-fatal: fast-exiting
    # cells may complete before we observe the child (see M1 site for
    # full rationale).
    local child_pid=""
    for _ in {1..250}; do
        child_pid=$(awk '{print $1}' /proc/$CURRENT_TIME_PID/task/$CURRENT_TIME_PID/children 2>/dev/null || true)
        [[ -n "$child_pid" ]] && break
        sleep 0.002
    done
    if [[ -z "$child_pid" ]]; then
        echo "warning: launch_contender_wait_marker: could not resolve contender PID for $cell — continuing" >&2
        CURRENT_CHILD_PID=""
    else
        CURRENT_CHILD_PID="$child_pid"
    fi
    CURRENT_CHILD_PID="$child_pid"
    CURRENT_M2_OUT_FILE="$out_file"
    CURRENT_M2_TIME_FILE="$time_file"

    # Poll for the marker at 1ms granularity (matches M1) with a 30s
    # wall-clock deadline.
    local start_ns deadline_ns
    start_ns=$(date +%s%N)
    deadline_ns=$((start_ns + 30000000000))
    while ! grep -qF "$expected_marker" "$out_file" 2>/dev/null; do
        if ! kill -0 "$CURRENT_TIME_PID" 2>/dev/null; then
            # Process exited — one final marker check (stdout flushed on exit).
            if grep -qF "$expected_marker" "$out_file" 2>/dev/null; then
                break
            fi
            echo "error: contender exited before marker for $cell (expected '$expected_marker'; see $out_file)" >&2
            wait "$CURRENT_TIME_PID" 2>/dev/null || true
            CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""
            return 1
        fi
        if [[ $(date +%s%N) -gt $deadline_ns ]]; then
            echo "error: marker not observed within 30s for $cell (Protocol A launch)" >&2
            [[ -n "$CURRENT_CHILD_PID" ]] && _kill_process_tree_recursive "$CURRENT_CHILD_PID"
            wait "$CURRENT_TIME_PID" 2>/dev/null || true
            CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""
            return 1
        fi
        sleep 0.001
    done
    return 0
}

kill_contender() {
    # Kill the contender's entire process tree BEFORE killing the
    # wrapper itself. The rust-camel-cli wrapper uses `setsid` to
    # spawn `camel run` in a new session; SIGKILL on the wrapper
    # alone leaves the setsid child orphaned (reparented to init)
    # and still bound to its listener port. The next cell then hits
    # EADDRINUSE. Killing the tree recursively from the wrapper PID
    # reaches the setsid child while its parent-children entries are
    # still intact. (bd rc-8ysn follow-up: T3 wrapper SIGKILL
    # orphan — fixed here by tree-aware kill.)
    if [[ -n "${CURRENT_CHILD_PID:-}" ]]; then
        _kill_process_tree_recursive "$CURRENT_CHILD_PID"
    fi
    if [[ -n "${CURRENT_TIME_PID:-}" ]]; then
        kill -KILL "$CURRENT_TIME_PID" 2>/dev/null || true
        wait "$CURRENT_TIME_PID" 2>/dev/null || true
    fi
    CURRENT_TIME_PID=""
    CURRENT_CHILD_PID=""
}

# Recursive process-tree kill via /proc/<pid>/task/<tid>/children walk.
# Kills descendants before the parent. Cycle-safe via visited set.
# Args: <pid>
_kill_process_tree_recursive() {
    local pid="$1"
    [[ -z "$pid" ]] && return
    kill -0 "$pid" 2>/dev/null || return
    local -A seen=()
    local -a stack=("$pid")
    local -a to_kill=()
    while [[ ${#stack[@]} -gt 0 ]]; do
        local cur="${stack[-1]}"
        unset 'stack[-1]'
        [[ -n "${seen[$cur]:-}" ]] && continue
        seen[$cur]=1
        to_kill+=("$cur")
        if [[ -d "/proc/$cur/task" ]]; then
            local tid child_line child
            while read -r tid; do
                [[ -z "$tid" ]] && continue
                child_line="$(cat /proc/$cur/task/$tid/children 2>/dev/null || true)"
                [[ -z "$child_line" ]] && continue
                for child in $child_line; do
                    [[ -z "$child" ]] && continue
                    [[ -z "${seen[$child]:-}" ]] && stack+=("$child")
                done
            done < <(ls /proc/$cur/task 2>/dev/null)
        fi
    done
    local i
    for ((i = ${#to_kill[@]} - 1; i >= 0; i--)); do
        kill -KILL "${to_kill[$i]}" 2>/dev/null || true
    done
}

# Kill the bridge subprocess recorded in /tmp/v3-bridge-pid-<cell>.txt
# (if present) plus its descendant process tree, then remove the PID
# file. Idempotent — safe to call when no bridge was spawned (file is
# absent). C6.
cleanup_bridge_subprocess() {
    local cell="$1"
    local cell_safe="${cell//\//_}"
    local bridge_pid_file="/tmp/v3-bridge-pid-${cell_safe}.txt"
    if [[ ! -f "$bridge_pid_file" ]]; then return; fi
    local bridge_pid
    bridge_pid="$(cat "$bridge_pid_file" 2>/dev/null || true)"
    if [[ -n "$bridge_pid" ]] && kill -0 "$bridge_pid" 2>/dev/null; then
        _kill_process_tree_recursive "$bridge_pid"
    fi
    rm -f "$bridge_pid_file"
}

# Write per-cell diagnostic metadata for post-hoc debugging. Called from
# all 4 return paths of the first-success probe (3 failures + 1 success).
#
# ALSO persists the cell stderr (caller's local `stderr_file`) to
# $dir/bridge-stderr.log — for rust-camel-* cells this captures the
# bridge subprocess's stderr (inherited via camel-bridge's default
# Stdio::inherit). Without this preservation on failure paths, the
# exit-codes.txt references a bridge-stderr.log that doesn't exist,
# hiding the actual cause of bridge/cell death.
#
# Args: <samples_dir> <reason> [<extra_key=value>...]
_write_probe_diag() {
    local dir="$1" reason="$2"; shift 2
    # stderr_file is the caller's local; bash dynamic scope makes it
    # visible here. Tolerate absence (defensive — older callers).
    local stderr_src="${stderr_file:-}"
    local stderr_dest="$dir/bridge-stderr.log"
    if [[ -n "$stderr_src" && -s "$stderr_src" ]]; then
        cp "$stderr_src" "$stderr_dest"
    else
        : > "$stderr_dest"  # empty file preserves the contract
    fi
    {
        echo "# probe reason: $reason"
        echo "cell_pid=${CURRENT_CHILD_PID:-}"
        echo "bridge_pid=$(cat "$bridge_pid_file" 2>/dev/null || true)"
        echo "bridge_stderr=$stderr_dest"
        for kv in "$@"; do
            echo "$kv"
        done
    } > "$dir/exit-codes.txt"
}

calibrate_protocol_a() {
    local loadgen_bin="$1"
    if [[ ${#PROTOCOL_A_CELLS[@]} -eq 0 ]]; then
        echo "calibrate_protocol_a: no Protocol A cells registered" >&2
        PROTOCOL_A_COMMON_RATE=0
        return 0
    fi
    local -a rates=()
    for pa_cell in "${PROTOCOL_A_CELLS[@]}"; do
        local url="${PROTOCOL_A_CELL_URL[$pa_cell]:-}"
        if [[ -z "$url" ]]; then
            echo "  calibration: $pa_cell → no URL, skipping (rate=0)" >&2
            rates+=("0")
            continue
        fi
        # T3 cells bind 0.0.0.0:8080 and are mutually exclusive on the
        # port — calibrate one at a time: launch, calibrate, kill.
        if ! launch_contender_wait_marker "$pa_cell"; then
            echo "  calibration: $pa_cell → launch failed (rate=0)" >&2
            rates+=("0")
            continue
        fi
        local line rate
        line="$("${LOADGEN_AFFINITY[@]}" "$loadgen_bin" calibrate --url="$url" --timeout-secs=60 2>/dev/null \
            | grep -m1 '^BENCH_CALIBRATION_RESULT' || true)"
        rate="$(printf '%s' "$line" | sed -n 's/.*max_sustainable_rate_per_sec=\([0-9]\+\).*/\1/p')"
        rate="${rate:-0}"
        rates+=("$rate")
        echo "  calibration: $pa_cell ($url) → max_sustainable=${rate}/sec"
        kill_contender
        cleanup_bridge_subprocess "$pa_cell"
        # Brief pause for kernel to release the listener socket. SIGKILL
        # frees LISTEN sockets immediately (no TIME_WAIT), but a small
        # delay avoids racing the next cell's bind() against this cell's
        # process teardown.
        sleep 0.3
    done
    # MIN across contenders (brief 2b: fairness invariant).
    local min_rate="${rates[0]:-0}"
    for r in "${rates[@]:1}"; do
        if [[ "$r" -lt "$min_rate" ]]; then
            min_rate="$r"
        fi
    done
    PROTOCOL_A_COMMON_RATE="$min_rate"
    echo "  calibration: common rate (MIN) = $PROTOCOL_A_COMMON_RATE/sec"
}

# Measure one cell's Protocol B latency (T1/T2/T4a/T4b).
#
# Re-launches the contender argv for the full M2 window (warmup-time +
# rounds*samples-per-round*timer_period), then parses
# /tmp/v3-protocol-b-<cell>.log for per-tick records.
#
# Args: <cell> <results-dir>
m2_measure_protocol_b() {
    local cell="$1" results_dir="$2"
    local loadgen_bin
    loadgen_bin="$(resolve_loadgen_bin)"
    local argv_str="${CELL_ARGV[$cell]}"
    # shellcheck disable=SC2206
    local argv=($argv_str)

    # F3: skip scenarios with no per-tick workload (T1/T2). Their routes
    # are timer→log or timer→EIP→log; they never emit BENCH_LATENCY and
    # running Protocol B for them would spam "no per-tick records" warnings.
    local scenario="${cell%%/*}"
    case "$scenario" in
        startup-minimal|t2-realistic-eip)
            echo "m2 protocol B: $cell → skipped (scenario $scenario has no per-tick workload per F3)" >&2
            return 0
            ;;
    esac

    local cell_safe="${cell//\//_}"
    local log_file="/tmp/v3-protocol-b-${cell_safe}.log"
    local bridge_pid_file="/tmp/v3-bridge-pid-${cell_safe}.txt"
    local out_file="$SCRATCH_DIR/m2.${cell_safe}.$(date +%s%N).out"
    local samples_dir="$results_dir/$cell"  # real subdir: <results>/<scenario>/<contender>/
    mkdir -p "$samples_dir"

    # Pre-clean tmpfs files from prior runs (spec F2: no stale data leaks).
    rm -f "$log_file" "$bridge_pid_file"
    echo "m2 protocol B: truncated latency + bridge pid files for $cell" >&2

    # Bridge PID tracking — round START (spec §4.11, brief 2f).
    local bridge_pid_start=""
    if [[ "$METRIC_BRIDGE_TRACKING" == "true" ]]; then
        bridge_pid_start=""
    fi

    # Launch the contender under GNU time (RSS attribution for non-bridge
    # artifacts; bridge RSS sampled separately at 10ms cadence per spec §4.9).
    local time_file="$SCRATCH_DIR/m2.${cell_safe}.$(date +%s%N).time"
    local stderr_file="$SCRATCH_DIR/m2.${cell_safe}.$(date +%s%N).err"
    "${SERVER_AFFINITY[@]}" "$TIME_BIN" -v -o "$time_file" "${argv[@]}" > "$out_file" 2>"$stderr_file" &
    CURRENT_TIME_PID=$!

    # Resolve the actual contender PID for RSS sampling + cleanup.
    # Poll at 2ms granularity (up to 500ms total). Non-fatal: fast-exiting
    # cells may complete before we observe the child (see M1 site for
    # full rationale).
    local child_pid=""
    for _ in {1..250}; do
        child_pid=$(awk '{print $1}' /proc/$CURRENT_TIME_PID/task/$CURRENT_TIME_PID/children 2>/dev/null || true)
        [[ -n "$child_pid" ]] && break
        sleep 0.002
    done
    if [[ -z "$child_pid" ]]; then
        echo "warning: m2 protocol B: could not resolve contender PID for $cell — continuing" >&2
        CURRENT_CHILD_PID=""
    else
        CURRENT_CHILD_PID="$child_pid"
    fi
    CURRENT_CHILD_PID="$child_pid"

    # First-success probe (spec F2): wait up to PROBE_DEADLINE_SECS for the first
    # BENCH_LATENCY record. If the cell or bridge subprocess dies first, abort the
    # round with diagnostics (cell_pid / bridge_pid in exit-codes.txt +
    # bridge-stderr.log path). For Java cells there is no bridge subprocess
    # (bridge_pid_file stays empty); the bridge check is skipped via
    # [[ -s $bridge_pid_file ]].
    local probe_deadline=$((SECONDS + PROBE_DEADLINE_SECS))
    local first_success=0
    while (( SECONDS < probe_deadline )); do
        if [[ -s "$log_file" ]] && grep -q '^BENCH_LATENCY [0-9][0-9]* [0-9][0-9]*$' "$log_file"; then
            first_success=1
            echo "m2 protocol B: first BENCH_LATENCY observed for $cell" >&2
            break
        fi
        # Contender died?
        if [[ -n "$CURRENT_CHILD_PID" ]] && ! kill -0 "$CURRENT_CHILD_PID" 2>/dev/null; then
            echo "error: m2 protocol B: cell $cell (pid=$CURRENT_CHILD_PID) died before first BENCH_LATENCY emission (exit=unknown-already-dead; kill -0 confirmed death)" >&2
            echo "error: bridge stderr capture at $samples_dir/bridge-stderr.log" >&2
            _write_probe_diag "$samples_dir" "cell died before first emission"
            return 1
        fi
        # Bridge subprocess died? (only for rust-camel-* cells where bridge_pid_file is populated)
        local bridge_pid_check=""
        [[ -s "$bridge_pid_file" ]] && bridge_pid_check="$(cat "$bridge_pid_file" 2>/dev/null || true)"
        if [[ -n "$bridge_pid_check" ]] && ! kill -0 "$bridge_pid_check" 2>/dev/null; then
            echo "error: m2 protocol B: bridge subprocess for $cell (pid=$bridge_pid_check) died before first emission (exit=unknown-already-dead; kill -0 confirmed death)" >&2
            echo "error: bridge stderr capture at $samples_dir/bridge-stderr.log" >&2
            _write_probe_diag "$samples_dir" "bridge subprocess died before first emission"
            return 1
        fi
        sleep 2
    done
    if (( first_success == 0 )); then
        echo "error: m2 protocol B: no BENCH_LATENCY record within ${PROBE_DEADLINE_SECS}s for cell=$cell" >&2
        echo "error: bridge stderr capture at $samples_dir/bridge-stderr.log" >&2
        _write_probe_diag "$samples_dir" "no BENCH_LATENCY within ${PROBE_DEADLINE_SECS}s timeout"
        return 1
    fi

    # RSS sampling for T4a/T4b: spawn bench-loadgen rss-sample against
    # the contender PID for the duration of the measurement window.
    # INVARIANT: rss_duration_ms covers ONE round only — the outer M2
    # round loop (line ~1280) provides rounds-per-cell multiplexing.
    # Do NOT multiply by M2_ROUNDS here (would capture multiple rounds
    # inside one RSS run and mask per-round dynamics).
    local rss_pid=""
    if [[ "$METRIC_RSS_SAMPLE" == "true" ]]; then
        local rss_duration_ms=$(( (M2_WARMUP_TIME + M2_SAMPLES_PER_ROUND * 10 / 1000) * 1000 ))
        # Cap at 600s for the sampler to avoid runaway.
        rss_duration_ms=$(( rss_duration_ms < 600000 ? rss_duration_ms : 600000 ))
        "${LOADGEN_AFFINITY[@]}" "$loadgen_bin" rss-sample --pid="$child_pid" \
            --interval-ms=10 --duration-ms="$rss_duration_ms" \
            > "$samples_dir/rss-samples.txt" 2>/dev/null &
        rss_pid=$!
    fi

    # Wait for the M2 measurement window to elapse. The contender runs
    # warmup_time + samples_per_round*tick_period seconds (ONE round only;
    # the outer M2 round loop provides rounds-per-cell multiplexing — see
    # brief 2c/2d: "5 rounds per cell (separate process launches)").
    # With T1's current repeatCount=1, the route exits after the first
    # tick — we still wait the full window for fixture parity (the
    # contender will simply have exited early).
    local m2_window_secs=$(( M2_WARMUP_TIME + (M2_SAMPLES_PER_ROUND * 10) / 1000 ))
    # Bound at 600s to avoid runaway smoke tests.
    m2_window_secs=$(( m2_window_secs < 600 ? m2_window_secs : 600 ))

    local elapsed=0
    while [[ $elapsed -lt $m2_window_secs ]]; do
        if ! kill -0 "$CURRENT_TIME_PID" 2>/dev/null; then
            break
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    # Bridge PID tracking — round END (spec §4.11, brief 2f).
    local bridge_pid_end=""
    if [[ -f "$bridge_pid_file" ]]; then
        bridge_pid_end="$(cat "$bridge_pid_file" 2>/dev/null || true)"
    fi
    if [[ "$METRIC_BRIDGE_TRACKING" == "true" && -n "$bridge_pid_start" && -n "$bridge_pid_end" \
          && "$bridge_pid_start" != "$bridge_pid_end" ]]; then
        echo "warn: bridge PID changed mid-round for $cell: $bridge_pid_start → $bridge_pid_end" >&2
        # Round is invalid; harness should re-run (Task 2 smoke does not
        # exercise this path — no T4a/T4b fixtures yet).
    fi

    # KILL the contender (RSS already captured for peak attribution).
    # Tree-aware kill: the rust-camel-cli wrapper uses `setsid` so a
    # bare SIGKILL on the wrapper would orphan the camel run child.
    # The bridge subprocess itself is killed separately via
    # cleanup_bridge_subprocess below (PID file recorded by the
    # bridge-wrapper.sh on spawn).
    if [[ -n "$CURRENT_CHILD_PID" ]]; then
        _kill_process_tree_recursive "$CURRENT_CHILD_PID"
    fi
    kill -KILL "$CURRENT_TIME_PID" 2>/dev/null || true
    wait "$CURRENT_TIME_PID" 2>/dev/null || true
    # CURRENT_CHILD_PID is NOT cleared yet — still needed by _write_probe_diag

    if [[ -n "$rss_pid" ]]; then
        wait "$rss_pid" 2>/dev/null || true
    fi

    # C6: kill any bridge subprocess spawned by this cell. The bridge
    # is NOT a direct child of the time wrapper (camel-bridge spawns
    # it via Command::new().spawn() with its own lifecycle), so the
    # kill above does not reach it. Without this, the bridge leaks
    # across M2 cells and accumulates memory + mTLS ports across the
    # 5h Task 5 run.
    cleanup_bridge_subprocess "$cell"

    # Record exit codes + bridge-stderr.log for diagnostics.
    # (stderr preservation is now done uniformly by _write_probe_diag
    # across all 4 return paths — success and the 3 failure modes.)
    _write_probe_diag "$samples_dir" "completed normally"

    CURRENT_TIME_PID=""; CURRENT_CHILD_PID=""

    # Minimum-count check (spec F2): require AT LEAST M2_SAMPLES_PER_ROUND
    # well-formed records per round. Less = scientifically unusable; the
    # publication gate (Task 11) treats this as a hard fail.
    #
    # Note: 'samples_per_round' is a window-sizing parameter (multiplied by
    # the 10ms tick period to compute m2_window_secs above), not a strict
    # record count target. At scale (samples_per_round=10000) the cell's
    # repeatCount limit (10000 ticks) makes observed match expected exactly.
    # At smoke scale (samples_per_round=10) observed is hundreds (the window
    # allows more emission than the minimum). This check catches "cell died
    # after emitting only N records" partial-data scenarios.
    local observed_samples
    observed_samples=$(grep -c '^BENCH_LATENCY [0-9][0-9]* [0-9][0-9]*$' "$log_file" || true)
    if (( observed_samples < M2_SAMPLES_PER_ROUND )); then
        echo "error: m2 protocol B: sample count below minimum for cell=$cell" >&2
        echo "error: expected>=$M2_SAMPLES_PER_ROUND observed=$observed_samples" >&2
        echo "error: stderr capture at $samples_dir/bridge-stderr.log" >&2
        # Mark the round as a hard failure; m2-summary.json is NOT emitted.
        echo "status=failed reason=insufficient-samples expected_min=$M2_SAMPLES_PER_ROUND observed=$observed_samples" \
            > "$samples_dir/m2-summary.txt"
        return 1
    fi
    echo "m2 protocol B: sample count OK for $cell (observed=$observed_samples, minimum=$M2_SAMPLES_PER_ROUND)" >&2

    # Parse the Protocol B log + emit per-cell summary.
    if [[ ! -s "$log_file" ]]; then
        echo "m2 protocol B: $cell → no per-tick records (fixture may not emit BENCH_LATENCY yet)"
        echo "status=not-measured reason=no-protocol-b-records" > "$samples_dir/m2-summary.txt"
        return 0
    fi
    cp "$log_file" "$samples_dir/protocol-b-samples.txt"
    "$loadgen_bin" parse-protocol-b --json --log="$log_file" > "$samples_dir/m2-summary.json" 2>&1
    # Keep a text version for human debugging.
    "$loadgen_bin" parse-protocol-b --log="$log_file" > "$samples_dir/m2-summary.txt" 2>&1

    echo "m2 protocol B: $cell → see $samples_dir/m2-summary.json"
}

# Measure one cell's Protocol A latency (T3 HTTP server, request-response).
#
# Three steps per cell: (1) harness already calibrated the common rate;
# (2) launch bench-loadgen measure-a against the cell's URL; (3) collect
# per-round latencies for BCa CI.
#
# Args: <cell> <results-dir>
m2_measure_protocol_a() {
    local cell="$1" results_dir="$2"
    local loadgen_bin
    loadgen_bin="$(resolve_loadgen_bin)"

    if [[ "$PROTOCOL_A_COMMON_RATE" -eq 0 ]]; then
        echo "m2 protocol A: $cell → skipped (common rate is 0; no Protocol A fixtures registered)"
        return 0
    fi

    local cell_safe="${cell//\//_}"
    local samples_dir="$results_dir/$cell_safe"
    mkdir -p "$samples_dir"
    local out_file="$samples_dir/protocol-a-summary.txt"

    local url="${PROTOCOL_A_CELL_URL[$cell]:-}"
    if [[ -z "$url" ]]; then
        echo "status=not-measured reason=no-url-for-cell" > "$out_file"
        echo "m2 protocol A: $cell → no URL registered (T3 fixture not present)"
        return 0
    fi

    # Defensive: ensure no stale Protocol A cell is bound to the port
    # from a prior round (C6 hygiene — calibration already kills each
    # cell, but the cleanup_bridge_subprocess call is independent of
    # Protocol A scenarios; here we just clear stale bridge state if
    # the cell somehow registered one).
    cleanup_bridge_subprocess "$cell"

    # Launch the contender (T3 fixture binds 0.0.0.0:8080). The marker
    # BENCH_ROUTE_READY fires after the Jetty/HTTP listener is bound
    # (per Task 3 rc-w1u9 guarantee), so a successful marker observation
    # means the URL is reachable for measure-a.
    if ! launch_contender_wait_marker "$cell"; then
        echo "status=failed reason=launch-failed" > "$out_file"
        echo "m2 protocol A: $cell → launch failed (see stderr above)"
        return 1
    fi

    # Brief 2c: 5 rounds per cell are produced by the outer M2 round
    # loop (each iteration launches a fresh process). This inner call
    # does ONE round of M2_SAMPLES_PER_ROUND samples — do NOT pass
    # M2_ROUNDS here (would result in 5 * 5 = 25 inner rounds, with
    # warmup shared across the inner 5 and rounds 2-5 starting from a
    # warm state, violating the per-round warmup invariant).
    # INVARIANT: BCa seed defaults to 0 (set in bench-loadgen CLI) for bit-reproducibility.
    # Do NOT override with $(date +%s) — the v3 report needs deterministic CIs for bisection.
    # C4: a failing measure-a now propagates non-zero (was silently
    # swallowed by `|| echo`, masking dead contenders as success).
    if ! "${LOADGEN_AFFINITY[@]}" "$loadgen_bin" measure-a --url="$url" \
        --rate="$PROTOCOL_A_COMMON_RATE" \
        --samples="$M2_SAMPLES_PER_ROUND" --rounds=1 \
        --warmup-msgs="$M2_WARMUP_MSGS" --warmup-time="$M2_WARMUP_TIME" \
        --bci-resamples="$M2_BCI_RESAMPLES" \
        --out="$samples_dir/protocol-a-samples.txt" \
        > "$out_file" 2>&1; then
        echo "status=failed reason=measure-a-error" >> "$out_file"
        # Tear down before returning non-zero.
        kill_contender
        cleanup_bridge_subprocess "$cell"
        sleep 0.3
        echo "m2 protocol A: $cell → measure-a failed (see $out_file)" >&2
        return 1
    fi

    # Tear down : kill the contender + any bridge subprocess it may have
    # spawned (T3 has no bridge, but the cleanup is a no-op then).
    kill_contender
    cleanup_bridge_subprocess "$cell"
    # Brief pause for kernel to release the listener socket for the
    # next Protocol A cell's bind().
    sleep 0.3

    echo "m2 protocol A: $cell → see $out_file"
}

# (PROTOCOL_A_CELL_URL is declared alongside PROTOCOL_A_CELLS at the
# top of the file, near the other per-cell maps.)

# =====================================================================
# M3 sustained throughput + M4 memory growth under load
# (v3.5 extension: bd rc-??? — T3 http-server only)
#
# For each T3 (http-server) cell registered in PROTOCOL_A_CELLS, run
# M3_ROUNDS rounds of:
#   1. Launch the contender under GNU time, wait for BENCH_ROUTE_READY
#   2. Start rss-sample --json in background (M4 memory-growth trace)
#   3. Run bench-loadgen measure-throughput (M3 sustained throughput)
#   4. Kill the contender's process tree
#   5. Save the rss-sample output to m4-round-N/<cell_safe>.json
# After all rounds, aggregate the per-round mean_msgs_per_sec into a
# per-cell m3-summary.json (median, min, max, per_round_means).
#
# The m3 + m4 paths share a single function: launching the contender
# (for sustained load) and killing the tree (to release 0.0.0.0:8080)
# is identical to M2 Protocol A; the new work is the parallel
# rss-sample trace and the per-round throughput aggregation.
# =====================================================================

m3_measure() {
    if [[ ${#PROTOCOL_A_CELLS[@]} -eq 0 ]]; then
        echo "m3: skipped (no Protocol A cells registered; M3 only targets T3 http-server)" >&2
        return 0
    fi
    local loadgen_bin
    loadgen_bin="$(resolve_loadgen_bin)"

    # T5: seeded randomized-block design. Instead of running all rounds
    # of cell1, then cell2, … (which lets systematic drift accumulate
    # within a cell), each round shuffles the cell list so every cell
    # pays the same temporal-position cost. The seed allows env override
    # (BENCH_SEED) for bit-reproducible re-runs of a published matrix.
    BENCH_SEED="${BENCH_SEED:-$(date +%s)}"
    echo "m3: randomized-block seed = $BENCH_SEED"

    # Per-cell failure state (C4: surface silent failures instead of
    # silently producing a median over partial / HTTP-errored data).
    local -A m3_cell_failed=()
    local -A m3_cell_reasons=()
    local cell
    for cell in "${PROTOCOL_A_CELLS[@]}"; do
        m3_cell_failed[$cell]=0
        m3_cell_reasons[$cell]=""
    done

    # Mutable copy of the cell list (shuffle is in-place on this copy).
    local -a proto_cells=("${PROTOCOL_A_CELLS[@]}")
    # round_orders[r] → space-separated cell order (for the order JSON).
    local -a round_orders=()

    # ===================================================================
    # Phase 1: randomized-block measurement (round-major).
    # ===================================================================
    local r
    for ((r = 0; r < M3_ROUNDS; r++)); do
        mkdir -p "$RUN_DIR/m3-round-$r" "$RUN_DIR/m4-round-$r"
        _shuffle_seeded proto_cells $(( BENCH_SEED + r ))
        round_orders+=("${proto_cells[*]}")
        for cell in "${proto_cells[@]}"; do
            local cell_safe="${cell//\//_}"
            echo "m3: $cell round $r → ${M3_DURATION_SECS}s sustained throughput (+ ${M3_WARMUP_SECS}s warmup)"
            local m3_dir="$RUN_DIR/m3-round-$r"
            local m4_dir="$RUN_DIR/m4-round-$r"
            local m3_out="$m3_dir/${cell_safe}.json"
            local m4_out="$m4_dir/${cell_safe}.json"

            # Defensive: clear stale bridge state (T3 has no bridge).
            cleanup_bridge_subprocess "$cell"

            # 1. Launch the contender under GNU time, wait for the marker.
            # Sets CURRENT_TIME_PID + CURRENT_CHILD_PID on success.
            if ! launch_contender_wait_marker "$cell"; then
                m3_cell_failed[$cell]=1
                m3_cell_reasons[$cell]+="r${r}:launch-failed"$'\n'
                echo "m3: $cell round $r → launch failed (see stderr above); skipping round" >&2
                continue
            fi

            # 2. Start rss-sample in background AFTER warmup so the trace
            # aligns with the M3 measurement window.
            local rss_pid=""
            ( sleep "$M3_WARMUP_SECS" && "${LOADGEN_AFFINITY[@]}" "$loadgen_bin" rss-sample --json \
                --pid="$CURRENT_CHILD_PID" \
                --interval-ms=2000 \
                --duration-ms=$((M3_DURATION_SECS * 1000)) \
                > "$m4_out" 2>&1 ) &
            rss_pid=$!
            CURRENT_RSS_PID="$rss_pid"

            # 3. Run sustained throughput measurement (loadgen pinned to
            # LOADGEN_CPUSET so a saturated contender cannot starve it).
            # C4: a non-zero exit OR HTTP errors (total_non_2xx /
            # total_errors) invalidate the whole cell.
            if ! "${LOADGEN_AFFINITY[@]}" "$loadgen_bin" measure-throughput \
                --url="$BENCH_HTTP_URL" \
                --duration-secs=$M3_DURATION_SECS \
                --warmup-secs=$M3_WARMUP_SECS \
                --out="$m3_out" 2>"$m3_dir/${cell_safe}.err"; then
                m3_cell_failed[$cell]=1
                m3_cell_reasons[$cell]+="r${r}:measure-throughput-exit-nonzero"$'\n'
                echo "warn: m3: $cell round $r → measure-throughput failed (see $m3_dir/${cell_safe}.err)" >&2
            elif [[ ! -f "$m3_out" || ! -s "$m3_out" ]]; then
                # C4: exit 0 but no (or empty) output file — the loadgen
                # produced nothing usable; treat as a hard cell failure
                # rather than silently dropping the round.
                m3_cell_failed[$cell]=1
                m3_cell_reasons[$cell]+="r${r}:no-output-file"$'\n'
                echo "warn: m3: $cell round $r → measure-throughput exited 0 but produced no output ($m3_out); cell invalidated" >&2
            else
                # File exists and is non-empty — validate it parses and
                # check HTTP errors + zero-throughput rounds (C4: a round
                # with zero successful requests must invalidate the cell,
                # not be silently excluded at aggregation).
                local non_2xx errors total_2xx
                non_2xx="$(jq -r '.total_non_2xx // 0' "$m3_out" 2>/dev/null || echo PARSE_ERR)"
                errors="$(jq -r '.total_errors // 0' "$m3_out" 2>/dev/null || echo PARSE_ERR)"
                total_2xx="$(jq -r '.total_2xx // 0' "$m3_out" 2>/dev/null || echo PARSE_ERR)"
                if [[ "$non_2xx" == "PARSE_ERR" || "$errors" == "PARSE_ERR" || "$total_2xx" == "PARSE_ERR" ]]; then
                    m3_cell_failed[$cell]=1
                    m3_cell_reasons[$cell]+="r${r}:output-unparseable"$'\n'
                    echo "warn: m3: $cell round $r → output file unparseable (jq failed on $m3_out); cell invalidated" >&2
                elif (( non_2xx > 0 || errors > 0 )); then
                    m3_cell_failed[$cell]=1
                    m3_cell_reasons[$cell]+="r${r}:http-errors-non2xx=${non_2xx}-errors=${errors}"$'\n'
                    echo "warn: m3: $cell round $r → HTTP errors (non_2xx=$non_2xx errors=$errors); cell invalidated" >&2
                elif (( total_2xx <= 0 )); then
                    m3_cell_failed[$cell]=1
                    m3_cell_reasons[$cell]+="r${r}:zero-2xx-requests"$'\n'
                    echo "warn: m3: $cell round $r → zero successful (2xx) requests; cell invalidated" >&2
                else
                    # Validate mean_msgs_per_sec is present and positive.
                    local mean_valid
                    mean_valid="$(jq -r '(.mean_msgs_per_sec // 0) | (type == "number") and (. > 0)' "$m3_out" 2>/dev/null || echo false)"
                    if [[ "$mean_valid" != "true" ]]; then
                        m3_cell_failed[$cell]=1
                        m3_cell_reasons[$cell]+="r${r}:invalid-mean"$'\n'
                        echo "warn: m3: $cell round $r → mean_msgs_per_sec missing or non-positive; cell invalidated" >&2
                    fi
                fi
            fi

            # 4. Kill the contender's process tree (releases 0.0.0.0:8080).
            kill_contender

            # 5. Reap the rss-sample background process.
            if [[ -n "$rss_pid" ]] && kill -0 "$rss_pid" 2>/dev/null; then
                kill -KILL "$rss_pid" 2>/dev/null || true
                wait "$rss_pid" 2>/dev/null || true
            fi
            CURRENT_RSS_PID=""

            cleanup_bridge_subprocess "$cell"
            sleep 0.3
        done
    done

    # Persist the realized randomized-block order (T5 reproducibility).
    {
        local rounds_json="[" _r _order _arr
        for ((_r = 0; _r < M3_ROUNDS; _r++)); do
            _order="${round_orders[$_r]:-}"
            # word-split the space-separated order deliberately.
            # shellcheck disable=SC2206
            local _cells=($_order)
            _arr="$(printf '%s\n' "${_cells[@]}" | jq -R . | jq -s . 2>/dev/null || echo '[]')"
            [[ "$_r" -gt 0 ]] && rounds_json+=","
            rounds_json+="$_arr"
        done
        rounds_json+="]"
        jq -n --argjson seed "$BENCH_SEED" --argjson order "$rounds_json" \
            '{seed:$seed, order:$order}' > "$RUN_DIR/measurement_order.json"
    }
    echo "m3: measurement order → $RUN_DIR/measurement_order.json"

    # ===================================================================
    # Phase 2: per-cell aggregation (M3 throughput + M4 memory growth).
    # ===================================================================
    for cell in "${PROTOCOL_A_CELLS[@]}"; do
        local cell_safe="${cell//\//_}"
        local samples_dir="$RUN_DIR/$cell_safe"
        mkdir -p "$samples_dir"
        echo "m3: aggregating $cell"

        # Collect per-round means (empty / missing → 0 so the summary is
        # always well-formed JSON).
        local -a round_means=()
        for ((r = 0; r < M3_ROUNDS; r++)); do
            local m3_file="$RUN_DIR/m3-round-$r/${cell_safe}.json"
            if [[ -f "$m3_file" ]]; then
                local v
                v="$(jq -r '.mean_msgs_per_sec // 0' "$m3_file" 2>/dev/null || echo 0)"
                round_means+=("$v")
            else
                round_means+=("0")
            fi
        done

        # Build the per_round_means JSON array (numbers, not strings).
        local per_round_json
        per_round_json="$(printf '%s\n' "${round_means[@]}" \
            | jq -R 'tonumber? // 0' | jq -s .)"

        # C4: if any round failed, do NOT compute a median over partial
        # data — emit a failed summary with the recorded failure reasons.
        if [[ "${m3_cell_failed[$cell]}" == "1" ]]; then
            local failed_json
            failed_json="$(printf '%s' "${m3_cell_reasons[$cell]}" \
                | jq -Rsc 'split("\n") | map(select(length>0))')"
            jq -n \
                --arg     cell            "$cell" \
                --argjson per_round_means "$per_round_json" \
                --argjson rounds          "$M3_ROUNDS" \
                --argjson duration_secs   "$M3_DURATION_SECS" \
                --argjson warmup_secs     "$M3_WARMUP_SECS" \
                --argjson failed_rounds   "$failed_json" \
                '{
                    cell: $cell,
                    status: "failed",
                    rounds: $rounds,
                    duration_secs: $duration_secs,
                    warmup_secs: $warmup_secs,
                    per_round_means: $per_round_means,
                    failed_rounds: $failed_rounds
                }' > "$samples_dir/m3-summary.json"
            echo "m3: $cell → FAILED (status=failed written to $samples_dir/m3-summary.json)" >&2
        else
            # Compute median, min, max via awk over the newline-joined
            # list. Drop zero/empty values so a partially-failed run
            # reports the median of the *successful* rounds.
            local stats
            stats="$(printf '%s\n' "${round_means[@]}" | awk '
                NF == 0 { next }
                { v = $1 + 0; if (v <= 0) next
                  vals[count++] = v
                  if (count == 1 || v < min) min = v
                  if (count == 1 || v > max) max = v
                }
                END {
                    if (count == 0) { print "0|0|0|0"; exit }
                    for (i = 0; i < count-1; i++) {
                        for (j = i+1; j < count; j++) {
                            if (vals[i] > vals[j]) { t = vals[i]; vals[i] = vals[j]; vals[j] = t }
                        }
                    }
                    median = (count % 2 == 1) ? vals[int(count/2)] : (vals[count/2 - 1] + vals[count/2]) / 2
                    printf "%.6f|%.6f|%.6f|%d\n", median, min, max, count
                }
            ')"
            local median_mean min_mean max_mean n_valid
            IFS='|' read -r median_mean min_mean max_mean n_valid <<< "$stats"

            # Write the per-cell summary.
            jq -n \
                --arg     cell            "$cell" \
                --argjson median_mean     "$median_mean" \
                --argjson min_mean        "$min_mean" \
                --argjson max_mean        "$max_mean" \
                --argjson per_round_means "$per_round_json" \
                --argjson rounds          "$M3_ROUNDS" \
                --argjson duration_secs   "$M3_DURATION_SECS" \
                --argjson warmup_secs     "$M3_WARMUP_SECS" \
                '{
                    cell: $cell,
                    status: "ok",
                    median_mean_msgs_per_sec: $median_mean,
                    min_mean: $min_mean,
                    max_mean: $max_mean,
                    per_round_means: $per_round_means,
                    rounds: $rounds,
                    duration_secs: $duration_secs,
                    warmup_secs: $warmup_secs
                }' > "$samples_dir/m3-summary.json"
            echo "m3: $cell → summary at $samples_dir/m3-summary.json (n_valid=$n_valid)"
        fi

        # ---- Aggregate M4 (C1 fix): collect ALL rounds' rss deltas
        # (including negatives), compute median/p25/p75, and select the
        # round whose delta is closest to the median as the representative
        # trace. The previous "largest delta" heuristic biased toward
        # noise spikes and silently dropped cells with memory shrinkage.
        # Each round's M4 file contains:
        #   BENCH_RSS_OUTCOME max_summed_vmrss_kib=… …
        #   BENCH_RSS_SAMPLES_JSON [{sample_offset_ns, summed_vmrss_kib, …}]
        local -a m4_initial=() m4_final=() m4_max=() m4_delta=() m4_samples=() m4_round=()
        for ((r = 0; r < M3_ROUNDS; r++)); do
            local m4_file="$RUN_DIR/m4-round-$r/${cell_safe}.json"
            [[ -f "$m4_file" ]] || continue
            local json_line
            json_line="$(grep 'BENCH_RSS_SAMPLES_JSON' "$m4_file" | tail -1 | sed 's/^BENCH_RSS_SAMPLES_JSON //')"
            [[ -n "$json_line" ]] || continue
            local n_samples rss_initial rss_final rss_max
            n_samples="$(printf '%s' "$json_line" | jq 'length')"
            [[ "$n_samples" -gt 0 ]] || continue
            rss_initial="$(printf '%s' "$json_line" | jq '.[0].summed_vmrss_kib')"
            rss_final="$(printf '%s' "$json_line" | jq '.[-1].summed_vmrss_kib')"
            rss_max="$(printf '%s' "$json_line" | jq '[.[].summed_vmrss_kib] | max')"
            m4_initial+=("$rss_initial")
            m4_final+=("$rss_final")
            m4_max+=("$rss_max")
            m4_delta+=("$((rss_final - rss_initial))")
            m4_samples+=("$json_line")
            m4_round+=("$r")
        done

        if [[ ${#m4_delta[@]} -eq 0 ]]; then
            echo "m4: $cell → no valid RSS traces found; skipping summary" >&2
        else
            # Sort deltas ascending for distribution stats.
            local -a sorted_delta=()
            IFS=$'\n' sorted_delta=($(printf '%s\n' "${m4_delta[@]}" | sort -n))
            unset IFS
            local nd=${#sorted_delta[@]}
            # Nearest-rank percentiles (1-indexed).
            local p25_idx=$(( (nd * 25 + 99) / 100 )); (( p25_idx < 1 )) && p25_idx=1; (( p25_idx > nd )) && p25_idx=nd
            local p75_idx=$(( (nd * 75 + 99) / 100 )); (( p75_idx < 1 )) && p75_idx=1; (( p75_idx > nd )) && p75_idx=nd
            local delta_p25="${sorted_delta[$((p25_idx-1))]}"
            local delta_p75="${sorted_delta[$((p75_idx-1))]}"
            local delta_min="${sorted_delta[0]}"
            local delta_max="${sorted_delta[$((nd-1))]}"
            local delta_median
            if (( nd % 2 == 1 )); then
                delta_median="${sorted_delta[$((nd/2))]}"
            else
                delta_median=$(( (sorted_delta[$((nd/2 - 1))] + sorted_delta[$((nd/2))]) / 2 ))
            fi

            # Select the round whose delta is closest to the median
            # (tie-break: lowest round index).
            local best_round="${m4_round[0]}" best_idx=0 best_dist="" di dist
            for ((di = 0; di < ${#m4_delta[@]}; di++)); do
                dist=$(( ${m4_delta[$di]} - delta_median )); (( dist < 0 )) && dist=$(( -dist ))
                if [[ -z "$best_dist" ]] || (( dist < best_dist )); then
                    best_dist="$dist"; best_round="${m4_round[$di]}"; best_idx="$di"
                fi
            done
            local best_initial="${m4_initial[$best_idx]}"
            local best_final="${m4_final[$best_idx]}"
            local best_max="${m4_max[$best_idx]}"
            local best_samples="${m4_samples[$best_idx]}"

            local delta_dist_json
            delta_dist_json="$(printf '%s\n' "${m4_delta[@]}" | jq -R 'tonumber' | jq -s .)"

            jq -n \
                --arg     cell           "$cell" \
                --argjson rss_initial    "$best_initial" \
                --argjson rss_final      "$best_final" \
                --argjson rss_delta      "${m4_delta[$best_idx]}" \
                --argjson rss_max        "$best_max" \
                --argjson representative_round "$best_round" \
                --argjson duration_secs  "$M3_DURATION_SECS" \
                --argjson interval_ms    2000 \
                --argjson delta_median   "$delta_median" \
                --argjson delta_min      "$delta_min" \
                --argjson delta_max      "$delta_max" \
                --argjson delta_p25      "$delta_p25" \
                --argjson delta_p75      "$delta_p75" \
                --argjson delta_distribution "$delta_dist_json" \
                --slurpfile samples /dev/stdin \
                '{
                    cell: $cell,
                    rss_initial_kib: $rss_initial,
                    rss_final_kib: $rss_final,
                    rss_delta_kib: $rss_delta,
                    rss_max_kib: $rss_max,
                    representative_round: $representative_round,
                    measurement_window_secs: $duration_secs,
                    interval_ms: $interval_ms,
                    delta_median: $delta_median,
                    delta_min: $delta_min,
                    delta_max: $delta_max,
                    delta_p25: $delta_p25,
                    delta_p75: $delta_p75,
                    delta_distribution: $delta_distribution,
                    rss_samples: $samples[0]
                }' <<< "$best_samples" > "$samples_dir/m4-summary.json"
            echo "m4: $cell → summary at $samples_dir/m4-summary.json (delta=${m4_delta[$best_idx]} KiB, median=$delta_median KiB, round=$best_round, n=${#m4_delta[@]})"
        fi
    done
}

# T2 baseline validation: run the loopback bench-devnull baseline before
# and after the M3 matrix. The devnull throughput is the upper bound on
# what the loadgen can push through the kernel alone — if a real
# contender approaches it (baseline < 1.25× fastest), the loadgen is
# saturated and the measurement is loadgen-capped, not contender-capped.
# Recovery: re-split to give the loadgen more cores and rerun once.
run_m3_with_validation() {
    local pre_baseline fastest post_baseline need_resplit
    # bench_devnull_baseline returns non-zero (echoing "0") when the
    # devnull stub fails to bind — guard with `|| true` so errexit does
    # not abort; the "0" value flows into the validation logic correctly.
    pre_baseline="$(bench_devnull_baseline)" || true
    echo "baseline (pre-matrix): ${pre_baseline} req/s"

    m3_measure

    fastest="$(fastest_contender_throughput)"
    echo "fastest contender median: ${fastest} req/s"

    post_baseline="$(bench_devnull_baseline)" || true
    echo "baseline (post-matrix): ${post_baseline} req/s"

    need_resplit=0
    if baseline_below "$pre_baseline" "$fastest"; then
        need_resplit=1
        echo "warn: pre-baseline ${pre_baseline} < 1.25×fastest (${fastest}); loadgen may be capped" >&2
    fi
    if baseline_below "$post_baseline" "$fastest"; then
        need_resplit=1
        echo "warn: post-baseline ${post_baseline} < 1.25×fastest (${fastest}); loadgen may be capped" >&2
    fi

    if (( need_resplit == 1 )); then
        echo "--- re-split: SERVER=2 physical cores, LOADGEN=rest; rerunning matrix ONCE ---"
        detect_cpu_topology 2
        write_provenance
        m3_measure
        fastest="$(fastest_contender_throughput)"
        local re_post
        re_post="$(bench_devnull_baseline)" || true
        echo "baseline (post-resplit): ${re_post} req/s; fastest=${fastest} req/s"
        if baseline_below "$re_post" "$fastest"; then
            echo "FATAL: loadgen-capped (baseline ${re_post} < 1.25×fastest ${fastest} after re-split), aborting" >&2
            exit 1
        fi
    fi
}

# =====================================================================
# Main
# =====================================================================

echo "=== benchmark harness v2 (bd rc-p9ki) + v3 M2 extension (bd rc-2vxg) + v3.5 M3/M4 extension ==="
echo "scenarios: ${SCENARIOS_FILTER:-<auto-discover>}"
echo "metric: $METRIC"
echo "M1: n=$N (+ $WARMUP_N warmup, discarded)"
if [[ "$METRIC" == "m2" || "$METRIC" == "m1+m2" ]]; then
    echo "M2: warmup_time=${M2_WARMUP_TIME}s warmup_msgs=${M2_WARMUP_MSGS} rounds=${M2_ROUNDS} samples_per_round=${M2_SAMPLES_PER_ROUND}"
fi
if [[ "$METRIC" == *"m3"* ]]; then
    echo "M3/M4: rounds=${M3_ROUNDS} duration_secs=${M3_DURATION_SECS} warmup_secs=${M3_WARMUP_SECS} (T3 http-server only)"
fi
echo "dry-run: $DRY_RUN"

# Step 1: discover scenarios.
discover_scenarios
echo "resolved scenarios: ${SCENARIOS[*]}"

# Step 2: pre-flight toolchain (GraalVM, native-image, gradle).
# Runs even on --dry-run because the user might be dry-running after
# a clean checkout, and a missing toolchain is exactly the kind of
# problem --dry-run should surface.
preflight_toolchain

# Step 3: resolve all per-cell artifacts. This builds (or skips via
# fingerprint) the 2 native artifacts per scenario, then resolves the
# 6 cells per non-bridge scenario (T4a/T4b stay at 4 — bridge
# asymmetric matrix per spec §4.3 + e_opus round-5 verdict: bridge
# tax is authoring-format-invariant).
resolve_all_cells

# Compute the expected cell count scenario-aware (T1/T2/T3 = 6 each,
# T4a/T4b = 4 each per spec §4.3 + e_opus round 5 won't-measure
# scope: bridge-YAML cells recorded as won't-measure in COVERAGE.md).
expected_cells=0
for _s in "${SCENARIOS[@]}"; do
    case "${SCENARIO_ARTIFACT_SET[$_s]:-full}" in
        bridge) expected_cells=$((expected_cells + 4)) ;;
        *)      expected_cells=$((expected_cells + 6)) ;;
    esac
done
echo "resolved cells: ${#CELLS[@]} (expected: $expected_cells)"
if [[ ${#CELLS[@]} -ne $expected_cells ]]; then
    echo "error: expected $expected_cells cells, got ${#CELLS[@]}" >&2
    exit 1
fi

# Step 4: --dry-run prints the shuffled cell list and exits 0.
if [[ "$DRY_RUN" == "true" ]]; then
    shuffle_cells
    echo "=== dry-run: ${#CELLS[@]}-cell shuffled order ==="
    dryrun_i=0
    for cell in "${CELLS[@]}"; do
        dryrun_i=$((dryrun_i + 1))
        dryrun_scenario="${CELL_SCENARIO[$cell]}"
        dryrun_proto="${SCENARIO_M2_PROTOCOL[$dryrun_scenario]:-?}"
        dryrun_skip="${CELL_WONT_MEASURE[$cell]:-}"
        if [[ -n "$dryrun_skip" ]]; then
            echo "  $dryrun_i. $cell (SKIP: $dryrun_skip)"
        elif [[ "$METRIC" == "m1" ]]; then
            echo "  $dryrun_i. $cell (M1 only)"
        else
            echo "  $dryrun_i. $cell (M1 + M2 protocol $dryrun_proto)"
        fi
    done
    echo "=== dry-run complete (no contender invoked) ==="
    exit 0
fi

# Step 5: create the per-run results dir.
mkdir -p "$RUN_DIR"
echo "results dir: $RUN_DIR"

# T2: CPU topology detection + provenance. Only meaningful for M3
# throughput runs (where server/loadgen core isolation prevents the
# contender from starving the loadgen). For cold-start-only runs the
# default empty affinity arrays are no-ops at every launch site.
if [[ "$METRIC" == *"m3"* ]]; then
    detect_cpu_topology
    write_provenance
fi

# -- M1 measurement (preserved verbatim from v2; runs only when metric
#    includes m1). All 12 v2 M1 invariants from spec §4.11 hold: n=50 +
#    3 warmup discarded, 1ms polling, 30s deadline, scenario-aware marker
#    validation, KILL after marker, env-var hygiene, GNU time RSS, raw
#    samples per cell, randomized cell ordering across FULL cell set. --
if [[ "$METRIC" == "m1" || "$METRIC" == "m1+m2" ]]; then
    # -- warmup (v1 Fix 1: warmup failures are FATAL, not skipped) --
    # A failed warmup means the smoke test was lying — the real
    # measured runs would silently drop data points. Stop the whole run
    # instead.
    echo "--- M1 warmup ($WARMUP_N runs per cell, discarded) ---"
    for ((w = 0; w < WARMUP_N; w++)); do
        shuffle_cells
        for cell in "${CELLS[@]}"; do
            if [[ -n "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
                echo "SKIP: $cell (M1 warmup) — ${CELL_WONT_MEASURE[$cell]}"
                continue
            fi
            measure_once "$cell" /dev/null \
                || { echo "FATAL: M1 warmup run failed for $cell — see error above; aborting before measured runs" >&2; exit 1; }
        done
    done

    # -- measured runs --
    echo "--- M1 measured runs ($N per cell) ---"
    for cell in "${CELLS[@]}"; do
        : > "$SCRATCH_DIR/${cell//\//_}.txt"   # fresh results file
    done
    for ((r = 0; r < N; r++)); do
        shuffle_cells
        for cell in "${CELLS[@]}"; do
            if [[ -n "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
                echo "SKIP: $cell (M1 round $r) — ${CELL_WONT_MEASURE[$cell]}"
                continue
            fi
            measure_once "$cell" "$SCRATCH_DIR/${cell//\//_}.txt"
        done
    done

    # -- save raw samples to RUN_DIR (spec §7.2 retention requirement) --
    echo "--- saving M1 raw samples to $RUN_DIR ---"
    for cell in "${CELLS[@]}"; do
        local_safe="${cell//\//_}"
        local_target="$RUN_DIR/$local_safe"
        mkdir -p "$local_target"
        cp "$SCRATCH_DIR/$local_safe.txt" "$local_target/samples.txt"
    done
fi

# -- M2 measurement (spec §4.1–4.11, brief task 2c–2k). Runs only when
#    metric includes m2. Per-cell protocol (A or B) is registered in
#    SCENARIO_M2_PROTOCOL. --
if [[ "$METRIC" == "m2" || "$METRIC" == "m1+m2" ]]; then
    echo "--- M2 measurement (warmup ${M2_WARMUP_TIME}s OR ${M2_WARMUP_MSGS} msgs, $M2_ROUNDS rounds × $M2_SAMPLES_PER_ROUND samples) ---"
    local_loadgen_bin="$(resolve_loadgen_bin)"

    # Protocol A calibration phase (spec §4.4). Launches each Protocol
    # A cell in turn, calibrates against its URL, kills it, then takes
    # the MIN rate across cells. PROTOCOL_A_CELLS is populated by
    # resolve_all_cells from SCENARIO_M2_PROTOCOL.
    if [[ ${#PROTOCOL_A_CELLS[@]} -gt 0 ]]; then
        echo "-- Protocol A calibration --"
        calibrate_protocol_a "$local_loadgen_bin"
    else
        echo "-- Protocol A calibration skipped (no Protocol A cells registered) --"
        PROTOCOL_A_COMMON_RATE=0
    fi

    # M2 rounds — same shuffle semantics as M1 (randomized across FULL
    # cell set per spec §4.11).
    for ((r = 0; r < M2_ROUNDS; r++)); do
        shuffle_cells
        for cell in "${CELLS[@]}"; do
            if [[ -n "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
                echo "SKIP: $cell (M2 round $r) — ${CELL_WONT_MEASURE[$cell]}"
                continue
            fi
            m2_scenario="${CELL_SCENARIO[$cell]}"
            m2_proto="${SCENARIO_M2_PROTOCOL[$m2_scenario]:-B}"
            # Per-cell feature activation (brief 2f/2g). Bridge PID
            # tracking + process-tree RSS sampling are only meaningful
            # for t4a/t4b cells where a bridge subprocess is actually
            # spawned. Defaults off; opt-in per scenario name so cells
            # from different scenarios in the same round loop do not
            # leak feature state into each other.
            METRIC_BRIDGE_TRACKING=false
            METRIC_RSS_SAMPLE=false
            case "$m2_scenario" in
                xslt-bridge|xsd-validation-bridge)
                    # Bridge scenarios spawn a subprocess (bridges/xml)
                    # that the route talks to via gRPC mTLS. PID tracking
                    # detects mid-round bridge restarts (invalidates the
                    # round); RSS sampling aggregates contender + bridge
                    # VmRSS at 10ms cadence per spec §4.9.
                    METRIC_BRIDGE_TRACKING=true
                    METRIC_RSS_SAMPLE=true
                    ;;
            esac
            if [[ "$m2_proto" == "A" ]]; then
                m2_measure_protocol_a "$cell" "$RUN_DIR/m2-round-$r" || \
                    echo "warn: m2 protocol A $cell failed (round $r); continuing" >&2
            else
                m2_measure_protocol_b "$cell" "$RUN_DIR/m2-round-$r" || \
                    echo "warn: m2 protocol B $cell failed (round $r); continuing" >&2
            fi
        done
    done
fi

# -- M3 + M4 measurement (sustained throughput + memory growth under
#    load). Runs only when the metric includes m3. Targets T3
#    http-server cells (PROTOCOL_A_CELLS); no-op for non-T3 scenarios.
if [[ "$METRIC" == *"m3"* ]]; then
    echo "--- M3 + M4 measurement (${M3_ROUNDS} rounds × ${M3_DURATION_SECS}s sustained throughput + RSS trace at 2s cadence) ---"
    run_m3_with_validation
fi

# -- summary: per-scenario per-pair output (v1 had per-pair; v2 groups
# by scenario too) --
# v3.5: camel-quarkus-dsl + camel-quarkus-yaml (JVM mode) dropped —
# redundant with camel-standalone-{dsl,yaml} baseline. Total: 3
# contenders per pair × 5 scenarios = 26 cells (was 32).
declare -a PAIR_A_CONTENDERS=(camel-standalone-dsl camel-quarkus-dsl-native rust-camel-lib)
declare -a PAIR_B_CONTENDERS=(camel-standalone-yaml camel-quarkus-yaml-native rust-camel-cli)
declare -A PAIR_LABELS=(
    ["camel-standalone-dsl"]="Pair A"
    ["camel-quarkus-dsl-native"]="Pair A (native)"
    ["rust-camel-lib"]="Pair A"
    ["camel-standalone-yaml"]="Pair B"
    ["camel-quarkus-yaml-native"]="Pair B (native)"
    ["rust-camel-cli"]="Pair B"
)

for scenario in "${SCENARIOS[@]}"; do
    echo ""
    echo "=== scenario: $scenario ==="
    echo "=== Pair A (embedded runtime, no parsing) — time_ms / rss_kb ==="
    for contender in "${PAIR_A_CONTENDERS[@]}"; do
        cell="$scenario/$contender"
        local_safe="${cell//\//_}"
        if [[ -n "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
            echo "  $contender: SKIP — ${CELL_WONT_MEASURE[$cell]}"
        elif [[ -s "$SCRATCH_DIR/$local_safe.txt" ]]; then
            echo -n "  $contender time: "; summarize 1 "$SCRATCH_DIR/$local_safe.txt"
            echo -n "  $contender rss:  "; summarize 2 "$SCRATCH_DIR/$local_safe.txt"
        else
            echo "  $contender: no data (no samples in $SCRATCH_DIR/$local_safe.txt)"
        fi
    done
    echo "=== Pair B (YAML-authored route, parsed at runtime) — time_ms / rss_kb ==="
    for contender in "${PAIR_B_CONTENDERS[@]}"; do
        cell="$scenario/$contender"
        local_safe="${cell//\//_}"
        if [[ -n "${CELL_WONT_MEASURE[$cell]:-}" ]]; then
            echo "  $contender: SKIP — ${CELL_WONT_MEASURE[$cell]}"
        elif [[ -s "$SCRATCH_DIR/$local_safe.txt" ]]; then
            echo -n "  $contender time: "; summarize 1 "$SCRATCH_DIR/$local_safe.txt"
            echo -n "  $contender rss:  "; summarize 2 "$SCRATCH_DIR/$local_safe.txt"
        else
            echo "  $contender: no data"
        fi
    done
done

echo ""
echo "=== raw samples: $RUN_DIR ==="
echo "=== done ==="
