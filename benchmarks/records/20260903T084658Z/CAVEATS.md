# Editorial caveats

This file was added after the record was sealed. It does not change the
measurement data. The sealed `run.json` and `summary.md` files remain
untouched.

## XSD validation fixture (`rc-audm.1`)

The Node XSD fixtures started a new worker for each validation. Each tick
therefore included WASM setup and schema parsing. Do not read their m2 values
as the cost of validation with a reusable validator.

## XSD memory results (`rc-audm.2`)

The `node-native` m4 increase includes heap churn from the per-tick XSD
workers. The Quarkus native RSS results need container-side verification.
That verification was not available during the audit.

## Realistic EIP ratio (`rc-audm.3`)

The m2 fixtures have step-for-step parity. Rust profiling confirmed real EIP
cost: 76 allocations and 20.6 KiB allocated per tick. However, the recorded
threefold Node-to-`rust-camel-lib` ratio is an environment artifact. Node ran
in a container, while `rust-camel-lib` ran natively. Host measurements did not
reproduce that ratio. Reverify cross-runtime protocol-B ratios under matching
container conditions.

## JSON language-step cost (`rc-audm.5`)

The `t2-json` m2 difference between `rust-camel-cli` and `rust-camel-lib` is
not pipe overhead. The route uses in-process markers. The difference measures
JavaScript, Rhai, and JSON work against a native Rust closure.

## XSLT comparison (`rc-audm.6`, `rc-dycfn`)

Do not compare the recorded `rust-camel-lib` XSLT m2 value with the later
manual `rust-camel-cli` value. Marker placement cannot explain the residual
difference. Run conditions remain the likely source, and the next canonical
run is the adjudicator.

## Protocol-A warmup window (`rc-audm.8`)

Protocol A checked the first 1,000 messages instead of a trailing time window.
This criterion can affect protocol-A m2 results and ratios. It directly caused
unconverged m2 status for `http-server/camel-standalone-dsl`,
`http-server/camel-standalone-yaml`, and `http-server/node-native`. A separate
protocol change must fix this defect.
