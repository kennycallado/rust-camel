# CXF Bridge

A Quarkus-based gRPC bridge for Apache CXF SOAP services with WS-Security support.

## WS-Security Configuration

The CXF bridge supports WS-Security (signing, encryption, verification, and decryption) via WSS4J. Security is enabled automatically when a keystore path is configured.

### Properties

| Property                                  | Default           | Description                                                                                                                                             |
| ----------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cxf.keystore.path`                       | _(none)_          | Path to the JKS keystore file. When set, WS-Security processing is enabled.                                                                             |
| `cxf.keystore.password`                   | _(none)_          | Password to the keystore.                                                                                                                               |
| `cxf.truststore.path`                     | _(none)_          | Path to the truststore for signature verification. Falls back to keystore for the manual consumer path; the producer in-interceptor requires this.                                                                   |
| `cxf.truststore.password`                 | _(none)_          | Password to the truststore.                                                                                                                             |
| `cxf.sig.username`                        | `clientkey`       | Alias of the key entry in the keystore used for signing.                                                                                                |
| `cxf.sig.password`                        | _(none)_          | Password for the private key entry.                                                                                                                     |
| `cxf.enc.username`                        | `serverkey`       | Alias used for encryption (recipient's public key).                                                                                                     |
| `cxf.security.actions.out`                | _(empty)_         | Space-separated WSS4J action tokens for outbound messages (e.g. `Signature`, `Signature Encrypt`, `Signature Timestamp`; see Timestamp behavior for constraints and build-time rejections).                                                      |
| `cxf.security.actions.in`                 | _(empty)_         | Space-separated WSS4J action tokens for inbound messages.                                                                                               |
| `CXF_PROFILE_<N>_SIGNATURE_ALGORITHM` (`cxf.security.signature.algorithm`) | _(WSS4J default)_ | Signature algorithm URI (e.g. `http://www.w3.org/2000/09/xmldsig#rsa-sha1` for legacy, `http://www.w3.org/2001/04/xmldsig-more#rsa-sha256` for modern). Applied on BOTH producer requests and consumer signed responses. |
| `CXF_PROFILE_<N>_SIGNATURE_DIGEST_ALGORITHM` (`cxf.security.signature.digest.algorithm`) | _(WSS4J default)_ | Digest algorithm URI (e.g. `http://www.w3.org/2000/09/xmldsig#sha1` or `http://www.w3.org/2001/04/xmlenc#sha256`). Applied on BOTH paths. |
| `CXF_PROFILE_<N>_SIGNATURE_C14N_ALGORITHM` (`cxf.security.signature.c14n.algorithm`) | _(WSS4J default)_ | Canonicalization algorithm URI (e.g. `http://www.w3.org/2001/10/xml-exc-c14n#`). Applied on BOTH paths. |
| `CXF_PROFILE_<N>_SIGNATURE_PARTS` (`cxf.security.signature.parts`) | _(WSS4J default)_ | Signed-parts definition, applied on the PRODUCER path only — see [Signature knobs](#signature-knobs) for grammar and the consumer restriction. |
| `cxf.security.username`                   | _(none)_          | Username for basic authentication.                                                                                                                      |
| `cxf.security.password`                   | _(none)_          | Password for basic authentication.                                                                                                                      |

### Example — minimal signing configuration

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
```

### Example — sign + encrypt outbound, verify + decrypt inbound

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.truststore.path=/etc/camel/truststore.jks
cxf.truststore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
cxf.security.actions.out=Signature Encrypt
cxf.security.actions.in=Signature Encrypt
```

### Example — legacy BUS interop (CAT112 / Baleares 112, rsa-sha1 + sha1)

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
cxf.security.actions.out=Signature
cxf.security.actions.in=Signature
cxf.security.signature.algorithm=http://www.w3.org/2000/09/xmldsig#rsa-sha1
cxf.security.signature.digest.algorithm=http://www.w3.org/2000/09/xmldsig#sha1
```

### Inbound security

The `cxf.security.actions.in` property gates inbound processing: only the listed
actions run on received messages. Both inbound paths are functional. `Signature`
verification checks the received signature against the truststore
(`cxf.truststore.path`, which the in-interceptor requires; the keystore
fallback applies to the manual consumer path only); `Encrypt` decryption
unwraps the message with the keystore private key, taking the keystore password
via the WSS4J password callback. See the `cxf.security.actions.in`,
`cxf.truststore.path`, and `cxf.truststore.password` rows in the Properties
table above.

### Signature knobs

The four signature knobs are configured as the sidecar environment
variables `CXF_PROFILE_<NAME>_SIGNATURE_ALGORITHM`,
`SIGNATURE_DIGEST_ALGORITHM`, `SIGNATURE_C14N_ALGORITHM`, and
`SIGNATURE_PARTS` (the Rust component options `signature_algorithm`,
`signature_digest_algorithm`, `signature_c14n_algorithm`,
`signature_parts` generate them). The `cxf.security.signature.*`
property keys below map to these; in native-image the property form is
inert — the env form is the operative surface. The knobs take effect at
startup and fail loud:

- **Both paths**: `algorithm`, `digest.algorithm`, and `c14n.algorithm`
  apply to producer requests (the outbound `Dispatch` interceptor) AND to
  consumer signed responses (`WssSecurityProcessor`).
- **Producer only**: `signature.parts`. Consumer coverage is pinned to
  the SOAP Body plus the `wsu:Timestamp` — that pair is the
  replay-defense invariant, so narrowing it by config is refused: a
  profile with `signature.parts` used by a consumer endpoint fails Rust
  endpoint construction, and the Java consumer path refuses the profile
  at runtime (the sidecar env form is `CXF_PROFILE_<NAME>_SIGNATURE_PARTS`;
  the rejection diagnostic names `SIGNATURE_PARTS` and the
  `Body+Timestamp` invariant).
- **Startup validation**: algorithm values must be absolute URIs (any
  scheme). A knob set while the outbound actions lack `Signature`, or
  without a signing keystore, aborts profile construction naming the
  offending setting. WSS4J remains the authority for whether an
  algorithm is supported — a well-formed but unsupported URI fails
  loudly at first invoke.
- **Parts grammar**: `;`-separated segments, each either a bare
  `localName` (e.g. `Body`) or `{modifier}{namespace}localName` where
  the modifier is empty or exactly `Element`/`Content`, the namespace
  may be empty, and the local name is non-empty — for example
  `{Content}{http://schemas.xmlsoap.org/soap/envelope/}Body`.
- Verified URI examples: signature
  `http://www.w3.org/2001/04/xmldsig-more#rsa-sha256`, digest
  `http://www.w3.org/2001/04/xmlenc#sha256` (sha-384 exists only as
  `http://www.w3.org/2001/04/xmldsig-more#sha384`), canonicalization
  `http://www.w3.org/2001/10/xml-exc-c14n#`.

### Timestamp behavior

When the outbound actions include `Timestamp`, the producer emits a
`wsu:Timestamp` element. The Timestamp action runs before Signature, so
signature part resolution sees the materialized element.

When the outbound actions include `Timestamp` and `Signature`, and
`SIGNATURE_PARTS` is not set, the producer signs the SOAP Body and the
timestamp together: it sets `SIGNATURE_PARTS` to
`Body;{}{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd}Timestamp`.
A rewritten or stripped timestamp then fails signature verification.

When `SIGNATURE_PARTS` is set, the producer applies its value verbatim. The
timestamp is then covered only if the value names it. Covering the timestamp
is the operator's responsibility in that case.

The manual consumer's inbound processing enforces the same rule: when the required inbound actions
include both `Timestamp` and `Signature`, the verified signature must cover
the timestamp.

Two rules fail loud at profile construction. They read the configured
outbound actions; blank or unset actions are exempt.

- Timestamp requires Signature. Configured outbound actions that contain
  `Timestamp` but not `Signature` are rejected: a timestamp emitted outside
  the signature is not tamper-evident.
- Signing material is required. Configured outbound actions that contain
  `Signature`, `Encrypt`, or `Timestamp` require a keystore. The composition
  rule is checked first; the keystore check runs only for a valid
  composition.

### Startup logging

When the bridge starts, it logs one of:

```
WS-Security: ENABLED (signing/verification active)
```

or

```
WS-Security: DISABLED (no keystore configured)
```

## Listener

Consumer addresses accept plain `http://` only. The bridge does not expose a TLS listener yet, so any other scheme (`https://`, and anything else) fails loud:

- At Rust route-build time: `CxfPoolConfig.bind_address` validation rejects the URI before the bridge process starts.
- At bridge startup: the endpoint publisher rejects the address again before any socket binds.

The default listener address is `http://0.0.0.0:9000/cxf`.

## Dispatch cache (producer)

Producer-side SOAP clients are cached per (WSDL, address, service, port,
security profile, operation, request timeout). Every entry's request
context — endpoint address, timeouts, SOAPAction — is written once at
creation and never mutated afterwards, so concurrent invokes cannot
cross-contaminate. Cardinality grows per distinct operation/timeout pair
per endpoint tuple. **Caution (ADR-0032):** `operation` and the request
timeout can also be supplied per-exchange via the `CamelCxfOperation`
and `CamelCxfTimeoutMs` Exchange headers. A route that derives either
from untrusted request data lets an external caller mint one cached
client (a permanent allocation) per distinct value — bound cardinality
only when these dimensions come from route configuration, not from the
data plane. An eviction cap is tracked as rc-urkv.

## Environment Variables

| Variable              | Default              | Description                                                                                                                                               |
| --------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CXF_MAX_BODY_BYTES`  | `16777216` (16 MiB)  | Listener request-body cap in bytes. Oversized request bodies are rejected with HTTP 413.                                                                   |

A malformed, non-positive, or above-ceiling value aborts startup. The ceiling is 17 MiB. Operators must respect the ordering constraint: the cap stays at or below 17 MiB, and the Rust gRPC decode limit is 18 MiB. A body accepted by the listener is therefore always decodable on the Rust side.
