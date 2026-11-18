#!/usr/bin/env bash
# gates-local.sh — run the local subset of the AGENTS.md QUALITY GATES registry.
#
# Why this exists (2026-11-08, v0.66.0): the registry is the single source of
# truth, but in practice the conductor ran a partial, hand-copied subset — once
# per mission, not on the merge result. Two recurring failures came from gates
# nobody ran locally: `lint-secrets` (a fixture literal) and
# `lint-unbounded-wait` (a bare channel receive in a new test). Both are cheap.
#
# This script PARSES the registry out of AGENTS.md instead of copying it, so a
# new gate is picked up here the moment it lands there. Gates that need the
# network or infra are skipped by name and left to CI.
#
# Usage:
#   bash scripts/gates-local.sh                 # run everything not skipped
#   GATES_ONLY="fmt check lint-unwrap" bash scripts/gates-local.sh
#   GATES_SKIP="" bash scripts/gates-local.sh   # also run the skipped ones
#   GATES_LANES=0 bash scripts/gates-local.sh   # skip the component-lane phase
#   GATES_LANES=all bash scripts/gates-local.sh # also run infra (docker) lanes
#
# The summary reports `passed:`, `failed:`, `skipped:` and `ran: X of Y`, so a
# sweep always states how much of the registry it covered. A partial sweep
# (GATES_ONLY non-empty) is labeled PARTIAL in the divider and names the
# `not selected:` gates, so an incomplete report can never read as a full one.
#
# After the registry sweep AND its summary, the COMPONENT LANES phase (AGENTS.md
# `## COMPONENT LANES`) runs the integration lane of every workspace crate the
# tree touches; its own summary follows the registry one, so a lane parse or
# execution error can never suppress the registry report.
# `GATES_SKIP`/`GATES_ONLY` treat it as gate name `lanes`, and a package name in
# GATES_SKIP drops its rows. `lanes passed/failed/skipped` count ROWS, not
# crates: two docker rows skipped is `lanes skipped: 2`.
#
# Exit: 0 only when every executed gate passed, no lane failed, and no touched
# crate with an integration surface lacks a lane.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
AGENTS="$ROOT/AGENTS.md"
# Network / infra / interactive gates: CI owns them.
#   lint-commits        -> git fetch origin main (network)
#   doc-guide-test      -> needs mdbook + docs toolchain
#   cargo-audit         -> advisory DB download
#   audit               -> cargo-audit wrapper, same DB
#   lint-bridge-releases -> GitHub release API (published tag and assets)
SKIP="${GATES_SKIP-lint-commits doc-guide-test cargo-audit audit lint-bridge-releases}"
ONLY="${GATES_ONLY:-}"

[[ -f "$AGENTS" ]] || { echo "gates-local: no AGENTS.md at $AGENTS" >&2; exit 2; }

# python3 parses the registry block; keep the failure message actionable.
PY_BIN="$(command -v python3 || command -v python || true)"
[[ -n "$PY_BIN" ]] || {
    echo "gates-local: python3 (or python) must be on PATH to parse AGENTS.md" >&2
    exit 2
}

# The lane phase stages NUL-delimited git output in a scratch dir; the trap keeps
# it from leaking on any exit path (including the fail-closed exit 2s).
lane_tmp=""
lane_cleanup() {
    if [[ -n "$lane_tmp" && -d "$lane_tmp" ]]; then
        rm -rf "$lane_tmp"
    fi
}
trap lane_cleanup EXIT

mapfile -t GATES < <("$PY_BIN" - "$AGENTS" <<'PY'
import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
start = text.index("## QUALITY GATES")
block = text[start:]
open_fence = block.index("```") + 3
close_fence = block.index("```", open_fence)
body = block[open_fence:close_fence]

gates = []
name = None
run = []
run_sep = " "


def strip_comment(value):
    """Drop a YAML inline comment: `name: x   # note` -> `x`."""
    marker = value.find("  #")
    if marker != -1:
        value = value[:marker]
    return value.rstrip()


def flush():
    if name:
        gates.append((name, run_sep.join(run).strip()))


for line in body.splitlines():
    m = re.match(r"^- name:\s*(.+?)\s*$", line)
    if m:
        flush()
        name, run, run_sep = strip_comment(m.group(1)).strip(), [], " "
        continue
    m = re.match(r"^\s+run:\s*(.*)$", line)
    if m:
        value = m.group(1)
        if value[:1] in ("|", ">"):
            # Literal/folded block: one shell command per line.
            run, run_sep = [], "\n"
        else:
            # Plain scalar wrapped over several lines: YAML folds with spaces.
            run, run_sep = [strip_comment(value)], " "
        continue
    if name and line.startswith("    "):
        continuation = line.strip()
        if continuation:
            run.append(continuation)
flush()

for gate_name, command in gates:
    print(gate_name.replace("\t", " ") + "\t" + command.replace("\n", "\\n"))
PY
)

[[ ${#GATES[@]} -gt 0 ]] || { echo "gates-local: parsed 0 gates from AGENTS.md" >&2; exit 2; }

failed=()
passed=()
skipped=()
not_selected=()

if [[ -n "$ONLY" ]]; then
    printf 'gates-local: PARTIAL sweep — GATES_ONLY="%s"\n' "$ONLY"
fi

for entry in "${GATES[@]}"; do
    name="${entry%%$'\t'*}"
    command="${entry#*$'\t'}"
    command="${command//\\n/$'\n'}"

    if [[ " $SKIP " == *" $name "* ]]; then
        skipped+=("$name")
        continue
    fi
    if [[ -n "$ONLY" && " $ONLY " != *" $name "* ]]; then
        not_selected+=("$name")
        continue
    fi

    printf '\n=== %s\n' "$name"
    if ( cd "$ROOT" && bash -o pipefail -c "$command" ); then
        printf 'PASS %s\n' "$name"
        passed+=("$name")
    else
        printf 'FAIL %s\n' "$name"
        failed+=("$name")
    fi
done

# ── registry summary ────────────────────────────────────────────────────────
# Contract order: registry execution → registry summary → lane phase → lane
# summary → combined exit. Printing this before the lane phase means a lane
# table parse error (exit 2) cannot swallow the coverage report.
ran=$(( ${#passed[@]} + ${#failed[@]} ))
total=${#GATES[@]}

if [[ -n "$ONLY" ]]; then
    printf '\n──── gates-local summary (PARTIAL — GATES_ONLY set) ────\n'
else
    printf '\n──────── gates-local summary ────────\n'
fi
printf 'passed:  %d\n' "${#passed[@]}"
printf 'failed:  %d%s\n' "${#failed[@]}" "${failed:+ — ${failed[*]}}"
printf 'skipped: %d%s\n' "${#skipped[@]}" "${skipped:+ — ${skipped[*]}}"
if [[ -n "$ONLY" ]]; then
    printf 'not selected: %d%s\n' "${#not_selected[@]}" "${not_selected:+ — ${not_selected[*]}}"
fi
printf 'ran:     %d of %d\n' "$ran" "$total"

# ── component lanes (AGENTS.md `## COMPONENT LANES`) ─────────────────────────
# The registry sweep above is repo-wide; this phase runs the integration lane
# of every workspace crate the tree actually touches. Fail-closed by design:
# an unparseable table, an unknown mode, a NUL byte, a whitespace-shifted crate
# key, invalid cargo metadata, or a missing merge base is exit 2 — never a
# silent "nothing touched".
#
# Selector precedence, highest first: GATES_LANES=0, GATES_SKIP=lanes,
# GATES_ONLY exclusion, enabled. A disabled phase bypasses parsing and detection
# entirely: no table read, no cargo metadata, no git diff — a malformed table
# cannot fail a sweep that never asked for lanes.
LANES_ENV="${GATES_LANES:-1}"
lane_phase=on

if [[ "$LANES_ENV" == "0" ]]; then
    echo 'component lanes: DISABLED (GATES_LANES=0)'
    lane_phase=off
elif [[ " $SKIP " == *" lanes "* ]]; then
    echo 'component lanes: DISABLED (GATES_SKIP=lanes)'
    lane_phase=off
elif [[ -n "$ONLY" && " $ONLY " != *" lanes "* ]]; then
    echo 'component lanes: not selected (GATES_ONLY)'
    lane_phase=off
fi

lane_rows=()
declare -A lane_crates=()
lane_passed=0
lane_failed=0
lane_failed_list=()
lane_skipped=()      # deduped labels for the summary line
lane_skipped_rows=0  # counted skips: one per skipped ROW (see below)
no_lane=()
no_lane_blocking=0

if [[ "$lane_phase" == on ]]; then
    lane_table="$("$PY_BIN" - "$AGENTS" <<'PY'
import os
import re
import sys

path = sys.argv[1]
name = os.path.basename(path)
marker = "## COMPONENT LANES"

# One scanner owns fence recognition, heading discovery and table extraction so
# they cannot disagree: a four-backtick doc example that happens to contain the
# marker is example text, never the section.
OPENER = re.compile(r"^( {0,3})(`{3,}|~{3,})(.*)$")
CLOSER = re.compile(r"^( {0,3})(`{3,}|~{3,})[ \t]*$")
HEADING = re.compile(r"^ {0,3}(#{1,6})(?:[ \t]+|$)")
LANE_HEADING = re.compile(r"## COMPONENT LANES\s*")
BAD_FIELD = re.compile(r"[\s\x00-\x1f\x7f]")

# Physical Markdown lines only: LF is the sole line separator. `str.splitlines()`
# also breaks on U+2028, U+2029, U+0085, VT and FF, which would turn one table
# row into two rows and rewrite the command bytes bash executes. `newline=""`
# disables universal-newline translation for the same reason; stripping a single
# trailing CR is CRLF tolerance (the CR belongs to the line terminator), and is
# the only byte after the command that may disappear.
with open(path, encoding="utf-8", newline="") as handle:
    lines = handle.read().split("\n")
lines = [line[:-1] if line.endswith("\r") else line for line in lines]


def opener(line):
    """Return (fence character, run length) when `line` opens a fence."""
    match = OPENER.match(line)
    if not match:
        return None
    fence = match.group(2)
    if fence[0] == "`" and "`" in match.group(3):
        return None
    return (fence[0], len(fence))


def closes(line, char, length):
    """Same character, run >= the opener's, and nothing but whitespace after."""
    match = CLOSER.match(line)
    if not match:
        return False
    fence = match.group(2)
    return fence[0] == char and len(fence) >= length


state = "search"  # search -> heading -> table
fence = None
fence_lineno = None
section_lineno = None
table_lineno = None
content = []
closed = False

for i, line in enumerate(lines):
    lineno = i + 1
    if fence is not None:
        if closes(line, fence[0], fence[1]):
            if state == "table":
                closed = True
                break
            fence = None
            fence_lineno = None
        elif state == "table":
            content.append((lineno, line))
        continue

    opening = opener(line)
    if opening is not None:
        fence = opening
        fence_lineno = lineno
        if state == "heading":
            state = "table"
            table_lineno = lineno
        continue

    if state == "search":
        # Complete-line heading, and only OUTSIDE a fence.
        if LANE_HEADING.fullmatch(line):
            state = "heading"
            section_lineno = lineno
        continue

    # state == "heading": an outside-fence heading of level <= 2 ends the
    # section, so a fence in a LATER section is never the lane table.
    heading = HEADING.match(line)
    if heading and len(heading.group(1)) <= 2:
        break

if state == "search":
    if fence is not None:
        # A doc example (typically four backticks) opened before the section and
        # never closed: name its opening line before the missing-section
        # message, so the operator can see the real cause of the exit 2.
        sys.stderr.write(
            "gates-local: lanes: unterminated fence at line %d in %s\n"
            % (fence_lineno, name)
        )
    sys.stderr.write("gates-local: lanes: no %s section in %s\n" % (marker, name))
    sys.exit(2)
if state == "heading":
    sys.stderr.write(
        "gates-local: lanes: no fenced table under %s (section at line %d) in %s\n"
        % (marker, section_lineno, name)
    )
    sys.exit(2)
if not closed:
    sys.stderr.write(
        "gates-local: lanes: unterminated fence at line %d under %s in %s\n"
        % (fence_lineno, marker, name)
    )
    sys.exit(2)

rows = []
for lineno, raw in content:
    if "\x00" in raw:
        # Bash command substitution silently drops NUL bytes (`tr\0ue` becomes
        # `true`), so a NUL must be rejected before any transport.
        sys.stderr.write(
            "gates-local: lanes: NUL byte at line %d in %s %s\n" % (lineno, name, marker)
        )
        sys.exit(2)
    # Table syntax tolerates ASCII space/tab padding only, and only around the
    # crate field, the two `|` delimiters and the mode field. The command field
    # is trimmed of its leading ASCII padding and then carried BYTE-IDENTICALLY:
    # `.strip()` also erases U+00A0 and U+2028, silently rewriting an unrunnable
    # command (bash exit 127) into a green one.
    line = raw.lstrip(" \t")
    if not line or line.startswith("#"):
        continue
    fields = line.split("|", 2)
    crate = fields[0].strip(" \t")
    mode = fields[1].strip(" \t") if len(fields) > 1 else ""
    command = fields[2].lstrip(" \t") if len(fields) > 2 else ""
    if len(fields) < 3 or not crate or not command or mode not in ("local", "docker"):
        sys.stderr.write(
            "gates-local: lanes: bad row %d in %s %s: %s\n" % (lineno, name, marker, raw)
        )
        sys.exit(2)
    # Crate and mode travel as TSV fields; whitespace or control characters in
    # them would shift those fields (`false` becoming `true`). Commands keep
    # tabs, pipes, quotes and backslashes.
    if BAD_FIELD.search(crate):
        sys.stderr.write(
            "gates-local: lanes: bad crate at line %d in %s %s: %r\n"
            % (lineno, name, marker, crate)
        )
        sys.exit(2)
    if BAD_FIELD.search(mode):
        sys.stderr.write(
            "gates-local: lanes: bad mode at line %d in %s %s: %r\n"
            % (lineno, name, marker, mode)
        )
        sys.exit(2)
    rows.append((crate, mode, command))

if not rows:
    sys.stderr.write(
        "gates-local: lanes: no rows in %s %s (fenced table at line %d)\n"
        % (name, marker, table_lineno)
    )
    sys.exit(2)

for crate, mode, command in rows:
    print("%s\t%s\t%s" % (crate, mode, command))
PY
)" || exit 2

    # Read each emitted row WHOLE (`IFS=`, no field splitting): `read -r crate
    # mode command` with `IFS=$'\t'` trims IFS whitespace from the line, which
    # would erase a trailing tab from the command — a second silent rewrite of
    # the bytes bash executes. Splitting is done below with parameter expansion,
    # which is byte-exact.
    while IFS= read -r row; do
        [[ -n "$row" ]] || continue
        crate="${row%%$'\t'*}"
        lane_rows+=("$row")
        lane_crates["$crate"]=1
    done <<< "$lane_table"

    base="$(git -C "$ROOT" merge-base HEAD main 2>/dev/null)" || base=""
    if [[ -z "$base" ]]; then
        echo "gates-local: cannot compute merge base with main" >&2
        exit 2
    fi

    if ! meta_json="$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps)"; then
        echo "gates-local: cargo metadata failed — cannot map changed paths to crates" >&2
        exit 2
    fi

    # Scratch dir for the NUL-delimited git output: $TMPDIR when set, else
    # /home/shared/tmp when it exists, else mktemp's own default. CI has neither
    # TMPDIR nor /home/shared/tmp; /tmp is never named explicitly.
    lane_tmp=""
    if [[ -n "${TMPDIR:-}" && -d "${TMPDIR}" ]]; then
        lane_tmp="$(mktemp -d "${TMPDIR%/}/gates-local-lanes.XXXXXX" 2>/dev/null)" || lane_tmp=""
    fi
    if [[ -z "$lane_tmp" && -d /home/shared/tmp ]]; then
        lane_tmp="$(mktemp -d /home/shared/tmp/gates-local-lanes.XXXXXX 2>/dev/null)" || lane_tmp=""
    fi
    if [[ -z "$lane_tmp" ]]; then
        lane_tmp="$(mktemp -d 2>/dev/null)" || lane_tmp=""
    fi
    if [[ -z "$lane_tmp" ]]; then
        echo "gates-local: cannot create a scratch dir for path detection" >&2
        exit 2
    fi

    # Changed paths: committed-vs-main (renames surface both sides) UNION
    # untracked files, so work in progress counts before it is committed.
    # Both producers are staged to scratch files and their exit status checked:
    # inside process substitution a failing `git diff` would lose its rc and
    # report zero touched crates.
    if ! git -C "$ROOT" diff -z --no-renames --name-only "$base" -- >"$lane_tmp/diff.nul"; then
        echo "gates-local: git diff failed — cannot enumerate changed paths" >&2
        exit 2
    fi
    if ! git -C "$ROOT" ls-files -z --others --exclude-standard >"$lane_tmp/untracked.nul"; then
        echo "gates-local: git ls-files failed — cannot enumerate untracked paths" >&2
        exit 2
    fi
    if ! cat "$lane_tmp/diff.nul" "$lane_tmp/untracked.nul" >"$lane_tmp/paths.nul" ||
        ! printf '%s' "$meta_json" >"$lane_tmp/meta.json"; then
        echo "gates-local: cannot stage scratch path/metadata files" >&2
        exit 2
    fi

    touched_map="$("$PY_BIN" - "$lane_tmp/meta.json" "$lane_tmp/paths.nul" "$lane_tmp" <<'PY'
import json
import os
import stat
import sys

meta_path, paths_path, scratch_path = sys.argv[1], sys.argv[2], sys.argv[3]

with open(paths_path, "rb") as handle:
    paths = handle.read().decode("utf-8", "surrogateescape").split("\0")


def invalid(detail):
    sys.stderr.write("gates-local: cargo metadata is structurally invalid: %s\n" % detail)
    sys.exit(2)


def has_test_sources(manifest_dir):
    """True when <manifest_dir>/tests/ holds any *.rs file, recursively.

    Cargo metadata only reports auto-discovered or explicit `[[test]]` targets,
    so a package with `autotests = false` (its tests/*.rs still run in CI) would
    look test-free. Only the package's OWN tests directory counts: a nested
    member crate's tests/ dir is not under it. An unreadable tests dir is a
    loud exit 2 — never a silent "no integration surface".
    """
    tests_dir = os.path.join(manifest_dir, "tests")
    try:
        info = os.stat(tests_dir)
    except FileNotFoundError:
        return False
    except OSError as error:
        sys.stderr.write("gates-local: cannot stat %s: %s\n" % (tests_dir, error))
        sys.exit(2)
    if not stat.S_ISDIR(info.st_mode):
        return False

    def unreadable(error):
        sys.stderr.write(
            "gates-local: cannot read %s: %s\n"
            % (getattr(error, "filename", tests_dir), error)
        )
        sys.exit(2)

    for _dirpath, _dirnames, filenames in os.walk(tests_dir, onerror=unreadable):
        for filename in filenames:
            if filename.endswith(".rs"):
                return True
    return False


try:
    with open(meta_path, encoding="utf-8") as handle:
        meta = json.load(handle)
except ValueError:
    sys.stderr.write("gates-local: cargo metadata returned invalid JSON\n")
    sys.exit(2)

# Fail closed on structurally empty metadata. Treating a missing workspace as
# "nothing touched" would turn a broken cargo invocation into a green sweep.
if not isinstance(meta, dict):
    invalid("top level is not an object")
root = meta.get("workspace_root")
if not isinstance(root, str) or not root:
    invalid("workspace_root must be a non-empty string")
declared = meta.get("packages")
if not isinstance(declared, list):
    invalid("packages must be a list")

packages = []
for index, pkg in enumerate(declared):
    if not isinstance(pkg, dict):
        invalid("packages[%d] must be an object" % index)
    pkg_name = pkg.get("name")
    if not isinstance(pkg_name, str) or not pkg_name:
        invalid("packages[%d].name must be a non-empty string" % index)
    manifest = pkg.get("manifest_path")
    if not isinstance(manifest, str) or not manifest:
        invalid("packages[%d].manifest_path must be a non-empty string" % index)
    targets = pkg.get("targets")
    if not isinstance(targets, list):
        invalid("packages[%d].targets must be a list" % index)
    has_tests = False
    for t_index, target in enumerate(targets):
        if not isinstance(target, dict):
            invalid("packages[%d].targets[%d] must be an object" % (index, t_index))
        kinds = target.get("kind")
        if not isinstance(kinds, list) or not all(isinstance(kind, str) for kind in kinds):
            invalid(
                "packages[%d].targets[%d].kind must be a list of strings" % (index, t_index)
            )
        if "test" in kinds:
            has_tests = True
    rel = os.path.relpath(os.path.dirname(manifest), root).replace(os.sep, "/")
    # Root-level package deliberately unmapped: every root tooling edit would
    # otherwise hit it. The root Cargo.toml is a virtual workspace today; if a
    # root [package] is ever added, its lane is NOT seen here — add one.
    if rel == "." or rel.startswith("../"):
        continue
    if not has_tests:
        # Metadata is the narrow leg; the documented contract also counts the
        # package's own tests/**/*.rs files whether cargo auto-discovers them
        # or not (e.g. `autotests = false`).
        has_tests = has_test_sources(os.path.dirname(manifest))
    packages.append((rel, pkg_name, has_tests))

# The scratch dir can land inside a package ($TMPDIR pointing into the tree):
# its own files must never count as a touch, or the sweep discovers itself and
# runs the lane of whatever package contains it.
scratch_rel = os.path.relpath(os.path.realpath(scratch_path), os.path.realpath(root))
scratch_rel = scratch_rel.replace(os.sep, "/")
if scratch_rel in (".", "..") or scratch_rel.startswith("../"):
    scratch_rel = ""

# Longest manifest dir first, so the first match below is the most specific
# package; matching `dir + "/"` keeps `crates/foo` from swallowing
# `crates/foo-extra/x.rs`.
packages.sort(key=lambda entry: len(entry[0]), reverse=True)

touched = {}
for raw in paths:
    if not raw:
        continue
    path = os.path.normpath(raw.replace(os.sep, "/"))
    if scratch_rel and (path == scratch_rel or path.startswith(scratch_rel + "/")):
        continue
    for rel, crate, has_tests in packages:
        if path == rel or path.startswith(rel + "/"):
            touched[crate] = touched.get(crate, False) or has_tests
            break

for crate in sorted(touched):
    print("%s\t%d" % (crate, 1 if touched[crate] else 0))
PY
)" || exit 2

    touched_crates=()
    declare -A touched_surface=()
    while IFS=$'\t' read -r crate surface; do
        [[ -n "$crate" ]] || continue
        touched_crates+=("$crate")
        touched_surface["$crate"]="$surface"
    done <<< "$touched_map"

    declare -A skip_printed=()
    # Display labels stay deduped (a crate with many skipped rows prints once),
    # but `lanes skipped` counts rows: two docker rows skipped is 2.
    declare -A lane_skip_label=()

    for row in "${lane_rows[@]}"; do
        crate="${row%%$'\t'*}"
        rest="${row#*$'\t'}"
        mode="${rest%%$'\t'*}"
        command="${rest#*$'\t'}"

        # A lane runs only for a crate the tree touches; untouched rows are
        # not candidates (a tooling-only sweep stays trivially green).
        [[ -n "${touched_surface[$crate]:-}" ]] || continue

        if [[ " $SKIP " == *" $crate "* ]]; then
            lane_skipped_rows=$(( lane_skipped_rows + 1 ))
            if [[ -z "${skip_printed[$crate]:-}" ]]; then
                echo "component lanes: SKIP $crate (GATES_SKIP)"
                skip_printed["$crate"]=1
            fi
            key="$crate (GATES_SKIP)"
            if [[ -z "${lane_skip_label[$key]:-}" ]]; then
                lane_skipped+=("$key")
                lane_skip_label["$key"]=1
            fi
            continue
        fi
        if [[ "$mode" == "docker" && "$LANES_ENV" != "all" ]]; then
            printf 'SKIP lane %s :: %s — docker opt-in (GATES_LANES=all; CI owns)\n' \
                "$crate" "$command"
            lane_skipped_rows=$(( lane_skipped_rows + 1 ))
            key="$crate (docker opt-in)"
            if [[ -z "${lane_skip_label[$key]:-}" ]]; then
                lane_skipped+=("$key")
                lane_skip_label["$key"]=1
            fi
            continue
        fi

        printf '\n=== lane %s :: %s\n' "$crate" "$command"
        if ( cd "$ROOT" && bash -o pipefail -c "$command" ); then
            printf 'PASS lane %s :: %s\n' "$crate" "$command"
            lane_passed=$(( lane_passed + 1 ))
        else
            printf 'FAIL lane %s :: %s\n' "$crate" "$command"
            lane_failed_list+=("$command")
            lane_failed=$(( lane_failed + 1 ))
        fi
    done

    # Fail-safe: a touched crate with no declared lane is loud. It blocks only
    # when the crate has an integration surface: a `*.rs` file under its OWN
    # `tests/` directory (auto-discovered or not, e.g. `autotests = false`), or
    # a cargo-metadata target whose kind includes `test` (explicit `[[test]]`
    # targets outside `tests/`).
    for crate in "${touched_crates[@]}"; do
        [[ -n "${lane_crates[$crate]:-}" ]] && continue
        if [[ " $SKIP " == *" $crate "* ]]; then
            # No declared row exists for this crate, so it counts as a single
            # crate-level skip; the message stays crate-level.
            lane_skipped_rows=$(( lane_skipped_rows + 1 ))
            if [[ -z "${skip_printed[$crate]:-}" ]]; then
                echo "component lanes: SKIP $crate (GATES_SKIP)"
                skip_printed["$crate"]=1
            fi
            key="$crate (GATES_SKIP)"
            if [[ -z "${lane_skip_label[$key]:-}" ]]; then
                lane_skipped+=("$key")
                lane_skip_label["$key"]=1
            fi
            continue
        fi
        echo "!! NO LANE for $crate — add one to AGENTS.md ## COMPONENT LANES"
        no_lane+=("$crate")
        if [[ "${touched_surface[$crate]:-0}" == "1" ]]; then
            echo "   blocking: crate has integration surface"
            no_lane_blocking=$(( no_lane_blocking + 1 ))
        else
            echo "   not blocking: no integration surface"
        fi
    done
fi

if [[ "$lane_phase" == on ]]; then
    printf '\n──── component lanes ────\n'
    printf 'lanes passed:  %d\n' "$lane_passed"
    printf 'lanes failed:  %d%s\n' "$lane_failed" "${lane_failed_list:+ — ${lane_failed_list[*]}}"
    printf 'lanes skipped: %d%s\n' "$lane_skipped_rows" "${lane_skipped:+ — ${lane_skipped[*]}}"
    if [[ ${#no_lane[@]} -eq 0 ]]; then
        printf 'no-lane:       0\n'
    else
        printf 'no-lane:       %d — %s (blocking: %d)\n' "${#no_lane[@]}" "${no_lane[*]}" "$no_lane_blocking"
    fi
fi

[[ ${#failed[@]} -eq 0 && "$lane_failed" -eq 0 && "$no_lane_blocking" -eq 0 ]]
