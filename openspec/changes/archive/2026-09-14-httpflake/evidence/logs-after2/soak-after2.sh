#!/usr/bin/env bash
# httpflake Task 1.4 (RE-RUN) — loaded-soak AFTER the completed fix.
# Fix in tree (uncommitted): rogue reset caller registry_rejects_tls_on_plain_port
# now holds REGISTRY_TEST_MUTEX; helper + hammer recover from mutex poison.
# Burners are always killed on EXIT/INT/TERM.
set -u

WT=/home/shared/rust-camel-worktrees/httpflake
EV="$WT/openspec/changes/httpflake/evidence/logs-after2"
PIDFILE=/tmp/httpflake-burners-after2.pid
RESULTS="$EV/results.tsv"
BURNERS=12
DEADLINE_SECS=${SOAK_DEADLINE_SECS:-3600}   # internal wall-clock budget
START=$(date +%s)

: > "$PIDFILE"
: > "$RESULTS"
printf 'run\tkind\trc\tmatches\tseconds\n' >> "$RESULTS"

cleanup() {
  if [ -s "$PIDFILE" ]; then
    while read -r pid; do
      [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done < "$PIDFILE"
  fi
  # wait briefly for reaping
  for _ in $(seq 1 20); do
    alive=0
    while read -r pid; do
      [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && alive=1
    done < "$PIDFILE"
    [ "$alive" -eq 0 ] && break
    sleep 0.1
  done
  # hard kill any stragglers from our pid file
  while read -r pid; do
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
  done < "$PIDFILE"
}
trap cleanup EXIT INT TERM

echo "=== soak-after2 start $(date -Is) nproc=$(nproc) burners=$BURNERS deadline=${DEADLINE_SECS}s ==="

# --- start burners (detached: no inherited stdout/stderr, own session) ---
for i in $(seq "$BURNERS"); do
  ( while :; do :; done ) > /dev/null 2>&1 &
  echo $! >> "$PIDFILE"
done
sleep 2
echo "--- burners started: $(wc -l < "$PIDFILE") pids ---"
cat "$PIDFILE" | tr '\n' ' '; echo

run_case() {
  local kind="$1"; shift
  local run="$1"; shift
  local now elapsed
  now=$(date +%s); elapsed=$((now - START))
  if [ "$elapsed" -ge "$DEADLINE_SECS" ]; then
    echo "DEADLINE reached ($elapsed s) before $kind run $run — stopping"
    return 2
  fi
  local log="$EV/run-${run}-${kind}.log"
  local t0 t1 secs rc matches
  t0=$(date +%s)
  if [ "$kind" = "full" ]; then
    RUSTC_WRAPPER= cargo test -p camel-component-http --lib -- --test-threads=12 > "$log" 2>&1
  else
    # targeted high-collision + regression test under load.
    # libtest multi-filter: filters go AFTER '--' (cargo accepts one TESTNAME only)
    RUSTC_WRAPPER= cargo test -p camel-component-http --lib -- content_type_inferred registry readiness_survives --test-threads=12 > "$log" 2>&1
  fi
  rc=$?
  t1=$(date +%s); secs=$((t1 - t0))
  matches=$(grep -c 'did not become ready' "$log" 2>/dev/null || true)
  printf '%s\t%s\t%s\t%s\t%s\n' "$run" "$kind" "$rc" "$matches" "$secs" >> "$RESULTS"
  echo "RESULT $kind run=$run rc=$rc matches=$matches secs=$secs log=$log"
  # compact failure excerpt
  grep -E '^test .* FAILED|panicked at|did not become ready|test result:' "$log" | head -20
  echo "MATCHES=$matches"
  return 0
}

# --- phase 1: full suite, exactly 15 ---
for run in $(seq 1 15); do
  run_case full "$run"; rc=$?
  [ "$rc" -eq 2 ] && break
  if [ "$rc" -eq 0 ] && [ "$(tail -1 "$RESULTS" | cut -f4)" -gt 0 ] 2>/dev/null; then
    echo "PANIC CAPTURED in full run $run"; break
  fi
done

# --- phase 2: targeted high-collision, exactly 3 ---
for run in $(seq 1 3); do
  run_case targeted "$run"; rc=$?
  [ "$rc" -eq 2 ] && break
  if [ "$rc" -eq 0 ] && [ "$(tail -1 "$RESULTS" | cut -f4)" -gt 0 ] 2>/dev/null; then
    echo "PANIC CAPTURED in targeted run $run"; break
  fi
done

# --- phase 3: supplementary targeted, exactly 10 ---
for run in $(seq 1 10); do
  run_case supplementary "$run"; rc=$?
  [ "$rc" -eq 2 ] && break
  if [ "$rc" -eq 0 ] && [ "$(tail -1 "$RESULTS" | cut -f4)" -gt 0 ] 2>/dev/null; then
    echo "PANIC CAPTURED in supplementary run $run"; break
  fi
done

echo "=== soak-after2 done $(date -Is) total_secs=$(( $(date +%s) - START )) ==="
# cleanup runs via EXIT trap
exit 0