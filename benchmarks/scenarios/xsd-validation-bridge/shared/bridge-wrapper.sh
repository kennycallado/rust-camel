#!/usr/bin/env bash
# T4a/T4b bridge binary wrapper: writes the bridge subprocess PID to a
# tmpfs file (read by the harness at round boundaries per spec §4.11)
# then exec's the real xml-bridge binary.
#
# Contract (spec §4.11 + Task 2 harness concern #5):
#   - The harness reads `/tmp/v3-bridge-pid-<cell>.txt` at round START
#     and round END; if the PID differs, the round is invalidated.
#   - Per-message /proc reads on the hot path are PROHIBITED (they
#     perturb the p99 being measured). The PID is captured ONCE at
#     bridge spawn — not per-tick.
#   - The harness tolerates the file's absence (treats as
#     no-PID-tracking, doesn't fail) — but producing the file is the
#     explicit Task 4 contract.
#
# Why a wrapper (not a production change to bridges/xml):
#   camel-bridge spawns the binary at `CAMEL_XML_BRIDGE_BINARY_PATH`
#   (Spike 1B confirmed this env var is the canonical pointer to the
#   xml-bridge). Pointing it at a wrapper lets us inject the PID write
#   without rebuilding the Java native binary OR modifying
#   crates/services/camel-bridge — both of which the brief flags as
#   "production change required → BLOCK + escalate".
#
# PID semantics after exec:
#   bash reads `$$` (its own PID), writes it, then `exec`s the real
#   bridge binary. exec preserves the PID: the bridge process inherits
#   the wrapper's PID, so the recorded PID IS the bridge's PID for its
#   whole lifetime.
#
# Invocation:
#   The fixture sets before spawning camel-bridge (which spawns this
#   wrapper):
#     CAMEL_XML_BRIDGE_BINARY_PATH=/abs/path/to/bridge-wrapper.sh
#     CAMEL_XML_BRIDGE_REAL_BINARY=/abs/path/to/bridges/xml/build/native/xml-bridge
#     V3_BRIDGE_PID_FILE=/tmp/v3-bridge-pid-<cell>.txt
#
#   If V3_BRIDGE_PID_FILE is unset, the wrapper still exec's the bridge
#   (PID file is best-effort; the harness tolerates absence).

set -euo pipefail

real_binary="${CAMEL_XML_BRIDGE_REAL_BINARY:-}"
pid_file="${V3_BRIDGE_PID_FILE:-}"

if [[ -z "$real_binary" ]]; then
    echo "bridge-wrapper: CAMEL_XML_BRIDGE_REAL_BINARY is unset; cannot exec bridge" >&2
    exit 2
fi

if [[ ! -x "$real_binary" ]]; then
    echo "bridge-wrapper: real binary not executable: $real_binary" >&2
    exit 2
fi

# Write our PID (= the bridge's PID after exec) once at spawn. Atomic
# write via temp-and-rename so the harness never reads a partial value.
if [[ -n "$pid_file" ]]; then
    tmp_file="${pid_file}.tmp.$$"
    printf '%d\n' "$$" > "$tmp_file"
    mv "$tmp_file" "$pid_file"
fi

# Hand off to the real binary; PID is preserved across exec.
exec "$real_binary" "$@"
