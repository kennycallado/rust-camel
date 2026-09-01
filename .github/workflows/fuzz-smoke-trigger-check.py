#!/usr/bin/env python3
"""Trigger-semantics guard for fuzz-smoke.yml.

Implements the `trigger_semantics_exact` local test from OpenSpec change
`fuzz-smoke-ci` (Task 2): parses the workflow with pyyaml — where the
`on:` key loads as boolean True — and asserts the pinned trigger
contract. Exit 0 = contract holds; any violation prints a message and
exits non-zero.
"""

import sys

import yaml

WORKFLOW = ".github/workflows/fuzz-smoke.yml"

EXPECTED_PATHS = {
    "crates/camel-dsl/**",
    "fuzz/**",
    "scripts/xtask/**",
    ".github/workflows/fuzz-smoke.yml",
}


def main() -> int:
    with open(WORKFLOW, encoding="utf-8") as fh:
        doc = yaml.safe_load(fh)

    # pyyaml parses the `on:` key as boolean True — resolve both shapes.
    triggers = doc.get(True) or doc.get("on") or {}

    failures = []

    def check(label, ok, detail=""):
        if ok:
            print(f"ok: {label}")
        else:
            failures.append(label)
            print(f"FAIL: {label}" + (f" ({detail})" if detail else ""))

    check("'push' is not a trigger", "push" not in triggers)

    pull_request = triggers.get("pull_request") or {}
    check(
        "pull_request.branches == ['main']",
        pull_request.get("branches") == ["main"],
        f"got {pull_request.get('branches')!r}",
    )
    paths = pull_request.get("paths") or []
    check(
        "pull_request.paths is exactly the pinned 4-entry set",
        len(paths) == 4 and set(paths) == EXPECTED_PATHS,
        f"got {paths!r}",
    )

    dispatch = triggers.get("workflow_dispatch")
    check("workflow_dispatch present", isinstance(dispatch, dict))
    default_time = ((dispatch or {}).get("inputs") or {}).get("time", {}).get("default")
    check(
        "workflow_dispatch.inputs.time.default == 60",
        default_time == 60,
        f"got {default_time!r}",
    )

    job = (doc.get("jobs") or {}).get("fuzz-smoke") or {}
    check(
        "jobs.fuzz-smoke.continue-on-error is True",
        job.get("continue-on-error") is True,
        f"got {job.get('continue-on-error')!r}",
    )
    check(
        "jobs.fuzz-smoke.timeout-minutes == 20",
        job.get("timeout-minutes") == 20,
        f"got {job.get('timeout-minutes')!r}",
    )

    if failures:
        print(f"trigger_semantics_exact: {len(failures)} assertion(s) failed")
        return 1
    print("trigger_semantics_exact: all assertions passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
