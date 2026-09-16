#!/usr/bin/env bash
# Local cold-start + RSS timer — mission 99 replica (M1-replica method),
# mission 102 (clidiet). Self-contained: fixture beside this script,
# results in results/ beside this script. Adds Maximum-RSS extraction
# (per sample, from /usr/bin/time -v) which the original omits.
#
# Usage: bench-cold-local.sh marker|help <bin> <n> <tag>
#   marker mode argv: run --config fixture/Camel.toml --routes fixture/routes/startup-minimal.yaml --no-watch
#   help mode argv:   --help
# Prints: "<tag> <mode> <ms>" per sample to stdout; raw ms to
#   results/<tag>.<mode>.txt; max-RSS-kb to results/<tag>.<mode>.rss.txt
# 3 warmup discarded (v1 Fix 1).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
FIXTURE="$HERE/fixture-startup-minimal"
RESULTS="$HERE/results"; mkdir -p "$RESULTS"
SCRATCH="${SCRATCH:-/tmp/m102}"; mkdir -p "$SCRATCH"

mode="$1"; bin="$2"; n="${3:-50}"; tag="$4"
MARKER="BENCH_ROUTE_READY"
TIME_BIN="${TIME_BIN:-/run/current-system/sw/bin/time}"

# cleanup_tree <time_pid>: kill the descendants of the measured binary,
# then the measured binary itself, then reap /usr/bin/time WITHOUT
# killing it — time emits its -v summary (incl. max RSS) only when the
# measured process it wraps has been reaped. Killing time first loses the
# summary (bless r3 finding).
cleanup_tree() {
    local tpid="$1" child
    child=$(awk '{print $1}' "/proc/$tpid/task/$tpid/children" 2>/dev/null)
    if [[ -n "$child" ]]; then
        pkill -KILL -P "$child" 2>/dev/null
        kill -KILL "$child" 2>/dev/null
    fi
    wait "$tpid" 2>/dev/null
}

measure_one() {
    local out="$SCRATCH/out.$$.$RANDOM" tf="$SCRATCH/tf.$$.$RANDOM"
    local start_ns elapsed pid child rss
    start_ns=$(date +%s%N)
    if [[ "$mode" == "marker" ]]; then
        "$TIME_BIN" -v -o "$tf" "$bin" run \
            --config "$FIXTURE/Camel.toml" \
            --routes "$FIXTURE/routes/startup-minimal.yaml" \
            --no-watch > "$out" 2>&1 &
    else
        "$TIME_BIN" -v -o "$tf" "$bin" --help > "$out" 2>&1 &
    fi
    pid=$!
    local deadline=$(( start_ns + 30000000000 ))
    if [[ "$mode" == "marker" ]]; then
        while ! grep -qF "$MARKER" "$out" 2>/dev/null; do
            if ! kill -0 "$pid" 2>/dev/null; then
                grep -qF "$MARKER" "$out" 2>/dev/null && break
                echo "ERROR-exit-before-marker" >&2; cleanup_tree "$pid"; rm -f "$out" "$tf"; return 1
            fi
            if [[ $(date +%s%N) -gt $deadline ]]; then
                echo "ERROR-deadline" >&2; cleanup_tree "$pid"; rm -f "$out" "$tf"; return 1
            fi
            sleep 0.001
        done
    else
        wait "$pid" 2>/dev/null
    fi
    elapsed=$(( ($(date +%s%N) - start_ns) / 1000000 ))
    cleanup_tree "$pid"
    # M102 addition: max RSS from time -v (kbytes). cleanup_tree kills the
    # measured binary but NOT time; time then reaps it, writes its -v
    # summary, and exits — only after that may the file be parsed.
    rss=$(grep -oE 'Maximum resident set size \(kbytes\): [0-9]+' "$tf" 2>/dev/null | grep -oE '[0-9]+$' || echo 0)
    echo "$rss" > "$SCRATCH/rss.$$"
    rm -f "$out" "$tf"
    echo "$elapsed"
}

# 3 warmup discarded (v1 Fix 1)
for _ in 1 2 3; do measure_one >/dev/null || true; done
: > "$RESULTS/$tag.$mode.txt"
: > "$RESULTS/$tag.$mode.rss.txt"
for ((i=0; i<n; i++)); do
    ms=$(measure_one) || { echo "$tag $mode FAIL"; continue; }
    echo "$tag $mode $ms"
    echo "$ms" >> "$RESULTS/$tag.$mode.txt"
    cat "$SCRATCH/rss.$$" >> "$RESULTS/$tag.$mode.rss.txt" 2>/dev/null || true
done
