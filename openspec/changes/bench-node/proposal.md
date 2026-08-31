# Proposal: bench-node

## Why

The benchmark suite compares rust-camel against JVM Camel contenders across
7 active scenarios, but the most common real-world alternative to an
integration framework is not another JVM framework — it is hand-rolled
Node.js. Adding Node.js contenders serves an honest-comparison goal stated
by the owner (2026-08-30): Node will likely WIN raw req/s in `http-server`;
publishing that honest loss buys credibility for the scenarios where a
declarative integration route beats hand-rolled orchestration
(`split-aggregate`, `t2-realistic-eip`, XML bridges). The contender that
humiliates us on the easy scenario is the same one that loses on the hard
ones. No cherry-picking: if Node competes, it competes everywhere.

## What Changes

- Two new contenders, admitted together under a new **contender
  completeness rule** (a contender is only admitted if it implements every
  ACTIVE scenario; registering a subset is a hard error):
  - `node-native` — no web framework. Node stdlib for 5 scenarios; external
    libraries ONLY where the stdlib has no equivalent capability (XML:
    XSD validation, XSLT), mirroring how Java stdlib (JDK Xerces/JAXP)
    and Rust crates supply those capabilities to existing contenders.
  - `node-fastify` — same route logic behind the Fastify server layer.
- 14 new cells (2 contenders × 7 active scenarios): http-server,
  startup-minimal, t2-json, t2-realistic-eip, split-aggregate,
  xsd-validation-bridge, xslt-bridge.
- Runner image gains a digest-pinned Node runtime installed from the
  official `nodejs.org` tarball (SHA256-verified at build); `pin.sh`
  learns `NODE_VERSION` + `NODE_SHA256`.
- `harness/run.sh` gains the node contender family (build step for npm
  fixtures, launch commands, marker/latency/payload contract unchanged).
- `COVERAGE.md` gains a "Node contender axis" table (7 scenarios × 2
  contenders) with every cell `open-if` (no measurement claimed until a
  container-hosted run publishes it).
- `scenarios/README.md` contender recipe gains the completeness rule.
- Explicitly EXCLUDED: `multi-step` (inactive — zero wiring, no contender
  set), any new scenario (JMS/cache/redis remain ideas), npm web frameworks
  other than Fastify, and any measurement runs (human-invoked, bd rc-f4po).

Owner decision recorded for the future JMS scenario: Node JMS parity
requires activating a compatibility mode that is not always possible in
enterprise environments; this benchmark allows it by explicit decision.

## Acceptance criteria

- All 14 cells register, dry-run green on a host without Node, and real
  smoke on a Node host produces the SAME input digests as the existing
  contenders per scenario.
- Completeness rule enforced by the harness (a contender family missing a
  fixture for any active scenario fails the run with an explicit list).
- Zone contract intact (7 entries under `benchmarks/`); `benchmarks/README.md`
  diet gate stays green.
- No era-1 history, records, or schema touched.

## Risk budget

Acceptable: Node winning `http-server` by a wide margin (that is the
point); fastify npm supply chain confined to lockfile-committed fixture
deps. Out of bounds: changing measurement protocol, marker contract, or
record schema; adding scenarios; touching era-1 evidence.

Bd: rc-skzz (epic), run execution rc-f4po.
