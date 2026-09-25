"""Tests for the future-run warmup policy documentation (benchwarmup).

Phase 2 of the benchwarmup change: CONTEXT.md must document the
trailing-window Protocol A warmup policy, and the sealed era-2 record
(`benchmarks/records/20260903T084658Z`) must never be modified or
republished. This test pins the record's exact SHA-256 hashes so any
change to the sealed files fails loudly.
"""
import hashlib
import pathlib
import unittest

HARNESS_DIR = pathlib.Path(__file__).resolve().parent
REPO_ROOT = HARNESS_DIR.parent.parent
CONTEXT_MD = HARNESS_DIR / "CONTEXT.md"
RUN_SH = HARNESS_DIR / "run.sh"
SEALED_DIR = REPO_ROOT / "benchmarks" / "records" / "20260903T084658Z"

# Pinned SHA-256 of the sealed era-2 record (2026-09-03). run.json and
# CAVEATS.md are evidence: any change is a republish and must fail this
# test. summary.md is a derived view (e_opus ruling 2026-09-25): the pin
# tracks the current renderer's output and is updated when the renderer
# legitimately changes — regeneration must always go through
# `summarize.py --check` green first.
SEALED_SHA256 = {
    "CAVEATS.md": (
        "b4cb35aab10f5b23bc7720062ced5e98b0029cb80e25a9bf173ecb5941e08044"
    ),
    "run.json": (
        "fe14eca6e69c55a5d5bd725772731e0b92f8e3bbcddd8924d1b8bd1b08342b84"
    ),
    "summary.md": (
        "ab1f7c5d4d51228432a54ff1ea5c027398037bfdf86b7a65a1d44102dca2682f"
    ),
}

# Policy phrases that must appear in the warmup methodology section (§2)
# of CONTEXT.md. Each guards one clause of the future-run protocol.
REQUIRED_PHRASES = (
    "trailing comparison-window",      # max_messages is a window, not a cap
    "wall-clock",                      # Protocol A collects until the deadline
    "MessageBoundUnconverged",         # retained only for compatibility
    "Protocol B",                      # unchanged
    "never modified; `summary.md` is a derived view",  # sealing boundary (e_opus 2026-09-25)
    "20260903T084658Z",                # the sealed record, by name
    "docs-investigation-strategy.md",  # live-defect link (§8)
)


def _methodology_section():
    """Return the §2 methodology section of CONTEXT.md."""
    text = CONTEXT_MD.read_text()
    start = text.index("## 2.")
    end = text.index("## 3.", start)
    return text[start:end]


class WarmupPolicyTest(unittest.TestCase):
    """Future-run warmup policy is documented and the record is sealed."""

    def test_protocol_a_policy_documented(self):
        """CONTEXT.md states the policy; sealed-record hashes are pinned."""
        section = _methodology_section()
        for phrase in REQUIRED_PHRASES:
            self.assertIn(
                phrase, section,
                f"warmup methodology must state: {phrase!r}",
            )

        # The sealed record dir must contain exactly the three pinned
        # files — any extra or missing entry fails the walk.
        actual = {p.name for p in SEALED_DIR.iterdir()}
        self.assertEqual(
            set(SEALED_SHA256), actual,
            "sealed record dir must contain exactly the three pinned files",
        )
        for name, expected in SEALED_SHA256.items():
            digest = hashlib.sha256(
                (SEALED_DIR / name).read_bytes()
            ).hexdigest()
            self.assertEqual(
                digest, expected,
                f"{name} changed — sealed record must never be modified",
            )

    def test_native_warmup_reason_statuses(self):
        """run.sh native failed-stability regex names all three reasons.

        bd rc-audm.7: run.sh writes the attempted status into
        m2-summary.json at measurement time only when the measure-a
        error is a warmup failed-stability. The grep regex must name
        every reason the Python classifier accepts (Task 2.1 step 5),
        so native shell emission and fallback classification stay in
        lockstep: MessageBoundUnconverged (historical Protocol A),
        TimeBoundUnconverged and InsufficientSamples (future
        benchwarmup trailing-window runs).
        """
        text = RUN_SH.read_text()
        self.assertIn(
            "warmup failed-stability: "
            "(MessageBoundUnconverged|TimeBoundUnconverged|InsufficientSamples)",
            text,
        )


if __name__ == "__main__":
    unittest.main()