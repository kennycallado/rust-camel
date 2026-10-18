# Proposal: cxf-consumer-per-profile-dispatch

## Why

The CXF bridge keeps one consumer stream observer for the whole bridge
(`CxfServerManager.consumerRequestObserver`). Every `OpenConsumerStream` call
overwrites it. With two `cxf://` consumer routes in one process (for example
profiles `v11` and `v12`), the bridge sends every inbound SOAP request to the
last route that started, whatever profile the request addressed. The listener
already resolves the profile from the first path segment after `/cxf/` and
copies it into `ConsumerRequest.security_profile`, but dispatch ignores it.
Bd: rc-gsy1v. A 2026-10-12 answer to camel-112 said per-profile routing is the
intended contract; this change makes that statement true.

## What Changes

- The bridge keeps one consumer stream per profile. Dispatch selects the stream
  by the request profile.
- The Rust consumer announces its profile when it opens the stream, as gRPC
  call metadata. No proto change.
- The bridge rejects an open without a known profile. The Rust side rejects a
  second consumer for the same profile at route start.
- The consumer-availability 503 gate applies to the addressed profile only.
- Docs: correct `docs/src/components/ws-soap.md` (lines 260 and 385),
  `crates/components/camel-cxf/CONTEXT.md` (line 112) and the cxf-fault-status
  spec wording.

Excluded: producer behaviour, WS-Security, WSDL catalog, proto schema changes,
load balancing several consumers on one profile.

## Acceptance criteria

- Two consumer routes on two profiles in one process each receive only the
  requests addressed to their profile.
- A request for a profile with no connected consumer gets HTTP 503, even when
  another profile has one.
- A second consumer on the same profile fails at route start with a clear error;
  stopping and restarting a route does not trigger that error.
- A bridge that rejects a profile (`INVALID_ARGUMENT`) produces one error log
  naming the profile and no reconnect loop.
- Replacing or closing the stream of one profile does not touch pending
  requests or the stream of another profile.
- Java and Rust regression tests cover the above.
- The documents named above no longer claim a single bridge-wide consumer
  stream or unconditional multiplexing.

## Risk budget

Accepted: a new bridge and crate pair must ship together (versions are already
locked). A stale crate that does not send the profile is rejected with
`INVALID_ARGUMENT`; the consumer logs the error and stops instead of retrying.
Out of bounds: a default or key-less fallback slot (it would bring the bug
back); any change to pending-request ownership semantics.
