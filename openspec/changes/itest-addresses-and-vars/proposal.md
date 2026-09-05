# Proposal: itest-addresses-and-vars

## Why

Scenario documents cannot express the partner-direct pattern. A send
to a harness partner dials the declared URI literally, and harness
endpoints carry the `:0` router-key port, so the dial hits
`connect refused 127.0.0.1:0` (bd rc-gz2r, p2). The failed dial also
poisons the endpoint receive lane: the parked roundtrip receiver
shadows every later server-role arrival on that URI.

Data does not flow between actions. `receive` can extract into
scenario variables and `validate` can read them, but `send` consumes
its `to`, `body`, and `headers` raw (bd rc-a5de, p2). Server-generated
IDs and dynamic tokens make create-read-update-delete chains
inexpressible in YAML.

Partner scripting exists only as a Rust API. The CLI binds every
document partner `permissive(200)`, so a YAML author cannot script a
response that discriminates by method or path.

## What Changes

- One scenario variable namespace. Harness `bindVar` addresses and
  `extract` results live together, filled at boot before the first
  action, last writer wins.
- `${name}` interpolation in `send` and `receive` endpoint strings,
  `send` body string leaves, and `send` header values. `$${` escapes a
  literal. Substitution is raw. A variable unset at send time fails
  `scenario-var-unresolved`, exit 1, naming the variable.
- Harness-provisioned endpoint references in `send` and `receive`
  resolve to the partner bound address before any dial.
- A top-level `partners:` document section declares scripted
  responses per endpoint URI, bound before route boot. An absent
  section keeps the `permissive(200)` default.

Excluded: inbound port-0 listeners and the bound-address API for
consumer routes (bd rc-5yon, separate change; the grammar here is the
landing zone for it), non-HTTP transports (bd rc-xnob), `${env:}` in
scenario strings (boot layer only, deliberate), percent-encoding at
substitution.

## Acceptance criteria

- A YAML-only scenario sends to a harness partner at its bound
  address, scripts the partner response, and validates the
  roundtrip. No Rust API calls.
- A CRUD chain runs: POST extracts an ID, a later GET interpolates it
  into the path, and validation passes.
- An unset `${name}` at send time fails exit 1 naming the variable.
- A send that fails before the wire leaves the endpoint receive lane
  usable.

## Risk budget

Adapter receive-lane surgery (the generation guard) is the highest
risk: pure async bookkeeping with a replace-then-fail race.
Substitution is string-only. Grammar stays strict:
`deny_unknown_fields` everywhere, unknown keys are load errors.
