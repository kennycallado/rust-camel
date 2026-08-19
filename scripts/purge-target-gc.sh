#!/usr/bin/env bash
# purge-target-gc.sh — GC cargo build artifacts that cargo never cleans.
#
# Cargo leaves garbage in target/ dirs: *.dwp companion files (dev profile
# split-debuginfo = "packed"; ~100-200 MiB per test/example binary) and
# orphaned hash-suffixed test/example binaries. Neither is ever GC'd.
#
# Usage: purge-target-gc.sh [options] [root ...]
#   root   dirs to sweep for target dirs (default: CWD)
#   -p PAT discover target dirs matching PAT (default: */target)
#   -d N   delete files older than N days (default: 7; env PURGE_GC_DAYS)
#   -o     also delete orphaned hash-suffixed binaries (default: off)
#   -n     dry run: print what would be deleted, delete nothing
#   -h     show this help
#
# Safety: refuses to run on /; deletes only *.dwp and (with -o) orphaned
# binaries inside dirs matching */target. Orphan = hash-suffixed executable
# in deps/ not referenced by any .fingerprint dep file.
set -euo pipefail
export LC_ALL=C

DAYS="${PURGE_GC_DAYS:-7}"; PATTERN='*/target'; DRY_RUN=0; ORPHANS=0

usage() { awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print } NR>1 && !/^#/ { exit }' "$0"; exit "${1:-0}"; }

while getopts "p:d:onh" opt; do
  case "$opt" in
    p) PATTERN="$OPTARG" ;; d) DAYS="$OPTARG" ;; o) ORPHANS=1 ;;
    n) DRY_RUN=1 ;; h) usage ;; *) usage 1 ;;
  esac
done
shift $((OPTIND - 1))

ROOTS=("$@"); [ "${#ROOTS[@]}" -eq 0 ] && ROOTS=(.)
for r in "${ROOTS[@]}"; do
  [ -e "$r" ] || { echo "WARN: skipping missing root: $r" >&2; continue; }
  [ "$(realpath -m "$r")" = "/" ] && { echo "ERROR: refusing to run on /" >&2; exit 1; }
done

total_bytes=0; total_files=0; seen=()

for root in "${ROOTS[@]}"; do
  [ -e "$root" ] || continue
  mapfile -t targets < <(find "$root" -type d -path "$PATTERN" -prune 2>/dev/null)
  case "$root" in $PATTERN) targets+=("$root") ;; esac

  for t in "${targets[@]}"; do
    [[ " ${seen[*]} " == *" $t "* ]] && continue
    seen+=("$t")
    t_bytes=0; t_files=0; t_dwp=0; t_orph=0

    while IFS= read -r -d '' f; do
      t_bytes=$((t_bytes + $(stat -c %s "$f"))); t_files=$((t_files + 1)); t_dwp=$((t_dwp + 1))
      if [ "$DRY_RUN" -eq 1 ]; then echo "  would delete: $f"; else rm -f "$f"; fi
    done < <(find "$t" -type f -name '*.dwp' -mtime +"$DAYS" -print0 2>/dev/null)

    if [ "$ORPHANS" -eq 1 ]; then
      for f in "$t"/*/deps/*; do
        [ -f "$f" ] && [ -x "$f" ] || continue
        base="$(basename "$f")"
        [[ "$base" =~ -[0-9a-f]{16}$ ]] || continue
        grep -q "$base" "$t"/*/.fingerprint/dep-* 2>/dev/null && continue
        [ -n "$(find "$f" -mtime +"$DAYS" -print -quit)" ] || continue
        t_bytes=$((t_bytes + $(stat -c %s "$f"))); t_files=$((t_files + 1)); t_orph=$((t_orph + 1))
        if [ "$DRY_RUN" -eq 1 ]; then echo "  would delete: $f"; else rm -f "$f"; fi
      done
    fi

    if [ "$t_files" -gt 0 ]; then
      printf '%-58s %4d files %8.1f MiB (dwp=%d orphan=%d)\n' \
        "$t" "$t_files" "$(awk -v b="$t_bytes" 'BEGIN { printf "%.1f", b / 1048576 }')" "$t_dwp" "$t_orph"
      total_bytes=$((total_bytes + t_bytes)); total_files=$((total_files + t_files))
    fi
  done
done

if [ "$total_files" -eq 0 ]; then
  echo "Nothing to purge (no *.dwp older than ${DAYS}d${ORPHANS:+ / orphans} under '$PATTERN')."
else
  echo "TOTAL: $total_files files, $(awk -v b="$total_bytes" 'BEGIN { printf "%.1f", b / 1048576 }') MiB ${DRY_RUN:+would be }freed."
fi