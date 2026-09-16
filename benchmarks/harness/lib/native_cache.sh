# Native-image build cache (bd rc-wdy13) — sourced by run.sh.
# Single source of truth for the native-artifact skip decision and
# runner resolution: skip decision + runner resolution live in
# lib/native_cache.sh; the fingerprint is written ONLY after a
# successful build + runner resolution. A failed build never
# advances the cache, so a stale fingerprint can no longer combine
# with a leftover runner image to silently skip a rebuild.
#
# Env contract (set by run.sh before any call): JAVA_HOME,
# BENCH_NATIVE_MODE, QUARKUS_NATIVE_BUILDER_IMAGE, and — local mode
# only — NATIVE_IMAGE_BIN.

# Print the first match of <runner_glob> (empty output when the glob
# has no match). The ONLY runner resolver — the build skip decision,
# the post-build presence check, and the cell-wiring resolution all
# go through this function (rc-wdy13).
resolve_native_runner() {
    local runner_glob="$1"
    # nullglob so an unmatched glob expands to nothing, not the
    # literal pattern; restore the previous state afterwards so the
    # rest of the script is unaffected.
    local prev_nullglob
    prev_nullglob="$(shopt -p nullglob 2>/dev/null || true)"
    shopt -s nullglob
    local matches=( $runner_glob )
    eval "$prev_nullglob" 2>/dev/null || shopt -u nullglob
    if [[ ${#matches[@]} -gt 0 ]]; then
        printf '%s' "${matches[0]}"
    fi
    return 0
}

# Compute the native-artifact fingerprint, compare it with the stored
# one, and resolve the runner (rc-wdy13). NEVER writes a file —
# fail-closed: a missing or stale stored fingerprint means "build",
# never an error. Sets NATIVE_CACHE_FINGERPRINT (digest) and
# NATIVE_CACHE_RUNNER (resolved path or empty). Returns 0 ONLY when
# the fingerprint matches AND the runner resolves (SKIP); returns 1
# when a build is needed.
native_cache_plan() {
    local native_dir="$1" shared_src_main="$2" sibling_gradle="$3" \
          settings_gradle="$4" gradle_dir="$5" app_props="$6" \
          runner_glob="$7"
    local fp_file="$native_dir/.bench-fingerprint"

    # Manifest of (path, content) for every input that could change
    # the native binary, in a deterministic order; hashed with
    # sha256sum. Sources are sorted so the fingerprint is stable
    # across runs.
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

    NATIVE_CACHE_FINGERPRINT="$(printf '%s' "$manifest" | sha256sum | awk '{print $1}')"
    NATIVE_CACHE_RUNNER="$(resolve_native_runner "$runner_glob")"

    if [[ -f "$fp_file" ]] \
        && [[ "$(cat "$fp_file")" == "$NATIVE_CACHE_FINGERPRINT" ]] \
        && [[ -n "$NATIVE_CACHE_RUNNER" ]]; then
        return 0   # SKIP: inputs unchanged and runner resolvable
    fi
    return 1       # BUILD needed (fail-closed on any mismatch)
}

# Write the digest computed by native_cache_plan to
# <native_dir>/.bench-fingerprint. The ONLY fingerprint write path
# in the system (rc-wdy13) — run.sh calls it exclusively after a
# successful build + runner resolution.
write_native_fingerprint() {
    local native_dir="$1"
    printf '%s' "$NATIVE_CACHE_FINGERPRINT" > "$native_dir/.bench-fingerprint"
}
