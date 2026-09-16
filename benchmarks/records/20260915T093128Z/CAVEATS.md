# Editorial caveats

This file was added after the record was generated. It does not change
the measurement data. The sealed `run.json` and `summary.md` files
remain untouched.

## Split invocation provenance

This record was produced by TWO harness invocations on the same
commit (`918a8398`), same container digest, same era:

- `m1+m2` — run `20260915T093128Z` (2026-09-15)
- `m3+m4` — run `20260916T061034Z` (2026-09-16)

The `run-all` default metric set was `m1+m2` (since fixed: the
RUNBOOK now prescribes `--metric=m1+m2+m3+m4`; bd rc-awyoj). The
m3/m4 arm also died twice on host-level causes (port 8080 held by a
stale wrapper; /home ENOSPC mid-run — scratch is now redirectable,
bd rc-awyoj) before completing cleanly. `meta.invocations` carries
both timestamps.

## rust-camel-lib per-request fixture asymmetry (`rc-h42s6`)

The `rust-camel-lib` http-server fixture runs `log(...)` +
`process(id++)` + `set_body` per request (smoke-trace steps restored
by rc-am22), while the `rust-camel-cli` route is bare `set_body`.
Cross-contender m2 protocol-A and m3 comparisons between these two
cells are NOT like-for-like: the lib cell pays one stdout write per
request. The lib-vs-cli gap in m3 (66k vs 83k msgs/s) must not be
read as a product claim. The cli +23.3% delta vs era-2 is also
pending fixture-shape verification at `9e8f36f`.

**Addendum 2026-09-16 (post-seal, annotation only):** the
fixture-fairness audit landed at `6a375fbb`
(`benchmarks/audits/fixture-fairness-2026-09.md`, 53/53 cells
verified) discharged the pending verification: at `9e8f36f` BOTH
rust http-server cells were symmetric-bare, so the cli +23.3%
vs era-2 delta is fixture-valid. The divergence from era-2 shape
was on the LIB side (rc-am22 restore), not the cli. No
measurement value changed; the lib-vs-cli cross-cell caveat above
stands until the era-3 re-run under the blessed minimal-bare
shape (e_opus ruling D1, 2026-09-16).

## http-server rust-camel-cli M1 RSS: wrapper-launched (`n/a`)

The cli http-server cell launches through a wrapper script; GNU
`time -v` measured the wrapper, so RSS is `n/a` and startup includes
wrapper overhead. The cold-start regression finding below stands on
the DIRECT-launched cells (startup-minimal/t2-json/t2-realistic-eip:
27-31ms at era-2 → 102-108ms here).

## m2 failed rounds (axum-bare, camel-quarkus-dsl-native)

Rounds 2-3 of protocol A recorded `status=failed reason=measure-a-error`
for these two cells; medians were computed over the remaining rounds.
Same failure class as era-2's `attempted (unconverged)` cells. Raw
per-round data is in the run dir.

## Findings for readers (do not over-read)

- `rust-camel-cli` cold-start regressed 3-4x vs era-2 (rc-j329x);
  m3 throughput improved +23.3%, m2 latency improved ~35% on tick
  cells — a throughput-for-boot-time trade concentrated in the jobs
  machinery landings.
- `xslt-bridge`: rust-camel lib+cli p99 ~3.1ms vs JVM 0.79ms /
  node 0.76ms in BOTH eras — suspected per-tick XSLT template
  recompile; unfixed, now visible across eras.
- `node-native` m4 +26MB per sustained window — same signature as
  era-2 (+26.9MB), heap churn under load.
- `axum-bare` is the new reference contender (rc-u034, absent in
  era-2): ratios use it as denominator; era-2 rows for it do not
  exist.
