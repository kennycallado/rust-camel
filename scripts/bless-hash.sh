#!/usr/bin/env bash
# bless-hash.sh — cargo-lock-free artifact hash for blessing/drift checks.
#
# 'cargo run -p xtask' takes the exclusive target-dir flock even on a no-op
# run (fingerprint verification), so blessing gates serialized behind peer
# agents' cargo test/clippy builds. This wrapper execs the prebuilt xtask
# binary directly: zero cargo locks (neither build nor package-cache).
#
# Freshness is content-keyed (sha256 of xtask sources vs a stamp file),
# NOT mtime-based: the shared target dir lives on ntfs3, whose coarse
# mtimes make 'find -newer' guards unreliable.
#
# Caveat: the stamp is shared across worktrees targeting the same dir. If
# two worktrees carry divergent xtask sources they will ping-pong rebuilds
# (rare; hash-artifacts format is stable). Cargo's own fingerprints keep
# real builds correct.
set -euo pipefail

change_dir="$1"
repo_root="$(git -C "$(dirname "$change_dir")" rev-parse --show-toplevel)"
target="${CARGO_TARGET_DIR:-$repo_root/target}"
bin="$target/debug/xtask"
stamp="$target/.xtask-src.sha"

# Content hash of xtask sources — immune to NTFS mtime coarseness.
# Strip the file paths ('awk {print $1}') so the stamp is identical from
# the main root and from any worktree (paths differ, contents don't).
cur="$(find "$repo_root/scripts/xtask/src" -name '*.rs' -type f -print0 \
        | sort -z | xargs -0 sha256sum | awk '{print $1}' | sha256sum | cut -d' ' -f1)"

if [[ ! -x "$bin" || ! -f "$stamp" || "$(cat "$stamp" 2>/dev/null)" != "$cur" ]]; then
  # Cold/stale path only: pay the cargo lock ONCE per source change, not
  # per hash. flock serializes concurrent cold rebuilds by peer agents.
  # TMPDIR is scoped to the target partition: cc-rs spills its .s temp
  # files to /tmp, which lives on the small root partition.
  mkdir -p "$target"
  (
    flock 9
    mkdir -p "$target/tmp"
    TMPDIR="$target/tmp" CARGO_INCREMENTAL=0 cargo build -p xtask \
      --manifest-path "$repo_root/scripts/xtask/Cargo.toml" >&2
    printf '%s' "$cur" > "$stamp"
  ) 9>"$target/.xtask-build.lock"
fi

exec "$bin" hash-artifacts --change-dir "$change_dir"
