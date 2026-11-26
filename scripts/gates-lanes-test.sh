#!/usr/bin/env bash
# shellcheck shell=bash
#
# gates-lanes-test.sh — hermetic regression suite for the COMPONENT LANES phase
# of scripts/gates-local.sh (AGENTS.md `## COMPONENT LANES`), bd rc-nmgf3
# mission 594.
#
# Why this exists: the lane phase is fail-closed logic (merge base, cargo
# metadata mapping, lane table parsing, per-crate skip, docker opt-in). Every
# one of those branches is a silently-wrong sweep when it regresses, and the
# only way to exercise them is to move a real tree. This suite builds throwaway
# trees instead:
#
# - Each case builds a tiny git repo under a scratch dir resolved the same way
#   gates-local.sh resolves its own: $GATES_LANES_TEST_TMPDIR when set, else
#   $TMPDIR when set, else /home/shared/tmp when it exists, else mktemp's
#   default. GitHub runners have neither TMPDIR nor /home/shared, so a hard
#   /home/shared/tmp default would break the ci.yml `lint-gates-lanes` step.
#   The repo always gets branch `main` (base commit) plus a `work` branch
#   carrying the case's touched files.
# - A stub `cargo` is first on PATH: it answers `metadata` with a canned JSON
#   in the real `cargo metadata --format-version 1 --no-deps` shape
#   (`workspace_root`, `packages[].name`, `packages[].manifest_path`,
#   `packages[].targets[].kind`) and appends every other argv to a log, so a
#   case asserts the exact commands executed without compiling anything.
# - The production script runs unmodified from the fixture repo with
#   GATES_ONLY=lanes, so the harness cannot drift from the implementation: it
#   asserts the strings scripts/gates-local.sh actually prints.
#
# Usage: bash scripts/gates-lanes-test.sh
#        GATES_LANES_TEST_TMPDIR=/path bash scripts/gates-lanes-test.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATES_LOCAL="$SCRIPT_DIR/gates-local.sh"

[[ -f "$GATES_LOCAL" ]] || {
    echo "gates-lanes-test: missing $GATES_LOCAL" >&2
    exit 2
}
command -v git >/dev/null 2>&1 || {
    echo "gates-lanes-test: git must be on PATH" >&2
    exit 2
}
command -v python3 >/dev/null 2>&1 || {
    echo "gates-lanes-test: python3 must be on PATH (gates-local.sh needs it)" >&2
    exit 2
}

TEST_TMP=""
if [[ -n "${GATES_LANES_TEST_TMPDIR:-}" ]]; then
    TEST_TMP="$GATES_LANES_TEST_TMPDIR"
elif [[ -n "${TMPDIR:-}" ]]; then
    TEST_TMP="$TMPDIR"
elif [[ -d /home/shared/tmp ]]; then
    TEST_TMP="/home/shared/tmp"
fi
if [[ -n "$TEST_TMP" ]]; then
    mkdir -p "$TEST_TMP" 2>/dev/null || {
        echo "gates-lanes-test: cannot create scratch dir $TEST_TMP" >&2
        exit 2
    }
    ROOT="$(mktemp -d "${TEST_TMP%/}/gates-lanes-test.XXXXXX" 2>/dev/null)" || {
        echo "gates-lanes-test: cannot allocate a scratch dir under $TEST_TMP" >&2
        exit 2
    }
else
    ROOT="$(mktemp -d 2>/dev/null)" || {
        echo "gates-lanes-test: cannot allocate a scratch dir with mktemp" >&2
        exit 2
    }
fi
trap 'rm -rf "$ROOT"' EXIT
# Fixtures that need a filesystem allowing tabs/newlines in names are allocated
# from mktemp's own default (some shared mounts reject such names with EINVAL)
# and registered here so the trap still cleans them.
EXTRA_ROOTS=()
cleanup_roots() {
    local extra
    for extra in "${EXTRA_ROOTS[@]+"${EXTRA_ROOTS[@]}"}"; do
        [[ -n "$extra" ]] && rm -rf "$extra"
    done
    rm -rf "$ROOT"
}
trap cleanup_roots EXIT

STUB_BIN="$ROOT/stub-bin"

install_stub_cargo() {
    mkdir -p "$STUB_BIN" || return 1
    cat >"$STUB_BIN/cargo" <<'SH'
#!/usr/bin/env bash
# Stub cargo for gates-lanes-test.sh: canned `metadata` JSON, argv log for
# everything else. Nothing is compiled.
set -uo pipefail
if [[ "${1:-}" == "metadata" ]]; then
    if [[ "${STUB_METADATA_FAIL:-0}" == "1" ]]; then
        echo "stub cargo: metadata exploded on purpose" >&2
        exit 101
    fi
    cat "${STUB_META:?stub cargo: STUB_META is unset}"
    exit 0
fi
printf '%s\n' "$*" >>"${STUB_ARGV_LOG:?stub cargo: STUB_ARGV_LOG is unset}"
if [[ -n "${STUB_TEST_FAIL_AT:-}" ]] && [[ "$(wc -l <"$STUB_ARGV_LOG")" -eq "$STUB_TEST_FAIL_AT" ]]; then exit 1; fi
exit 0
SH
    chmod +x "$STUB_BIN/cargo"
}

# ── per-case state ───────────────────────────────────────────────────────────
BASE=""
RUNS=""
REPO=""
META=""
EXTRA_BIN=""
RUN=0
OUT=""
ERR=""
ARGV=""
SWEEP_CODE=0
FAIL_DETAIL=""

REAL_GIT="$(command -v git)"

# ── fixture helpers ─────────────────────────────────────────────────────────
# fixture_new <tag> [base-dir] — reset per-case state under <base-dir>/<tag>
# (default: the suite ROOT). The optional base lets one case pick a different
# filesystem: /home/shared/tmp rejects tabs/newlines in file names (EINVAL).
fixture_new() {
    local tag="${1:?fixture tag}" base="${2:-$ROOT}"
    BASE="$base/$tag"
    RUNS="$BASE/runs"
    REPO="$BASE/repo"
    META="$BASE/meta.json"
    RUN=0
    OUT=""
    ERR=""
    ARGV=""
    EXTRA_BIN=""
    SWEEP_CODE=0
    FAIL_DETAIL=""
    mkdir -p "$RUNS" "$REPO" || return 1
}

# install_failing_git <subcommand> — prepend a `git` shim that exits 128 for
# that subcommand and delegates everything else to the real git. Used to pin the
# fail-closed paths where one git probe fails (diff, ls-files) while merge-base
# still succeeds.
install_failing_git() {
    local sub="${1:?git subcommand}"
    EXTRA_BIN="$BASE/stub-git"
    mkdir -p "$EXTRA_BIN" || return 1
    {
        printf '#!/usr/bin/env bash\n'
        printf '# Stub git: only the %s subcommand fails; everything else is real git.\n' "$sub"
        printf 'set -uo pipefail\n'
        printf 'for arg in "$@"; do\n'
        printf '    if [[ "$arg" == %q ]]; then\n' "$sub"
        printf '        echo "stub git: %s exploded on purpose" >&2\n' "$sub"
        printf '        exit 128\n'
        printf '    fi\n'
        printf 'done\n'
        printf 'exec %q "$@"\n' "$REAL_GIT"
    } >"$EXTRA_BIN/git"
    chmod +x "$EXTRA_BIN/git"
}

# fixture_agents_preamble — the "# fixture" header plus the QUALITY GATES block
# only. Raw fixtures (NUL bytes, bogus fences, headings inside the fence) append
# their own `## COMPONENT LANES` section, which fixture_agents cannot express.
fixture_agents_preamble() {
    cat >"$REPO/AGENTS.md" <<'EOF'
# gates-lanes-test fixture

## QUALITY GATES

```
- name: noop
  run: 'true'
```
EOF
}

# fixture_meta_raw <json> — write canned metadata verbatim, for malformed shapes.
fixture_meta_raw() {
    printf '%s\n' "${1:?json}" >"$META"
}

# nul_line_of <needle-line-number> — 1-based line number of the first line in
# $REPO/AGENTS.md that carries a NUL byte (empty when there is none).
nul_line_of_agents() {
    python3 - "$REPO/AGENTS.md" <<'PY'
import sys

try:
    data = open(sys.argv[1], "rb").read()
except OSError:
    sys.exit(0)
for i, line in enumerate(data.split(b"\n"), 1):
    if b"\x00" in line:
        print(i)
        break
PY
}

# fixture_init_repo [base-branch] — fresh repo on `main` (or the given branch).
fixture_init_repo() {
    local branch="${1:-main}"
    git -C "$REPO" init -q -b "$branch" || return 1
    git -C "$REPO" config user.email gates-lanes-test@example.invalid || return 1
    git -C "$REPO" config user.name "gates-lanes-test" || return 1
    git -C "$REPO" config commit.gpgsign false || return 1
    git -C "$REPO" config core.hooksPath /dev/null || return 1
    git -C "$REPO" config gc.auto 0 || return 1
}

# fixture_agents <lane rows...> — minimal AGENTS.md with the two fences the
# production script parses.
fixture_agents() {
    {
        cat <<'EOF'
# gates-lanes-test fixture

## QUALITY GATES

```
- name: noop
  run: 'true'
```

## COMPONENT LANES

```
EOF
        local row
        for row in "$@"; do
            printf '%s\n' "$row"
        done
        printf '```\n'
    } >"$REPO/AGENTS.md"
}

# crate_seed <rel-dir> <package-name> — a lib crate with no integration surface.
crate_seed() {
    local rel="$1" name="$2"
    mkdir -p "$REPO/$rel/src" || return 1
    printf '[package]\nname = "%s"\nversion = "0.0.0"\nedition = "2021"\n' "$name" \
        >"$REPO/$rel/Cargo.toml" || return 1
    printf 'pub fn stub() {}\n' >"$REPO/$rel/src/lib.rs" || return 1
}

fixture_base_commit() {
    git -C "$REPO" add -A || return 1
    git -C "$REPO" commit -q -m base || return 1
    git -C "$REPO" checkout -q -b work || return 1
}

commit_work() {
    git -C "$REPO" add -A || return 1
    git -C "$REPO" commit -q -m "${1:-work}" || return 1
}

# fixture_meta <name:rel-dir:kinds>... — canned cargo metadata for the fixture.
fixture_meta() {
    python3 - "$META" "$REPO" "$@" <<'PY'
import json
import os
import sys

out, root = sys.argv[1], sys.argv[2]
packages = []
for spec in sys.argv[3:]:
    name, rel, kinds = spec.split(":")
    packages.append(
        {
            "name": name,
            "manifest_path": os.path.join(root, rel, "Cargo.toml"),
            "targets": [
                {"kind": [kind], "name": "%s_%s" % (name.replace("-", "_"), kind)}
                for kind in kinds.split(",")
                if kind
            ],
        }
    )
with open(out, "w", encoding="utf-8") as handle:
    json.dump({"workspace_root": root, "packages": packages}, handle)
PY
}

# run_sweep [GATES_* assignments...] — run the production script with cwd = the
# fixture repo, capturing stdout/stderr/argv per run.
run_sweep() {
    RUN=$((RUN + 1))
    OUT="$RUNS/out-$RUN.txt"
    ERR="$RUNS/err-$RUN.txt"
    ARGV="$RUNS/argv-$RUN.log"
    (
        cd "$REPO" || exit 90
        env -u GATES_ONLY -u GATES_SKIP -u GATES_LANES \
            PATH="${EXTRA_BIN:+$EXTRA_BIN:}$STUB_BIN:$PATH" \
            STUB_ARGV_LOG="$ARGV" \
            STUB_META="$META" \
            "$@" bash "$GATES_LOCAL"
    ) >"$OUT" 2>"$ERR"
    SWEEP_CODE=$?
}

# ── assertions (set FAIL_DETAIL, return 1 on failure) ────────────────────────
assert_rc() {
    if [[ "$SWEEP_CODE" -ne "$1" ]]; then
        FAIL_DETAIL="sweep exit $SWEEP_CODE, expected $1"
        return 1
    fi
}

assert_rc_nonzero() {
    if [[ "$SWEEP_CODE" -eq 0 ]]; then
        FAIL_DETAIL="sweep exited 0, expected non-zero"
        return 1
    fi
}

assert_out_has() {
    if grep -Fq -- "$1" "$OUT"; then
        return 0
    fi
    FAIL_DETAIL="stdout missing: $1"
    return 1
}

assert_out_has_re() {
    if grep -Eq -- "$1" "$OUT"; then
        return 0
    fi
    FAIL_DETAIL="stdout does not match: $1"
    return 1
}

assert_out_lacks() {
    if grep -Fq -- "$1" "$OUT"; then
        FAIL_DETAIL="stdout unexpectedly contains: $1"
        return 1
    fi
}

assert_err_has() {
    if grep -Fq -- "$1" "$ERR"; then
        return 0
    fi
    FAIL_DETAIL="stderr missing: $1"
    return 1
}

assert_err_has_re() {
    if grep -Eq -- "$1" "$ERR"; then
        return 0
    fi
    FAIL_DETAIL="stderr does not match: $1"
    return 1
}

assert_err_lacks() {
    if grep -Fq -- "$1" "$ERR"; then
        FAIL_DETAIL="stderr unexpectedly contains: $1"
        return 1
    fi
}

# assert_err_names_row — the fail-closed message must identify the file AND the
# offending line/row number, never a bare "parse error".
assert_err_names_row() {
    assert_err_has "$1" || return 1
    assert_err_has_re '(line|row) +[0-9]+' || return 1
}

assert_argv_exact() {
    local want="$1" got
    if [[ ! -s "$ARGV" ]]; then
        FAIL_DETAIL="argv log empty/missing — no command executed (expected [$want])"
        return 1
    fi
    got="$(cat "$ARGV")"
    if [[ "$got" != "$want" ]]; then
        FAIL_DETAIL="argv log: got [$got], want [$want]"
        return 1
    fi
}

assert_argv_line() {
    if [[ ! -s "$ARGV" ]]; then
        FAIL_DETAIL="argv log empty/missing (expected line [$1])"
        return 1
    fi
    if grep -Fxq -- "$1" "$ARGV"; then
        return 0
    fi
    FAIL_DETAIL="argv log missing line [$1]; got: [$(tr '\n' '|' <"$ARGV")]"
    return 1
}

assert_argv_empty() {
    if [[ -s "$ARGV" ]]; then
        FAIL_DETAIL="expected no command execution; argv log: [$(tr '\n' '|' <"$ARGV")]"
        return 1
    fi
}

# assert_out_order <earlier> <later> — the first stdout line matching <earlier>
# must precede the first line matching <later> (fixed-string match).
assert_out_order() {
    local first second
    first="$(grep -Fn -- "$1" "$OUT" | head -n1 | cut -d: -f1)"
    second="$(grep -Fn -- "$2" "$OUT" | head -n1 | cut -d: -f1)"
    if [[ -z "$first" ]]; then
        FAIL_DETAIL="stdout missing (ordering): $1"
        return 1
    fi
    if [[ -z "$second" ]]; then
        FAIL_DETAIL="stdout missing (ordering): $2"
        return 1
    fi
    if [[ "$first" -ge "$second" ]]; then
        FAIL_DETAIL="ordering: [$1] at line $first is not before [$2] at line $second"
        return 1
    fi
}

# assert_out_count <fixed-string> <n> — exactly <n> stdout lines contain it.
assert_out_count() {
    local got
    got="$(grep -Fc -- "$1" "$OUT")"
    if [[ "$got" -ne "$2" ]]; then
        FAIL_DETAIL="stdout has $got lines matching [$1], expected $2"
        return 1
    fi
}

# assert_out_lacks_re <extended-regex> — no stdout line matches it.
assert_out_lacks_re() {
    if grep -Eq -- "$1" "$OUT"; then
        FAIL_DETAIL="stdout unexpectedly matches: $1"
        return 1
    fi
}

# assert_out_line <fixed-string> — an exact stdout LINE equals it.
assert_out_line() {
    if grep -Fxq -- "$1" "$OUT"; then
        return 0
    fi
    FAIL_DETAIL="stdout has no exact line [$1]"
    return 1
}

# assert_file_bytes <got> <want> — byte-identical files (cmp), for trace proofs.
assert_file_bytes() {
    local got="$1" want="$2" delta
    if [[ ! -f "$got" ]]; then
        FAIL_DETAIL="missing trace file: $got"
        return 1
    fi
    if delta="$(cmp "$want" "$got" 2>&1)"; then
        return 0
    fi
    FAIL_DETAIL="trace bytes differ: $delta (want $want, got $got)"
    return 1
}

# ── cases ───────────────────────────────────────────────────────────────────
# 1. touched crate with lane + tests/ -> the lane command executes verbatim.
case_01() {
    fixture_new 01-touched-with-lane || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | cargo test -p camel-component-llm --features decisions' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    mkdir -p "$REPO/crates/llm/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/llm/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib,test' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-component-llm' || return 1
    assert_argv_exact 'test -p camel-component-llm --features decisions'
}

# 2. a failing lane command propagates: exit non-zero + FAIL + failed count.
case_02() {
    fixture_new 02-lane-failure || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-fail | local | false' || return 1
    crate_seed crates/fail camel-fail || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/fail/src/lib.rs" || return 1
    commit_work "touch fail" || return 1
    fixture_meta 'camel-fail:crates/fail:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has 'FAIL lane camel-fail' || return 1
    assert_out_has_re '^lanes failed: +1( |$)'
}

# 3. missing lane on a crate with tests/**/*.rs blocks the sweep.
case_03() {
    fixture_new 03-no-lane-tests-dir || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/test-crate camel-test || return 1
    mkdir -p "$REPO/crates/test-crate/tests" || return 1
    printf '#[test]\nfn discovered() {}\n' >"$REPO/crates/test-crate/tests/discovered.rs" || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test-crate/src/lib.rs" || return 1
    commit_work "touch test-crate" || return 1
    fixture_meta 'camel-test:crates/test-crate:lib,test' 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has '!! NO LANE for camel-test' || return 1
    assert_out_has 'blocking: crate has integration surface'
}

# 4. missing lane on a crate whose only integration surface is an explicit
#    [[test]] target outside tests/ still blocks (metadata kind = test).
case_04() {
    fixture_new 04-no-lane-explicit-test-target || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/explicit camel-x || return 1
    printf '\n[[test]]\nname = "explicit_suite"\npath = "src/it.rs"\n' \
        >>"$REPO/crates/explicit/Cargo.toml" || return 1
    printf '#[test]\nfn explicit() {}\n' >"$REPO/crates/explicit/src/it.rs" || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/explicit/src/lib.rs" || return 1
    commit_work "touch explicit" || return 1
    fixture_meta 'camel-x:crates/explicit:lib,test' 'camel-component-llm:crates/llm:lib' || return 1
    if [[ -d "$REPO/crates/explicit/tests" ]]; then
        FAIL_DETAIL="fixture is wrong: crates/explicit/tests exists"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has '!! NO LANE for camel-x' || return 1
    assert_out_has 'blocking: crate has integration surface'
}

# 5. missing lane on a crate whose tests/ file is feature-gated still blocks.
case_05() {
    fixture_new 05-no-lane-feature-gated || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/gated camel-gated || return 1
    mkdir -p "$REPO/crates/gated/tests" || return 1
    printf '#![cfg(feature = "x")]\n\n#[test]\nfn gated() {}\n' \
        >"$REPO/crates/gated/tests/gated.rs" || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/gated/src/lib.rs" || return 1
    commit_work "touch gated" || return 1
    fixture_meta 'camel-gated:crates/gated:lib,test' 'camel-component-llm:crates/llm:lib' || return 1
    grep -Fq '#![cfg(feature = "x")]' "$REPO/crates/gated/tests/gated.rs" || {
        FAIL_DETAIL="fixture is wrong: gated file lost its cfg gate"
        return 1
    }
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has '!! NO LANE for camel-gated' || return 1
    assert_out_has 'blocking: crate has integration surface'
}

# 6. missing lane on a crate with no integration surface is loud but not
#    blocking (exit 0).
case_06() {
    fixture_new 06-no-lane-no-surface || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/plain camel-dsl || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/plain/src/lib.rs" || return 1
    commit_work "touch plain" || return 1
    fixture_meta 'camel-dsl:crates/plain:lib' 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has '!! NO LANE for camel-dsl' || return 1
    assert_out_has 'not blocking: no integration surface'
}

# 7. tooling-only tree (scripts/, AGENTS.md): no lanes, no no-lane, exit 0.
case_07() {
    fixture_new 07-tooling-only || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | cargo test -p camel-component-llm' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    mkdir -p "$REPO/scripts" || return 1
    printf '#!/usr/bin/env bash\ntrue\n' >"$REPO/scripts/x" || return 1
    chmod +x "$REPO/scripts/x" || return 1
    printf '\n<!-- touched -->\n' >>"$REPO/AGENTS.md" || return 1
    commit_work "tooling only" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib,test' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has_re '^lanes passed: +0$' || return 1
    assert_out_has_re '^no-lane: +0$' || return 1
    assert_argv_empty
}

# 8. GATES_LANES=0 disables the phase; GATES_LANES=all opts docker rows in.
case_08() {
    fixture_new 08a-lanes-disabled || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | cargo test -p camel-component-llm' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes GATES_LANES=0
    assert_rc 0 || return 1
    assert_out_has 'component lanes: DISABLED (GATES_LANES=0)' || return 1
    assert_argv_empty || return 1

    fixture_new 08b-docker-opt-in || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-test | docker | cargo test -p camel-test --features integration-tests --test kafka_test' || return 1
    crate_seed crates/test camel-test || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes GATES_LANES=all
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-test --features integration-tests --test kafka_test'
}

# 9. selectors: GATES_ONLY non-selecting, GATES_ONLY=lanes, GATES_SKIP=lanes,
#    per-crate GATES_SKIP (row present and row absent).
case_09() {
    fixture_new 09-selectors || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | cargo test -p camel-component-llm' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1

    run_sweep GATES_ONLY="fmt check"
    assert_rc 0 || return 1
    assert_out_has 'component lanes: not selected (GATES_ONLY)' || return 1
    assert_argv_empty || return 1

    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-component-llm' || return 1
    assert_argv_exact 'test -p camel-component-llm' || return 1

    run_sweep GATES_ONLY=lanes GATES_SKIP=lanes
    assert_rc 0 || return 1
    assert_out_has 'component lanes: DISABLED (GATES_SKIP=lanes)' || return 1
    assert_argv_empty || return 1

    run_sweep GATES_ONLY=lanes GATES_SKIP=camel-component-llm
    assert_rc 0 || return 1
    assert_out_has 'component lanes: SKIP camel-component-llm (GATES_SKIP)' || return 1
    assert_argv_empty || return 1

    # Per-crate skip also silences the NO-LANE fail-safe for a row-less crate.
    fixture_new 09b-skip-no-lane || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/test camel-test || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes GATES_SKIP=camel-test
    assert_rc 0 || return 1
    assert_out_has 'component lanes: SKIP camel-test (GATES_SKIP)' || return 1
    assert_out_has_re '^no-lane: +0$' || return 1
    assert_argv_empty
}

# 10. cross-crate rename committed on the work branch surfaces both crates.
case_10() {
    fixture_new 10-cross-crate-rename || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-a | local | cargo test -p camel-a' \
        'camel-b | local | cargo test -p camel-b' || return 1
    crate_seed crates/a camel-a || return 1
    crate_seed crates/b camel-b || return 1
    printf 'pub fn moved() {}\n' >"$REPO/crates/a/src/moved.rs" || return 1
    fixture_base_commit || return 1
    git -C "$REPO" mv crates/a/src/moved.rs crates/b/src/moved.rs || return 1
    commit_work "move a -> b" || return 1
    fixture_meta 'camel-a:crates/a:lib' 'camel-b:crates/b:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_line 'test -p camel-a' || return 1
    assert_argv_line 'test -p camel-b'
}

# 11. an uncommitted modification and an untracked file both count as touches.
case_11() {
    fixture_new 11-dirty-and-untracked || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-a | local | cargo test -p camel-a' \
        'camel-b | local | cargo test -p camel-b' || return 1
    crate_seed crates/a camel-a || return 1
    crate_seed crates/b camel-b || return 1
    fixture_base_commit || return 1
    printf '// dirty, never committed\n' >>"$REPO/crates/a/src/lib.rs" || return 1
    printf 'pub fn untracked() {}\n' >"$REPO/crates/b/src/extra.rs" || return 1
    fixture_meta 'camel-a:crates/a:lib' 'camel-b:crates/b:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_line 'test -p camel-a' || return 1
    assert_argv_line 'test -p camel-b'
}

# 12. malformed lane rows are exit 2 with stderr naming the row: a row without
#     three fields, an unknown mode, and an empty command.
case_12() {
    local bad_row
    for bad_row in 'bad|row' 'camel-x | nope | true' 'camel-x | local |'; do
        fixture_new "12-malformed-$(printf '%s' "$bad_row" | tr -c 'a-z0-9' '_')" || return 1
        fixture_init_repo || return 1
        fixture_agents "$bad_row" || return 1
        fixture_base_commit || return 1
        run_sweep GATES_ONLY=lanes
        assert_rc 2 || return 1
        assert_err_has 'bad row' || return 1
        assert_err_has "$bad_row" || return 1
    done
}

# 13. fail-closed detection: cargo metadata failure and a missing merge base
#     are both exit 2 with a loud stderr message.
case_13() {
    fixture_new 13a-metadata-failure || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes STUB_METADATA_FAIL=1
    assert_rc 2 || return 1
    assert_err_has 'cargo metadata failed' || return 1

    fixture_new 13b-no-main-branch || return 1
    fixture_init_repo trunk || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'cannot compute merge base with main'
}

# 14. prose that merely CONTAINS the marker is not a section: with no
#     complete-line `## COMPONENT LANES` heading anywhere, the phase is exit 2
#     and the lane-like fence in that other section must never execute.
case_14() {
    fixture_new 14-marker-in-prose || return 1
    fixture_init_repo || return 1
    crate_seed crates/llm camel-component-llm || return 1
    mkdir -p "$REPO/crates/llm/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/llm/tests/basic.rs" || return 1
    cat >"$REPO/AGENTS.md" <<'EOF'
# gates-lanes-test fixture

## QUALITY GATES

```
- name: noop
  run: 'true'
```

## NOTES

The lane table belongs under `## COMPONENT LANES`, but this fixture has that
marker only as inline prose — never as a complete line of its own.

```
camel-component-llm | local | cargo test -p camel-component-llm
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib,test' || return 1
    # Guard the fixture: a complete-line heading must genuinely be absent.
    if grep -Fxq '## COMPONENT LANES' "$REPO/AGENTS.md"; then
        FAIL_DETAIL="fixture is wrong: AGENTS.md has a complete-line ## COMPONENT LANES heading"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'no ## COMPONENT LANES section' || return 1
    assert_argv_empty
}

# 15. a real `## COMPONENT LANES` heading whose own section carries no fence,
#     with the lane fence living under a LATER `## OTHER` heading -> exit 2
#     (fence discovery is bounded to the section) and no lane executes.
case_15() {
    fixture_new 15-fence-in-later-section || return 1
    fixture_init_repo || return 1
    crate_seed crates/llm camel-component-llm || return 1
    mkdir -p "$REPO/crates/llm/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/llm/tests/basic.rs" || return 1
    cat >"$REPO/AGENTS.md" <<'EOF'
# gates-lanes-test fixture

## QUALITY GATES

```
- name: noop
  run: 'true'
```

## COMPONENT LANES

This section intentionally has no fenced table of its own.

## OTHER

```
camel-component-llm | local | cargo test -p camel-component-llm
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib,test' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'no fenced table under ## COMPONENT LANES' || return 1
    assert_argv_empty
}

# 16. `git diff` failing (rc 128) while merge-base succeeds is fail-closed exit
#     2 — never "zero touched crates" — and the lane summary never prints.
case_16() {
    fixture_new 16-diff-failure || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | cargo test -p camel-component-llm' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1
    install_failing_git diff || return 1
    # Guard the shim: merge-base must still work while diff fails.
    if ! "$EXTRA_BIN/git" -C "$REPO" merge-base HEAD main >/dev/null 2>&1; then
        FAIL_DETAIL="fixture is wrong: shimmed git cannot compute merge-base"
        return 1
    fi
    if "$EXTRA_BIN/git" -C "$REPO" diff --name-only >/dev/null 2>&1; then
        FAIL_DETAIL="fixture is wrong: shimmed git diff succeeded"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'git diff failed — cannot enumerate changed paths' || return 1
    assert_out_lacks 'lanes passed:' || return 1
    assert_argv_empty
}

# 17. `lanes skipped` counts ROWS while its label list stays deduped, and the
#     registry summary prints BEFORE the lane phase output.
case_17() {
    fixture_new 17-docker-skip-row-units || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-test | docker | cargo test -p camel-test --features integration-tests --test kafka_test' \
        'camel-test | docker | cargo test -p camel-test --features integration-tests --test redis_test' || return 1
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn kafka() {}\n' >"$REPO/crates/test/tests/kafka_test.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has_re '^lanes skipped: +2( |$)' || return 1
    # Exactly two skip lines (row units) …
    assert_out_count 'SKIP lane camel-test ::' 2 || return 1
    # … while the label list stays deduped to one entry per skip reason.
    assert_out_count 'camel-test (docker opt-in)' 1 || return 1
    # The registry report is COMPLETE before any lane activity starts: its
    # `ran:` line precedes the first skip line, and the registry summary
    # precedes the lane-phase divider.
    assert_out_order 'ran:' 'SKIP lane camel-test ::' || return 1
    assert_out_order 'gates-local summary' '──── component lanes ────' || return 1
    assert_argv_empty
}

# 18. a lane-table parse error must not swallow the registry report: stdout
#     keeps the heading plus the complete passed/failed/skipped/ran block while
#     stderr names the file and the bad row, and the lane phase neither runs
#     nor summarizes. run_sweep keeps stdout and stderr in separate files, so
#     the report's survival is proven rather than inferred.
case_18() {
    fixture_new 18-report-survives-parse-error || return 1
    fixture_init_repo || return 1
    fixture_agents 'bad|row' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/llm/src/lib.rs" || return 1
    commit_work "touch llm" || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    # Registry report survived on stdout …
    assert_out_has 'gates-local summary' || return 1
    assert_out_has_re '^passed: +[0-9]+$' || return 1
    assert_out_has_re '^failed: +[0-9]+$' || return 1
    assert_out_has_re '^skipped: +[0-9]+' || return 1
    assert_out_has_re '^ran: +[0-9]+ of [0-9]+$' || return 1
    # … while the parse error names the file and the bad row on stderr …
    assert_err_names_row 'AGENTS.md' || return 1
    # … and the lane phase ran nothing and printed no summary.
    assert_argv_empty || return 1
    assert_out_lacks 'lanes passed:'
}

# 19. a NUL byte inside a table row is rejected fail-closed, naming the file and
#     the line, before any lane executes.
case_19() {
    fixture_new 19-nul-byte-in-row || return 1
    fixture_init_repo || return 1
    crate_seed crates/ok camel-ok || return 1
    fixture_agents_preamble || return 1
    {
        printf '\n## COMPONENT LANES\n\n```\n'
        printf 'camel-ok | local | cargo test -p camel-ok\n'
        printf 'camel-nul | local | cargo test -p camel-nul\0\n'
        printf '```\n'
    } >>"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/ok/src/lib.rs" || return 1
    commit_work "touch ok" || return 1
    fixture_meta 'camel-ok:crates/ok:lib' || return 1
    local nul_line
    nul_line="$(nul_line_of_agents)"
    if [[ -z "$nul_line" ]]; then
        FAIL_DETAIL="fixture is wrong: AGENTS.md carries no NUL byte"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'AGENTS.md' || return 1
    assert_err_has_re "(line|row) +${nul_line}([^0-9]|$)" || return 1
    assert_argv_empty
}

# 20. field injection: whitespace inside the CRATE field is rejected (file +
#     line) instead of shifting the tab-separated transport. The tab fixture is
#     built so a shifted read would execute `true` for a row whose command is
#     `false`; the guard must leave that command unexecuted.
case_20() {
    fixture_new 20a-tab-in-crate-field || return 1
    fixture_init_repo || return 1
    crate_seed crates/x camel-x || return 1
    fixture_agents_preamble || return 1
    {
        printf '\n## COMPONENT LANES\n\n```\n'
        # Real command is `false`; a tab-shifted read would run `true`.
        printf 'camel-x\tlocal\ttrue | local | false\n'
        printf '```\n'
    } >>"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_names_row 'AGENTS.md' || return 1
    assert_argv_empty || return 1
    assert_out_lacks 'PASS lane'

    fixture_new 20b-space-in-crate-field || return 1
    fixture_init_repo || return 1
    crate_seed crates/x camel-x || return 1
    fixture_agents 'camel-x extra | local | true' || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_names_row 'AGENTS.md' || return 1
    assert_argv_empty
}

# 21. a lane command carrying pipes, quotes, backslashes and a tab reaches bash
#     byte-identically: the printed lane header echoes the table text and the
#     stub argv log shows the exact token sequence the pipeline produced.
case_21() {
    local cmd want_argv
    cmd="cargo test -p camel-exotic --features 'a|b' --tag 'x\\y' --note 'n"$'\t'"1' | cat"
    want_argv="test -p camel-exotic --features a|b --tag x\\y --note n"$'\t'"1"
    fixture_new 21-exotic-command || return 1
    fixture_init_repo || return 1
    fixture_agents "camel-exotic | local | $cmd" || return 1
    crate_seed crates/exotic camel-exotic || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/exotic/src/lib.rs" || return 1
    commit_work "touch exotic" || return 1
    fixture_meta 'camel-exotic:crates/exotic:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has "=== lane camel-exotic :: $cmd" || return 1
    assert_out_has "PASS lane camel-exotic :: $cmd" || return 1
    assert_argv_exact "$want_argv"
}

# 22. structurally invalid metadata is loud exit 2 for every shape, never a
#     silent "no touched crates": empty object, `packages` not a list, package
#     without a name, package without a manifest path, `targets` not a list,
#     target kind not a list.
case_22() {
    local shape
    for shape in empty-object packages-wrong-type missing-name missing-manifest-path targets-wrong-type kind-wrong-type; do
        fixture_new "22-${shape}" || return 1
        fixture_init_repo || return 1
        fixture_agents 'camel-x | local | cargo test -p camel-x' || return 1
        crate_seed crates/x camel-x || return 1
        fixture_base_commit || return 1
        printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
        commit_work "touch x" || return 1
        case "$shape" in
        empty-object)
            fixture_meta_raw '{}' || return 1
            ;;
        packages-wrong-type)
            fixture_meta_raw "{\"workspace_root\": \"$REPO\", \"packages\": \"nope\"}" || return 1
            ;;
        missing-name)
            fixture_meta_raw "{\"workspace_root\": \"$REPO\", \"packages\": [{\"manifest_path\": \"$REPO/crates/x/Cargo.toml\", \"targets\": []}]}" || return 1
            ;;
        missing-manifest-path)
            fixture_meta_raw "{\"workspace_root\": \"$REPO\", \"packages\": [{\"name\": \"camel-x\", \"targets\": []}]}" || return 1
            ;;
        targets-wrong-type)
            fixture_meta_raw "{\"workspace_root\": \"$REPO\", \"packages\": [{\"name\": \"camel-x\", \"manifest_path\": \"$REPO/crates/x/Cargo.toml\", \"targets\": {}}]}" || return 1
            ;;
        kind-wrong-type)
            fixture_meta_raw "{\"workspace_root\": \"$REPO\", \"packages\": [{\"name\": \"camel-x\", \"manifest_path\": \"$REPO/crates/x/Cargo.toml\", \"targets\": [{\"kind\": \"lib\"}]}]}" || return 1
            ;;
        esac
        run_sweep GATES_ONLY=lanes
        assert_rc 2 || return 1
        assert_err_has 'gates-local:' || return 1
        assert_err_has 'metadata' || return 1
        assert_argv_empty || return 1
    done
}

# 23. the lane phase must not discover its own scratch subtree: with TMPDIR
#     inside an untouched crate, the scratch files are untracked paths in that
#     crate, and counting them would self-execute a lane nobody touched.
case_23() {
    fixture_new 23-scratch-self-discovery || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-touched | local | cargo test -p camel-touched' \
        'camel-untouched | local | cargo test -p camel-untouched' || return 1
    crate_seed crates/touched camel-touched || return 1
    crate_seed crates/untouched camel-untouched || return 1
    fixture_base_commit || return 1
    mkdir -p "$REPO/crates/untouched/tmp" || return 1
    printf '// touched\n' >>"$REPO/crates/touched/src/lib.rs" || return 1
    commit_work "touch touched" || return 1
    fixture_meta 'camel-touched:crates/touched:lib' 'camel-untouched:crates/untouched:lib' || return 1
    run_sweep GATES_ONLY=lanes TMPDIR="$REPO/crates/untouched/tmp"
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-touched' || return 1
    assert_out_lacks 'PASS lane camel-untouched'
}

# 24. only a bare ``` line closes the table: a shorter backtick run, a ~~~ run
#     and a non-bare ```-plus-text line each fail to close it, so the phase ends
#     fail-closed on the unterminated fence instead of running the row.
case_24() {
    local closer tag
    while IFS='|' read -r tag closer; do
        [[ -n "$tag" ]] || continue
        fixture_new "24-${tag}" || return 1
        fixture_init_repo || return 1
        crate_seed crates/x camel-x || return 1
        fixture_agents_preamble || return 1
        {
            printf '\n## COMPONENT LANES\n\n```\n'
            printf 'camel-x | local | cargo test -p camel-x\n'
            printf '%s\n' "$closer"
        } >>"$REPO/AGENTS.md" || return 1
        fixture_base_commit || return 1
        printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
        commit_work "touch x" || return 1
        fixture_meta 'camel-x:crates/x:lib' || return 1
        run_sweep GATES_ONLY=lanes
        assert_rc 2 || return 1
        assert_err_has 'AGENTS.md' || return 1
        assert_err_has_re 'unterminated|[Ff]ence' || return 1
        assert_argv_empty || return 1
    done <<'EOF'
shorter-backticks|``
wrong-character|~~~
non-bare-closer|``` trailing text
EOF
}

# 25. a heading line INSIDE the table fence is a comment row, not a section
#     boundary: the table still parses and its lane runs.
case_25() {
    fixture_new 25-heading-inside-fence || return 1
    fixture_init_repo || return 1
    crate_seed crates/x camel-x || return 1
    fixture_agents_preamble || return 1
    {
        printf '\n## COMPONENT LANES\n\n```\n'
        printf '## COMPONENT LANES\n'
        printf 'camel-x | local | cargo test -p camel-x\n'
        printf '```\n'
    } >>"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-x' || return 1
    assert_argv_exact 'test -p camel-x'
}

# 26. fence shape errors: an unterminated fence is exit 2 naming the OPENING
#     line, and an empty fence is exit 2 (no rows) — never a green no-op.
case_26() {
    fixture_new 26a-unterminated-fence || return 1
    fixture_init_repo || return 1
    crate_seed crates/x camel-x || return 1
    fixture_agents_preamble || return 1
    {
        printf '\n## COMPONENT LANES\n\n```\n'
        printf 'camel-x | local | cargo test -p camel-x\n'
    } >>"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    local open_line
    open_line="$(grep -n '^```$' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    if [[ -z "$open_line" ]]; then
        FAIL_DETAIL="fixture is wrong: no opening fence line found"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'unterminated fence' || return 1
    assert_err_has_re "line +${open_line}([^0-9]|$)" || return 1
    assert_argv_empty || return 1

    fixture_new 26b-empty-fence || return 1
    fixture_init_repo || return 1
    crate_seed crates/x camel-x || return 1
    fixture_agents || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'no rows' || return 1
    assert_argv_empty
}

# 27. `git ls-files` failing (rc 128) while merge-base succeeds is fail-closed
#     exit 2 with its own message, and the lane summary never prints.
case_27() {
    fixture_new 27-ls-files-failure || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-x | local | cargo test -p camel-x' || return 1
    crate_seed crates/x camel-x || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/x/src/lib.rs" || return 1
    commit_work "touch x" || return 1
    fixture_meta 'camel-x:crates/x:lib' || return 1
    install_failing_git ls-files || return 1
    if ! "$EXTRA_BIN/git" -C "$REPO" merge-base HEAD main >/dev/null 2>&1; then
        FAIL_DETAIL="fixture is wrong: shimmed git cannot compute merge-base"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'git ls-files failed' || return 1
    assert_out_lacks 'lanes passed:' || return 1
    assert_argv_empty
}

# 28. exotic changed paths survive the NUL-delimited transport: a committed path
#     with a space (git diff -z) and untracked tab/newline names (ls-files -z)
#     each map to their crate.
case_28() {
    fixture_new 28a-space-in-committed-path || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-weird | local | cargo test -p camel-weird' || return 1
    crate_seed crates/weird camel-weird || return 1
    fixture_base_commit || return 1
    printf 'x\n' >"$REPO/crates/weird/committed space.txt" || return 1
    commit_work "space path" || return 1
    fixture_meta 'camel-weird:crates/weird:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-weird' || return 1

    # This fixture needs a filesystem that allows tabs/newlines in names: a
    # shared scratch mount can reject them (EINVAL), and plain `mktemp -d`
    # honours $TMPDIR — which is exactly the hostile mount under the fleet
    # env. Probe candidate bases and use the first that accepts a tab name;
    # if none does, skip the sub-case LOUDLY (never silently).
    local alt_root="" cand probe
    for cand in "${GATES_LANES_TEST_TMPDIR:-}" "${TMPDIR:-}" /home/shared/tmp /dev/shm; do
        [ -n "$cand" ] && [ -d "$cand" ] && [ -w "$cand" ] || continue
        probe="$cand/.gates-lanes-probe.$$"$'\t'"x"
        if printf 'x\n' >"$probe" 2>/dev/null; then
            rm -f "$probe"
            alt_root="$(mktemp -d "$cand/gates-lanes-exotic.XXXXXX" 2>/dev/null)" || alt_root=""
            [ -n "$alt_root" ] && break
        fi
    done
    if [ -z "$alt_root" ]; then
        printf 'SKIP case 28: 28b sub-case skipped — no probed filesystem accepts tab/newline names\n'
        return 0
    fi
    EXTRA_ROOTS+=("$alt_root")
    fixture_new 28b-tab-newline-untracked-paths "$alt_root" || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-weird | local | cargo test -p camel-weird' || return 1
    crate_seed crates/weird camel-weird || return 1
    fixture_base_commit || return 1
    printf 'x\n' >"$REPO/crates/weird/tab"$'\t'"name.txt" || return 1
    printf 'x\n' >"$REPO/crates/weird/new"$'\n'"line.txt" || return 1
    fixture_meta 'camel-weird:crates/weird:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-weird'
}

# 29. overlapping package dirs (crates/a and crates/a/b): the longest matching
#     manifest dir wins, so a file in crates/a/b never dispatches crates/a.
case_29() {
    fixture_new 29-nested-package-dirs || return 1
    fixture_init_repo || return 1
    fixture_agents \
        'camel-a | local | cargo test -p camel-a' \
        'camel-b | local | cargo test -p camel-b' || return 1
    crate_seed crates/a camel-a || return 1
    crate_seed crates/a/b camel-b || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/a/b/src/lib.rs" || return 1
    commit_work "touch nested" || return 1
    fixture_meta 'camel-a:crates/a:lib' 'camel-b:crates/a/b:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-b' || return 1
    assert_out_lacks 'PASS lane camel-a' || return 1

    # The outer crate still maps its own files.
    printf '// touched\n' >>"$REPO/crates/a/src/lib.rs" || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_argv_line 'test -p camel-a' || return 1
    assert_argv_line 'test -p camel-b'
}

# 30. dispatch is keyed by PACKAGE NAME (the table's crate column) while path
#     mapping uses the manifest directory: the two differ here.
case_30() {
    fixture_new 30-package-name-differs-from-dir || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-pkg-name | local | cargo test -p camel-pkg-name' || return 1
    crate_seed crates/dirname camel-pkg-name || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/dirname/src/lib.rs" || return 1
    commit_work "touch dirname" || return 1
    fixture_meta 'camel-pkg-name:crates/dirname:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-pkg-name' || return 1
    assert_out_has_re '^no-lane: +0$' || return 1
    assert_argv_exact 'test -p camel-pkg-name'
}

# 31. lane failures: a command that cannot be found and a failing pipeline
#     under `bash -o pipefail` both FAIL the lane and the sweep.
case_31() {
    fixture_new 31a-invalid-command || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-badcmd | local | nonexistent-cmd-xyz' || return 1
    crate_seed crates/badcmd camel-badcmd || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/badcmd/src/lib.rs" || return 1
    commit_work "touch badcmd" || return 1
    fixture_meta 'camel-badcmd:crates/badcmd:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has 'FAIL lane camel-badcmd' || return 1
    assert_out_has_re '^lanes failed: +1( |$)' || return 1

    fixture_new 31b-failing-pipeline || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-badpipe | local | false | true' || return 1
    crate_seed crates/badpipe camel-badpipe || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/badpipe/src/lib.rs" || return 1
    commit_work "touch badpipe" || return 1
    fixture_meta 'camel-badpipe:crates/badpipe:lib' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has 'FAIL lane camel-badpipe :: false | true' || return 1
    assert_out_has_re '^lanes failed: +1( |$)'
}

# 32. selector precedence against a MALFORMED table: a disabled or not-selected
#     lane phase bypasses parsing entirely (exit 0, no bad-row error), while
#     GATES_ONLY=lanes surfaces the parse error.
case_32() {
    fixture_new 32-selector-precedence-malformed || return 1
    fixture_init_repo || return 1
    fixture_agents 'bad|row' || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    fixture_meta 'camel-component-llm:crates/llm:lib' || return 1

    run_sweep GATES_ONLY=lanes GATES_LANES=0
    assert_rc 0 || return 1
    assert_out_has 'component lanes: DISABLED (GATES_LANES=0)' || return 1
    assert_err_lacks 'bad row' || return 1
    assert_argv_empty || return 1

    run_sweep GATES_ONLY=lanes GATES_SKIP=lanes
    assert_rc 0 || return 1
    assert_out_has 'component lanes: DISABLED (GATES_SKIP=lanes)' || return 1
    assert_err_lacks 'bad row' || return 1
    assert_argv_empty || return 1

    run_sweep GATES_ONLY="fmt check"
    assert_rc 0 || return 1
    assert_out_has 'component lanes: not selected (GATES_ONLY)' || return 1
    assert_err_lacks 'bad row' || return 1
    assert_argv_empty || return 1

    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_has 'bad row' || return 1
    assert_argv_empty || return 1
}

# 33. scratch allocation failure is loud, not a silent pass: a nested run of a
#     COPY of this suite (never the repo file) with an unusable scratch path
#     exits 2 naming itself and runs no case.
case_33() {
    fixture_new 33-scratch-allocation-failure || return 1
    local copy="$BASE/copy"
    mkdir -p "$copy" || return 1
    cp "$SCRIPT_DIR/gates-local.sh" "$copy/gates-local.sh" || return 1
    cp "$SCRIPT_DIR/gates-lanes-test.sh" "$copy/gates-lanes-test.sh" || return 1
    # A regular file where the parent directory should be: mkdir -p fails for
    # root and non-root alike, unlike a permission-only simulation.
    printf 'not a directory\n' >"$BASE/blocked-file" || return 1
    local nested_out="$BASE/nested.out" nested_err="$BASE/nested.err" nested_rc=0
    (
        env -u TMPDIR -u GATES_LANES_TEST_TMPDIR \
            GATES_LANES_TEST_TMPDIR="$BASE/blocked-file/scratch" \
            bash "$copy/gates-lanes-test.sh"
    ) >"$nested_out" 2>"$nested_err" || nested_rc=$?
    if [[ "$nested_rc" -ne 2 ]]; then
        FAIL_DETAIL="nested harness exit $nested_rc, expected 2 (loud scratch failure)"
        return 1
    fi
    if ! grep -Fq 'gates-lanes-test:' "$nested_err"; then
        FAIL_DETAIL="nested harness stderr does not identify itself: [$(cat "$nested_err")]"
        return 1
    fi
    if ! grep -Eq 'scratch|cannot' "$nested_err"; then
        FAIL_DETAIL="nested harness stderr is not loud about the scratch dir: [$(cat "$nested_err")]"
        return 1
    fi
    if grep -Fq 'PASS case' "$nested_out"; then
        FAIL_DETAIL="nested harness ran cases despite the scratch failure"
        return 1
    fi
}

# 34. the round-2 regression: a four-backtick DOCUMENTATION EXAMPLE holding a
#     complete-line `## COMPONENT LANES` heading plus a lane row sits BEFORE the
#     real section. Discovery must treat the fenced example as example text and
#     parse the real section, so the fake row's `true` never executes and only
#     the real section's marker command reaches the argv log.
case_34() {
    fixture_new 34-four-backtick-example-before-section || return 1
    fixture_init_repo || return 1
    crate_seed crates/fake camel-fake || return 1
    fixture_agents_preamble || return 1
    cat >>"$REPO/AGENTS.md" <<'EOF'

## NOTES

The fenced example below shows what a lane table looks like. Its heading and
row are example text: the four-backtick fence swallows the inner table.

````
## COMPONENT LANES

```
camel-fake | local | true
```
````

## COMPONENT LANES

```
camel-fake | local | cargo test -p camel-fake --real-section
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/fake/src/lib.rs" || return 1
    commit_work "touch fake" || return 1
    fixture_meta 'camel-fake:crates/fake:lib' || return 1
    # Guard the fixture: a complete-line fake heading lives INSIDE the
    # four-backtick example, and the real heading follows the example.
    local headings fence_open fake_first real_first
    headings="$(grep -Fxc '## COMPONENT LANES' "$REPO/AGENTS.md")"
    if [[ "$headings" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: expected 2 complete-line ## COMPONENT LANES headings, got $headings"
        return 1
    fi
    if [[ "$(grep -Fxc '````' "$REPO/AGENTS.md")" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: the four-backtick example does not open and close"
        return 1
    fi
    fence_open="$(grep -Fnx '````' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    fake_first="$(grep -Fnx '## COMPONENT LANES' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    real_first="$(grep -Fnx '## COMPONENT LANES' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    if [[ -z "$fence_open" || -z "$fake_first" || -z "$real_first" ]] ||
        [[ "$fence_open" -ge "$fake_first" || "$fake_first" -ge "$real_first" ]]; then
        FAIL_DETAIL="fixture is wrong: fake heading ($fake_first) is not inside the example (opens at $fence_open) before the real heading ($real_first)"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-fake :: cargo test -p camel-fake --real-section' || return 1
    # The fake row's command would print this exact line: it must never run.
    assert_out_lacks 'PASS lane camel-fake :: true' || return 1
    assert_out_count 'PASS lane camel-fake' 1 || return 1
    assert_argv_exact 'test -p camel-fake --real-section'
}

# 35. the same shadowing scenario behind the other fence shapes: a `~~~` example
#     and a four-backtick opener closed by a LONGER run. Both examples are
#     properly closed, so the real section after them still wins.
case_35() {
    fixture_new 35a-tilde-fenced-example-before-section || return 1
    fixture_init_repo || return 1
    crate_seed crates/fake camel-fake || return 1
    fixture_agents_preamble || return 1
    cat >>"$REPO/AGENTS.md" <<'EOF'

## NOTES

A tilde-fenced example, closed with tildes of the same length.

~~~
## COMPONENT LANES

```
camel-fake | local | true
```
~~~

## COMPONENT LANES

```
camel-fake | local | cargo test -p camel-fake --real-tilde
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/fake/src/lib.rs" || return 1
    commit_work "touch fake" || return 1
    fixture_meta 'camel-fake:crates/fake:lib' || return 1
    if [[ "$(grep -Fxc '~~~' "$REPO/AGENTS.md")" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: the tilde example does not open and close"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-fake :: cargo test -p camel-fake --real-tilde' || return 1
    assert_out_lacks 'PASS lane camel-fake :: true' || return 1
    assert_argv_exact 'test -p camel-fake --real-tilde' || return 1

    # A longer closer (5 backticks closing a 4-backtick opener) also ends the
    # example, so its fake section stays example text.
    fixture_new 35b-longer-closer-example-before-section || return 1
    fixture_init_repo || return 1
    crate_seed crates/fake camel-fake || return 1
    fixture_agents_preamble || return 1
    cat >>"$REPO/AGENTS.md" <<'EOF'

## NOTES

A four-backtick example closed by a longer five-backtick run.

````
## COMPONENT LANES

```
camel-fake | local | true
```
`````

## COMPONENT LANES

```
camel-fake | local | cargo test -p camel-fake --real-longer-closer
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/fake/src/lib.rs" || return 1
    commit_work "touch fake" || return 1
    fixture_meta 'camel-fake:crates/fake:lib' || return 1
    if [[ "$(grep -Fxc '````' "$REPO/AGENTS.md")" -ne 1 ]] ||
        [[ "$(grep -Fxc '`````' "$REPO/AGENTS.md")" -ne 1 ]]; then
        FAIL_DETAIL="fixture is wrong: expected one four-backtick opener and one five-backtick closer"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-fake :: cargo test -p camel-fake --real-longer-closer' || return 1
    assert_out_lacks 'PASS lane camel-fake :: true' || return 1
    assert_argv_exact 'test -p camel-fake --real-longer-closer'
}

# 36. the surface contract is `tests/**/*.rs` OR an explicit `[[test]]` target,
#     so a tests/ file on disk blocks even when cargo metadata reports NO test
#     target (what `autotests = false` with no [[test]] emits). The stub
#     metadata is lib-only and the crate has no lane row.
case_36() {
    fixture_new 36-surface-tests-dir-without-test-target || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/auto camel-auto || return 1
    mkdir -p "$REPO/crates/auto/tests" || return 1
    printf '#[test]\nfn integration() {}\n' >"$REPO/crates/auto/tests/integration.rs" || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/auto/src/lib.rs" || return 1
    commit_work "touch auto" || return 1
    fixture_meta 'camel-auto:crates/auto:lib' 'camel-component-llm:crates/llm:lib' || return 1
    # Guard the fixture: the tests/ file exists on disk and metadata really
    # carries no test target (otherwise this case would restate case 3).
    if [[ ! -f "$REPO/crates/auto/tests/integration.rs" ]]; then
        FAIL_DETAIL="fixture is wrong: crates/auto/tests/integration.rs is missing"
        return 1
    fi
    if grep -Fq '"test"' "$META"; then
        FAIL_DETAIL="fixture is wrong: metadata carries a test target"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc_nonzero || return 1
    assert_out_has '!! NO LANE for camel-auto' || return 1
    assert_out_has 'blocking: crate has integration surface'
}

# 37. negative twin of 36: no tests/ file on disk and lib-only metadata is
#     genuinely no integration surface — loud but not blocking, exit 0.
case_37() {
    fixture_new 37-surface-no-tests-dir || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/plain camel-plain || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/plain/src/lib.rs" || return 1
    commit_work "touch plain" || return 1
    fixture_meta 'camel-plain:crates/plain:lib' 'camel-component-llm:crates/llm:lib' || return 1
    if [[ -d "$REPO/crates/plain/tests" ]]; then
        FAIL_DETAIL="fixture is wrong: crates/plain/tests exists"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has '!! NO LANE for camel-plain' || return 1
    assert_out_has 'not blocking: no integration surface'
}

# 38. scope of the filesystem check: a NESTED workspace member's tests/ must not
#     count toward the enclosing package. crates/a has lib-only metadata and no
#     own tests/; crates/a/b is a separate package in the same metadata and owns
#     the tests/ dir, so touching crates/a stays non-blocking.
case_38() {
    fixture_new 38-surface-nested-member-isolation || return 1
    fixture_init_repo || return 1
    fixture_agents 'camel-component-llm | local | true' || return 1
    crate_seed crates/a camel-a || return 1
    crate_seed crates/a/b camel-b || return 1
    mkdir -p "$REPO/crates/a/b/tests" || return 1
    printf '#[test]\nfn nested() {}\n' >"$REPO/crates/a/b/tests/nested.rs" || return 1
    crate_seed crates/llm camel-component-llm || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/a/src/lib.rs" || return 1
    commit_work "touch a" || return 1
    fixture_meta 'camel-a:crates/a:lib' 'camel-b:crates/a/b:lib' 'camel-component-llm:crates/llm:lib' || return 1
    # Guard the fixture: crates/a has no tests/ of its own, the nested member
    # does, and the touched path maps to crates/a.
    if [[ -d "$REPO/crates/a/tests" || ! -f "$REPO/crates/a/b/tests/nested.rs" ]]; then
        FAIL_DETAIL="fixture is wrong: tests/ dirs are not where this case needs them"
        return 1
    fi
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has '!! NO LANE for camel-a' || return 1
    assert_out_has 'not blocking: no integration surface' || return 1
    assert_out_lacks '!! NO LANE for camel-b'
}

# ── registry parser (## QUALITY GATES) ──────────────────────────────────────
# The registry phase extracts the fenced gate block under the first complete-line
# `## QUALITY GATES` heading found OUTSIDE fences. These cases pin the hardened
# parser: fence-interior headings never hijack extraction, hostile examples above
# and below cannot widen or narrow the gate set, every malformed registry is a
# NAMED exit 2 (file + line) with NO dispatch (validate-then-emit), and command
# bytes reach `bash -c` verbatim. Byte-exactness is observed through
# `$BASH_EXECUTION_STRING`, which carries the complete operand; `$BASH_COMMAND`
# only names the running command and would truncate multi-line operands.

# 39. a four-backtick doc example holding a complete-line `## QUALITY GATES`
#     heading and a fake gate sits BEFORE the real section. Extraction must take
#     the real section, so the fake gate never runs.
case_39() {
    fixture_new 39-registry-fenced-example-above || return 1
    fixture_init_repo || return 1
    cat >"$REPO/AGENTS.md" <<'EOF'
# gates-lanes-test fixture

## NOTES

The fenced example below shows the registry shape. Its heading is example text:
the four-backtick fence swallows the inner block.

````
## QUALITY GATES

```
- name: fake
  run: 'true'
```
````

## QUALITY GATES

```
- name: real
  run: 'true'
```
EOF
    fixture_base_commit || return 1
    local headings fake_line real_line
    headings="$(grep -Fxc '## QUALITY GATES' "$REPO/AGENTS.md")"
    if [[ "$headings" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: expected 2 complete-line ## QUALITY GATES headings, got $headings"
        return 1
    fi
    if [[ "$(grep -Fxc '````' "$REPO/AGENTS.md")" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: the four-backtick example does not open and close"
        return 1
    fi
    fake_line="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    real_line="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    if [[ -z "$fake_line" || -z "$real_line" || "$fake_line" -ge "$real_line" ]]; then
        FAIL_DETAIL="fixture is wrong: fake heading ($fake_line) is not before the real heading ($real_line)"
        return 1
    fi
    run_sweep GATES_LANES=0
    assert_rc 0 || return 1
    assert_out_has '=== real' || return 1
    assert_out_has 'PASS real' || return 1
    assert_out_lacks '=== fake' || return 1
    assert_out_count '=== real' 1
}

# 40. hostile examples above AND below the real section: the gate set stays
#     exactly the real one, in registry order, and each operand reaches bash
#     byte-identically (quote/pipe bytes ride in a shell comment; the multi-line
#     gate carries a literal backslash-n and a TAB inside a quoted argument).
case_40() {
    fixture_new 40-registry-hostile-above-and-below || return 1
    fixture_init_repo || return 1
    local T="$REPO/traces" tab=$'\t'
    local g1 g2_l1 g2_l2
    g1="printf '%s' \"\$BASH_EXECUTION_STRING\" > \"\$T\"/g1 # a|b\"c"
    g2_l1="printf '%s' \"\$BASH_EXECUTION_STRING\" > \"\$T\"/g2; : 'x\n${tab}y'"
    g2_l2="printf 'tail' >> \"\$T\"/g2"
    {
        printf '%s\n' '# gates-lanes-test fixture' ''
        printf '%s\n' '## NOTES' ''
        printf '%s\n' 'A hostile fenced example ABOVE the real section.' ''
        printf '%s\n' '````' '## QUALITY GATES' '' '```' '- name: fakeabove' "  run: 'true'" '```' '````' ''
        printf '%s\n' '## QUALITY GATES' '' '```'
        printf '%s\n' '- name: g1' "  run: $g1"
        printf '%s\n' '- name: g2' '  run: |'
        printf '    %s\n' "$g2_l1" "$g2_l2"
        printf '%s\n' '```' ''
        printf '%s\n' '## NOTES' ''
        printf '%s\n' 'A hostile fenced example BELOW the real section.' ''
        printf '%s\n' '````' '## QUALITY GATES' '' '```' '- name: fakebelow' "  run: 'true'" '```' '````'
    } >"$REPO/AGENTS.md" || return 1
    mkdir -p "$T" || return 1
    fixture_base_commit || return 1
    # Expected operands: g1 is one line; g2 is L1, newline, L2 then the appended
    # `tail` (both printfs carry no trailing newline, so the file is L1\nL2tail).
    local want_g1="$BASE/expected-g1" want_g2="$BASE/expected-g2"
    printf '%s' "$g1" >"$want_g1" || return 1
    {
        printf '%s\n' "$g2_l1"
        printf '%s' "$g2_l2"
        printf 'tail'
    } >"$want_g2" || return 1
    local headings fake_above real_line fake_below
    headings="$(grep -Fxc '## QUALITY GATES' "$REPO/AGENTS.md")"
    if [[ "$headings" -ne 3 ]]; then
        FAIL_DETAIL="fixture is wrong: expected 3 complete-line ## QUALITY GATES headings, got $headings"
        return 1
    fi
    if [[ "$(grep -Fxc '````' "$REPO/AGENTS.md")" -ne 4 ]]; then
        FAIL_DETAIL="fixture is wrong: expected two closed four-backtick examples"
        return 1
    fi
    fake_above="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    real_line="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | sed -n 2p | cut -d: -f1)"
    fake_below="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    if [[ -z "$fake_above" || -z "$real_line" || -z "$fake_below" ]] ||
        [[ "$fake_above" -ge "$real_line" || "$real_line" -ge "$fake_below" ]]; then
        FAIL_DETAIL="fixture is wrong: headings are not fakeabove ($fake_above) < real ($real_line) < fakebelow ($fake_below)"
        return 1
    fi
    run_sweep GATES_LANES=0 T="$T"
    assert_rc 0 || return 1
    assert_out_has 'PASS g1' || return 1
    assert_out_has 'PASS g2' || return 1
    assert_out_order '=== g1' '=== g2' || return 1
    assert_out_lacks '=== fakeabove' || return 1
    assert_out_lacks '=== fakebelow' || return 1
    assert_file_bytes "$T/g1" "$want_g1" || return 1
    assert_file_bytes "$T/g2" "$want_g2" || return 1
    assert_out_count '=== g1' 1
}

# 41. a `~~~` fence opened before the heading and never closed is a NAMED error
#     (opening line named before the missing-section message) and nothing is
#     dispatched — a fence swallowed the heading, so it was never a section.
case_41() {
    fixture_new 41-registry-unclosed-fence-above || return 1
    fixture_init_repo || return 1
    cat >"$REPO/AGENTS.md" <<'EOF'
~~~
intro

## QUALITY GATES

```
- name: real
  run: 'true'
```
EOF
    fixture_base_commit || return 1
    local open_line
    open_line="$(grep -Fnx '~~~' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    if [[ -z "$open_line" ]]; then
        FAIL_DETAIL="fixture is wrong: no ~~~ opener found"
        return 1
    fi
    run_sweep GATES_LANES=0
    assert_rc 2 || return 1
    assert_err_has_re "unterminated fence at line +${open_line}([^0-9]|$)" || return 1
    assert_err_has 'no ## QUALITY GATES section' || return 1
    assert_out_lacks_re '^=== ' || return 1
    assert_argv_empty
}

# 42. a NUL byte inside a `run:` payload is a named exit 2 BEFORE any dispatch:
#     the valid first gate must not run (validate-then-emit).
case_42() {
    fixture_new 42-registry-nul-in-run-payload || return 1
    fixture_init_repo || return 1
    {
        printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
        printf '%s\n' '- name: noop' "  run: 'true'" '- name: bad'
        printf "  run: 'tr\0ue'\n"
        printf '%s\n' '```'
    } >"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    local nul_line
    nul_line="$(nul_line_of_agents)"
    if [[ -z "$nul_line" ]]; then
        FAIL_DETAIL="fixture is wrong: AGENTS.md carries no NUL byte"
        return 1
    fi
    run_sweep GATES_LANES=0
    assert_rc 2 || return 1
    assert_err_has_re "NUL byte at line +${nul_line}([^0-9]|$)" || return 1
    assert_out_lacks_re '^=== ' || return 1
    assert_argv_empty
}

# 43. a literal TAB in a gate name is a field-injection vector on the TSV
#     transport: it is a named exit 2 (file + line, mentions the name) and the
#     gate never runs.
case_43() {
    fixture_new 43-registry-tab-in-name || return 1
    fixture_init_repo || return 1
    {
        printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
        printf -- '- name: gate\tinject\n'
        printf '%s\n' "  run: 'true'" '```'
    } >"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    if ! grep -Fq -- '- name: gate'$'\t''inject' "$REPO/AGENTS.md"; then
        FAIL_DETAIL="fixture is wrong: the tab in the gate name did not survive"
        return 1
    fi
    run_sweep GATES_LANES=0
    assert_rc 2 || return 1
    assert_err_has 'AGENTS.md' || return 1
    assert_err_has_re '(line|row) +[0-9]+' || return 1
    assert_err_has 'name' || return 1
    assert_out_lacks_re '^=== ' || return 1
    assert_argv_empty
}

# 44. grammar rejections, parameterized: every malformed registry is a named
#     exit 2 with file + line/row and no dispatch.
case_44() {
    local tag
    for tag in second-run continuation-before-run stray-line empty-name run-before-name no-run empty-run-value; do
        fixture_new "44-${tag}" || return 1
        fixture_init_repo || return 1
        case "$tag" in
        second-run)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name: g' "  run: 'true'" "  run: 'false'" '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        continuation-before-run)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name: g' '    early continuation' "  run: 'true'" '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        stray-line)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name: g' 'not-a-gate-line' "  run: 'true'" '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        empty-name)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name:' "  run: 'true'" '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        run-before-name)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' "  run: 'true'" '- name: g' '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        no-run)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name: g' '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        empty-run-value)
            {
                printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
                printf '%s\n' '- name: g' '  run:' '```'
            } >"$REPO/AGENTS.md" || return 1
            ;;
        esac
        fixture_base_commit || return 1
        run_sweep GATES_LANES=0
        assert_rc 2 || return 1
        assert_err_names_row 'AGENTS.md' || return 1
        assert_out_lacks_re '^=== ' || return 1
        assert_argv_empty || return 1
        if [[ "$tag" == empty-run-value ]]; then
            assert_err_has 'empty run' || return 1
        fi
    done
}

# 45. the registry twin of case 21: pipe, quotes, a literal backslash-n, a TAB,
#     a UTF-8 byte and a shell line continuation all reach `bash -c` byte-exactly.
case_45() {
    fixture_new 45-registry-exotic-command || return 1
    fixture_init_repo || return 1
    local T="$REPO/traces" tab=$'\t'
    local x1 x2 x3 x4
    x1="printf '%s' \"\$BASH_EXECUTION_STRING\" > \"\$T\"/exotic"
    x2="printf '%s\n' 'a${tab}b' > sed.in && sed 's/a\nb/x/' sed.in | tee 'out 1'.log"
    x3="printf 'x' --flag=é \\"
    x4="--more"
    {
        printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
        printf '%s\n' '- name: exotic' '  run: |'
        printf '    %s\n' "$x1" "$x2" "$x3"
        printf '    %s\n' "$x4"
        printf '%s\n' '```'
    } >"$REPO/AGENTS.md" || return 1
    mkdir -p "$T" || return 1
    fixture_base_commit || return 1
    # Expected operand: four continuation lines joined by newlines, with the
    # last line carrying no terminator (the parser strips its indentation).
    local want="$BASE/expected-exotic"
    {
        printf '%s\n' "$x1"
        printf '%s\n' "$x2"
        printf '%s\n' "$x3"
        printf '%s' "$x4"
    } >"$want" || return 1
    run_sweep GATES_LANES=0 T="$T"
    assert_rc 0 || return 1
    assert_file_bytes "$T/exotic" "$want" || return 1
    assert_out_has 'PASS exotic'
}

# 46. registry scratch allocation failure is a named exit 2, not a silent pass
#     or a raw mktemp error: a sweep-only mktemp stub fails every rung.
case_46() {
    fixture_new 46-registry-scratch-allocation-failure || return 1
    fixture_init_repo || return 1
    fixture_agents_preamble || return 1
    fixture_base_commit || return 1
    EXTRA_BIN="$BASE/stub-mktemp"
    mkdir -p "$EXTRA_BIN" || return 1
    {
        printf '#!/usr/bin/env bash\n'
        printf '# Stub mktemp: the registry scratch ladder must fail through this.\n'
        printf 'set -uo pipefail\n'
        printf 'echo "stub mktemp: exploded on purpose" >&2\n'
        printf 'exit 1\n'
    } >"$EXTRA_BIN/mktemp" || return 1
    chmod +x "$EXTRA_BIN/mktemp" || return 1
    # Guard the stub: an mkdtemp that works would make this case meaningless.
    if "$EXTRA_BIN/mktemp" -d >/dev/null 2>&1; then
        FAIL_DETAIL="fixture is wrong: the mktemp stub succeeded"
        return 1
    fi
    run_sweep GATES_LANES=0
    assert_rc 2 || return 1
    assert_err_has 'cannot create a scratch dir' || return 1
    assert_err_has 'registry' || return 1
    assert_out_lacks_re '^=== ' || return 1
    assert_argv_empty
}

# 47. the SAME hostile fixture through BOTH parsers: the registry takes only its
#     real gates block and the lanes phase only its real table, so the fake
#     headings inside the four-backtick example never widen either surface.
case_47() {
    fixture_new 47-registry-and-lanes-same-hostile-input || return 1
    fixture_init_repo || return 1
    cat >"$REPO/AGENTS.md" <<'EOF'
# gates-lanes-test fixture

## NOTES

The four-backtick example holds BOTH fake sections; neither is a section.

````
## QUALITY GATES

```
- name: fakereg
  run: 'true'
```

## COMPONENT LANES

```
camel-fake | local | true
```
````

## QUALITY GATES

```
- name: noop
  run: 'true'
```

## COMPONENT LANES

```
camel-real | local | cargo test -p camel-real
```
EOF
    crate_seed crates/real camel-real || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/real/src/lib.rs" || return 1
    commit_work "touch real" || return 1
    fixture_meta 'camel-real:crates/real:lib' || return 1
    local fence_open fence_close fake_reg fake_lanes real_reg real_lanes
    if [[ "$(grep -Fxc '````' "$REPO/AGENTS.md")" -ne 2 ]] ||
        [[ "$(grep -Fxc '## QUALITY GATES' "$REPO/AGENTS.md")" -ne 2 ]] ||
        [[ "$(grep -Fxc '## COMPONENT LANES' "$REPO/AGENTS.md")" -ne 2 ]]; then
        FAIL_DETAIL="fixture is wrong: expected one closed four-backtick example holding both fake headings"
        return 1
    fi
    fence_open="$(grep -Fnx '````' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    fence_close="$(grep -Fnx '````' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    fake_reg="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    real_reg="$(grep -Fnx '## QUALITY GATES' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    fake_lanes="$(grep -Fnx '## COMPONENT LANES' "$REPO/AGENTS.md" | head -n1 | cut -d: -f1)"
    real_lanes="$(grep -Fnx '## COMPONENT LANES' "$REPO/AGENTS.md" | tail -n1 | cut -d: -f1)"
    if [[ -z "$fence_open" || -z "$fence_close" || -z "$fake_reg" || -z "$fake_lanes" ||
        -z "$real_reg" || -z "$real_lanes" ]] ||
        [[ "$fence_open" -ge "$fake_reg" || "$fake_reg" -ge "$fake_lanes" ||
            "$fake_lanes" -ge "$fence_close" || "$fence_close" -ge "$real_reg" ||
            "$real_reg" -ge "$real_lanes" ]]; then
        FAIL_DETAIL="fixture is wrong: fake headings are not inside the example before both real headings"
        return 1
    fi
    run_sweep
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-real' || return 1
    assert_out_has 'PASS noop' || return 1
    assert_out_has 'PASS lane camel-real' || return 1
    assert_out_lacks 'fakereg' || return 1
    assert_out_lacks 'camel-fake' || return 1
    assert_out_lacks 'PASS lane camel-fake :: true'
}

# 48. the registry summary counts pass/fail/skip exactly, and skip labels print
#     in registry order (the `${skipped[*]}` append order, not GATES_SKIP order).
case_48() {
    fixture_new 48-registry-summary-counts || return 1
    fixture_init_repo || return 1
    {
        printf '%s\n' '# gates-lanes-test fixture' '' '## QUALITY GATES' '' '```'
        printf '%s\n' '- name: ok1' "  run: 'true'"
        printf '%s\n' '- name: bad' "  run: 'false'"
        printf '%s\n' '- name: ok2' "  run: 'true'"
        printf '%s\n' '```'
    } >"$REPO/AGENTS.md" || return 1
    fixture_base_commit || return 1
    run_sweep GATES_LANES=0
    assert_rc_nonzero || return 1
    assert_out_line 'passed:  2' || return 1
    assert_out_line 'failed:  1 — bad' || return 1
    assert_out_line 'skipped: 0' || return 1
    assert_out_line 'ran:     3 of 3' || return 1
    run_sweep GATES_LANES=0 GATES_SKIP="ok1 ok2 bad"
    assert_rc 0 || return 1
    assert_out_line 'skipped: 3 — ok1 bad ok2' || return 1
    assert_out_line 'ran:     0 of 3'
}

# 49. the workspace-unification lane (camel-test's suite row) is exercised from
#     the PRODUCTION declaration in AGENTS.md: default list, allowlist override,
#     blank/wildcard guards, failure aggregation, GATES_SKIP, malformed twin.
PROD_AGENTS="$(cd "$SCRIPT_DIR/.." && pwd)/AGENTS.md"
prod_row="$(sed -n 's/^camel-test | local | suites=/suites=/p' "$PROD_AGENTS")"

case_49() {
    # 49a — the production default list dispatches in table order.
    fixture_new 49a-default-list || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    unset CAMEL_TEST_WORKSPACE_SUITES
    run_sweep GATES_ONLY=lanes
    assert_rc 0 || return 1
    assert_out_has 'PASS lane camel-test' || return 1
    assert_out_lacks 'NO LANE for camel-test' || return 1
    assert_argv_exact 'test -p camel-test --features integration-sql --test sql_deadline_ordering'$'\n''test -p camel-test --features integration-tests --no-run'$'\n''test --workspace --test dev_watch_script_test'$'\n''test --workspace --test dev_watch_proto_test' || return 1

    # 49b — an allowlisted subset override replaces the default list.
    fixture_new 49b-allowlisted-subset || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes CAMEL_TEST_WORKSPACE_SUITES='dev_watch_proto_test'
    assert_rc 0 || return 1
    assert_argv_exact 'test -p camel-test --features integration-sql --test sql_deadline_ordering'$'\n''test -p camel-test --features integration-tests --no-run'$'\n''test --workspace --test dev_watch_proto_test' || return 1

    # 49c — an empty list is a named error and dispatches no workspace call
    #       while the compile-only row still runs.
    fixture_new 49c-empty-list || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes CAMEL_TEST_WORKSPACE_SUITES=
    assert_rc_nonzero || return 1
    assert_argv_line 'test -p camel-test --features integration-tests --no-run' || return 1
    if grep -q -- '--workspace' "$ARGV"; then
        FAIL_DETAIL="empty override dispatched a workspace cargo call"
        return 1
    fi
    assert_out_has 'FAIL lane camel-test :: suites=' || return 1
    assert_err_has 'empty CAMEL_TEST_WORKSPACE_SUITES suite list' || return 1

    # 49d — a whitespace-only list is the same empty list.
    fixture_new 49d-whitespace-list || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes 'CAMEL_TEST_WORKSPACE_SUITES=   '
    assert_rc_nonzero || return 1
    assert_argv_line 'test -p camel-test --features integration-tests --no-run' || return 1
    if grep -q -- '--workspace' "$ARGV"; then
        FAIL_DETAIL="empty override dispatched a workspace cargo call"
        return 1
    fi
    assert_out_has 'FAIL lane camel-test :: suites=' || return 1
    assert_err_has 'empty CAMEL_TEST_WORKSPACE_SUITES suite list' || return 1

    # 49e — a wildcard entry is rejected literally, never expanded against the
    #       worktree (the row's set -f guard).
    fixture_new 49e-wildcard-entry || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    printf 'x\n' >"$REPO/matching_test" || return 1
    run_sweep GATES_ONLY=lanes 'CAMEL_TEST_WORKSPACE_SUITES=dev_watch_script_test *_test'
    assert_rc_nonzero || return 1
    assert_argv_line 'test -p camel-test --features integration-tests --no-run' || return 1
    if grep -q -- '--workspace' "$ARGV"; then
        FAIL_DETAIL="wildcard override dispatched a workspace cargo call"
        return 1
    fi
    assert_out_has 'FAIL lane camel-test :: suites=' || return 1
    assert_err_has 'literal suite names' || return 1

    # 49f — a failing first suite still runs the second (aggregation).
    fixture_new 49f-first-suite-failure || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    unset CAMEL_TEST_WORKSPACE_SUITES
    run_sweep GATES_ONLY=lanes STUB_TEST_FAIL_AT=3
    assert_rc_nonzero || return 1
    assert_argv_exact 'test -p camel-test --features integration-sql --test sql_deadline_ordering'$'\n''test -p camel-test --features integration-tests --no-run'$'\n''test --workspace --test dev_watch_script_test'$'\n''test --workspace --test dev_watch_proto_test' || return 1
    assert_out_has 'FAIL lane camel-test :: suites=' || return 1

    # 49g — GATES_SKIP=lanes disables the row entirely.
    fixture_new 49g-skip-lanes || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes GATES_SKIP=lanes
    assert_rc 0 || return 1
    assert_argv_empty || return 1
    assert_out_has 'component lanes: DISABLED (GATES_SKIP=lanes)' || return 1

    # 49h — a malformed twin row (bad mode) is still a named parse error.
    if [[ "$(printf '%s\n' "$prod_row" | wc -l)" -eq 1 && -n "$prod_row" ]]; then
        :
    else
        FAIL_DETAIL="production lane declaration missing or duplicated"
        return 1
    fi
    fixture_new 49h-malformed-twin || return 1
    fixture_init_repo || return 1
    fixture_agents "camel-test | locall | $prod_row" || return 1
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes
    assert_rc 2 || return 1
    assert_err_names_row 'AGENTS.md' || return 1
    assert_argv_empty || return 1

    # 49i — a Docker-classified suite is not an allowlisted local suite.
    fixture_new 49i-docker-override || return 1
    fixture_init_repo || return 1
    cp "$PROD_AGENTS" "$REPO/AGENTS.md" || return 1
    if [[ "$(grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md")" -lt 1 ]]; then
        FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"
        return 1
    fi
    crate_seed crates/test camel-test || return 1
    mkdir -p "$REPO/crates/test/tests" || return 1
    printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs" || return 1
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/test/src/lib.rs" || return 1
    commit_work "touch camel-test" || return 1
    fixture_meta 'camel-test:crates/test:lib,test' || return 1
    run_sweep GATES_ONLY=lanes CAMEL_TEST_WORKSPACE_SUITES='redis_state_test'
    assert_rc_nonzero || return 1
    assert_argv_line 'test -p camel-test --features integration-tests --no-run' || return 1
    if grep -q -- '--workspace' "$ARGV"; then
        FAIL_DETAIL="rejected override dispatched a workspace cargo call"
        return 1
    fi
    assert_err_has 'not an allowlisted local suite' || return 1
    assert_err_has 'redis_state_test' || return 1
}

# 50. the lane phase cannot read/decode AGENTS.md (a registry gate rewrote it
#     with invalid UTF-8 after the registry parse): a named exit 2, no traceback,
#     and no lane command dispatched.
case_50() {
    fixture_new 50-lanes-read-guard || return 1
    fixture_init_repo || return 1
    crate_seed crates/ok camel-ok || return 1
    # Written by hand: fixture_agents_preamble's registry gate is `noop`, and
    # this fixture needs a gate that corrupts AGENTS.md at RUN time (the octal
    # escapes produce the invalid bytes, so the committed file stays valid).
    cat >"$REPO/AGENTS.md" <<'EOF' || return 1
# gates-lanes-test fixture

## QUALITY GATES

```
- name: corrupt
  run: printf '\377\376' >> AGENTS.md
```

## COMPONENT LANES

```
camel-ok | local | cargo test -p camel-ok
```
EOF
    fixture_base_commit || return 1
    printf '// touched\n' >>"$REPO/crates/ok/src/lib.rs" || return 1
    commit_work "touch ok" || return 1
    fixture_meta 'camel-ok:crates/ok:lib' || return 1
    run_sweep "GATES_ONLY=corrupt lanes"
    local repo_root
    if ! repo_root="$(git -C "$REPO" rev-parse --show-toplevel)"; then
        FAIL_DETAIL="fixture is wrong: cannot resolve the fixture repo root"
        return 1
    fi
    assert_rc 2 || return 1
    assert_err_has "gates-local: lanes: cannot read $repo_root/AGENTS.md: 'utf-8' codec can't decode" || return 1
    assert_err_lacks 'Traceback' || return 1
    assert_argv_empty
}

# ── driver ──────────────────────────────────────────────────────────────────
CASES=(
    "touched crate with lane + tests/ executes the lane"
    "lane command failure propagates"
    "missing lane + tests/**/*.rs blocks"
    "missing lane + explicit [[test]] target outside tests/ blocks"
    "missing lane + feature-gated tests/ file blocks"
    "missing lane + no integration surface is loud but not blocking"
    "tooling-only touched tree runs no lanes"
    "GATES_LANES=0 disables the phase / GATES_LANES=all runs docker rows"
    "GATES_ONLY and GATES_SKIP selectors print and honour"
    "cross-crate rename surfaces both crates"
    "dirty and untracked files count as touches"
    "malformed lane rows exit 2 naming the row"
    "fail-closed on cargo-metadata and merge-base failures"
    "marker in prose only is not a section (exit 2, no lane)"
    "fence in a later section is not the lane table (exit 2)"
    "git diff failure is fail-closed (exit 2, no lane summary)"
    "docker skips count rows; registry summary precedes the lane phase"
    "registry report survives a lane-table parse error"
    "NUL byte in a table row is rejected with file+line"
    "whitespace in the crate field cannot inject fields"
    "exotic lane command reaches bash byte-identically"
    "structurally invalid metadata is loud exit 2"
    "the lane phase ignores its own scratch subtree"
    "only a bare fence closer closes the table"
    "a heading inside the fence is a comment row"
    "unterminated and empty fences are exit 2"
    "git ls-files failure is fail-closed (exit 2)"
    "exotic changed paths map to their crate"
    "nested package dirs pick the longest prefix"
    "dispatch is keyed by package name, not directory"
    "invalid command and failing pipeline FAIL the lane"
    "disabled/not-selected phases bypass a malformed table"
    "scratch allocation failure is loud, not silent"
    "four-backtick doc example before the real section never wins"
    "tilde fence and longer-closer doc examples never win"
    "tests/ on disk blocks even without a metadata test target"
    "no tests/ dir and lib-only metadata is not blocking"
    "a nested member's tests/ does not block the enclosing crate"
    "registry: fenced example does not hijack extraction"
    "registry: hostile example above AND below, gates exact"
    "registry: unclosed fence before the heading is a named error"
    "registry: NUL in a run payload is rejected"
    "registry: tab in a gate name cannot inject fields"
    "registry: grammar rejections are named, file+line"
    "registry: exotic command reaches bash byte-identically"
    "registry: scratch allocation failure is a named exit 2"
    "registry and lanes: same hostile input, both parsers win"
    "registry: summary counts on pass/fail/skip"
    "workspace-unification lane: production declaration, allowlist override, blank/wildcard guard, failure aggregation, skip, malformed row"
    "lanes: unreadable lane table is a named exit 2 (read/decode guard)"
)

main() {
    install_stub_cargo || {
        echo "gates-lanes-test: cannot install the stub cargo" >&2
        exit 2
    }

    local total=0 passed=0 failed=0
    local index=0 name rc
    for name in "${CASES[@]}"; do
        index=$((index + 1))
        FAIL_DETAIL=""
        SWEEP_CODE=0
        rc=0
        "case_$(printf '%02d' "$index")" || rc=$?
        total=$((total + 1))
        if [[ "$rc" -eq 0 ]]; then
            printf 'PASS case %d: %s\n' "$index" "$name"
            passed=$((passed + 1))
        else
            printf 'FAIL case %d: %s\n' "$index" "$name"
            printf '  detail: %s\n' "${FAIL_DETAIL:-assertion failed without detail (a fixture step failed?)}"
            printf '  artifacts: %s\n' "${BASE:-unknown}"
            failed=$((failed + 1))
        fi
    done

    printf '%d/%d cases green\n' "$passed" "$total"
    if [[ "$failed" -ne 0 ]]; then
        exit 1
    fi
}

main "$@"
