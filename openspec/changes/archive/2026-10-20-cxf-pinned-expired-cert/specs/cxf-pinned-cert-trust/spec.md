# cxf-pinned-cert-trust Delta

Capability: cxf security (consumer inbound WSS signature trust validation).

## ADDED Requirements

### Requirement: Opt-in acceptance of an expired exactly-pinned certificate

The bridge SHALL accept an inbound signature whose single presented certificate is expired IF AND ONLY IF the consuming profile sets `allow_expired_pinned_certificate` to true, exactly one certificate is presented, revocation is disabled, and the DER encoding of the presented certificate byte-equals an entry in the profile's keystore or truststore. Any other verification failure SHALL be rejected unchanged.

#### Scenario: Expired pinned cert accepted when opted in

- **GIVEN** a profile with `allow_expired_pinned_certificate = true` and an expired self-signed certificate pinned in its store
- **WHEN** an envelope signed with that expired certificate is verified
- **THEN** verification succeeds and the signer is populated

#### Scenario: E2E signed envelope reaches the route

- **GIVEN** the opted-in profile of the previous scenario
- **WHEN** a signed envelope produced with the expired certificate passes through `processInbound`
- **THEN** the envelope reaches the route with its signer populated

#### Scenario: Expired pinned cert rejected by default

- **GIVEN** a profile with `allow_expired_pinned_certificate` unset (default false) and the same pinned expired certificate
- **WHEN** an envelope signed with the expired certificate is verified
- **THEN** verification fails with the `invalidCert` WSS4J error and nothing reaches the route

#### Scenario: Expired cert rejected when not pinned

- **GIVEN** a profile with the opt-in on and a store that does NOT contain the presented certificate
- **WHEN** an envelope signed with a different key (same DN, different key) is verified
- **THEN** verification fails

### Requirement: The opt-in relaxes only expiry, never other validity gates

The opt-in SHALL NOT widen any gate other than certificate expiry. Not-yet-valid certificates, chains of more than one certificate, an issuer certificate pinned in place of the presented leaf, and revocation-enabled verification SHALL still be rejected when the opt-in is on.

#### Scenario: Not-yet-valid cert rejected

- **GIVEN** a profile with the opt-in on and a pinned certificate whose validity starts in the future
- **WHEN** an envelope signed with that certificate is verified
- **THEN** verification fails

#### Scenario: Chain with expired leaf rejected

- **GIVEN** a profile with the opt-in on
- **WHEN** an envelope presents a two-certificate chain whose leaf is expired
- **THEN** verification fails

#### Scenario: Pinned issuer does not authorize expired leaf

- **GIVEN** a profile with the opt-in on and only the issuer certificate pinned
- **WHEN** an envelope presents an expired leaf signed by that issuer
- **THEN** verification fails

#### Scenario: Revocation enabled disables the opt-in

- **GIVEN** a `PinnedTrustMerlin` with the opt-in on and the pinned expired certificate
- **WHEN** `verifyTrust` is called with `enableRevocation=true`
- **THEN** verification fails

This change SHALL NOT add a profile-level revocation setting.

### Requirement: Startup fail-closed validation of the opt-in

The bridge SHALL reject the profile at startup when `allow_expired_pinned_certificate` is true but the resolved inbound actions lack `Signature` or no keystore/truststore is configured, and SHALL log a startup WARN naming the profile and accepted risk when the opt-in is active. Existing `resolveActionsOrDefault` resolution SHALL apply: blank inbound actions default to `Signature`, including keystore-only consumer profiles; explicit actions without `Signature` reject.

#### Scenario: Startup rejects flag without Signature action

- **GIVEN** a profile with the opt-in on whose inbound actions lack `Signature`
- **WHEN** the bridge builds the profile
- **THEN** startup fails with an error naming the knob

#### Scenario: Startup rejects flag without verification material

- **GIVEN** the opt-in is on and inbound actions are blank, but neither truststore nor keystore is configured
- **WHEN** the bridge builds the profile
- **THEN** startup fails with an error naming the knob

#### Scenario: Startup warns about the accepted risk

- **GIVEN** a valid opted-in profile
- **WHEN** the bridge builds and loads the profile
- **THEN** a WARN naming the profile and the loss of expiry as a key-lifetime bound is logged

#### Scenario: Default inbound Signature action permits the opt-in

- **GIVEN** the opt-in is on, a truststore is configured, and inbound actions are blank
- **WHEN** the bridge builds the profile
- **THEN** actions resolve to `Signature` and startup succeeds

### Requirement: Independent composition with the SHA-1 algorithm allow-list

The expiry opt-in and the SHA-1 algorithm allow-list SHALL remain independent knobs: the algorithm allow-list is evaluated by signature-algorithm validation, the expiry opt-in only by trust validation. A profile may enable both, and a rejection MUST be attributable to exactly one gate by its error message.

#### Scenario: SHA-1 and expiry opt-ins compose

- **GIVEN** the partner case: a self-signed certificate with an RSA-SHA1 signature, expired, pinned, with both opt-ins enabled
- **WHEN** an envelope signed with it is verified
- **THEN** verification succeeds

#### Scenario: SHA-1 allow-list off rejects with the algorithm error

- **GIVEN** the same certificate with the expiry opt-in on but the SHA-1 allow-list off
- **WHEN** an envelope signed with it is verified
- **THEN** verification fails with the signature-algorithm error, distinguishable from `invalidCert`

#### Scenario: Expiry opt-in off rejects with invalidCert

- **GIVEN** the same certificate with the SHA-1 allow-list on but the expiry opt-in off
- **WHEN** an envelope signed with it is verified
- **THEN** verification fails with `invalidCert`, distinguishable from the algorithm error

### Requirement: Consumer-path scope and configuration flow

The opt-in SHALL apply to the consumer inbound verification path only; the producer `ProfileInInterceptor` path SHALL NOT honor it, and this limit SHALL be documented. The knob SHALL flow per profile: `CxfSecurityFields` (`allow_expired_pinned_certificate`, default false) → env `CXF_PROFILE_<N>_ALLOW_EXPIRED_PINNED_CERTIFICATE` → `SecurityProfileStore` → `SecurityProfile.Builder` → `getVerificationCrypto()`, which instantiates `PinnedTrustMerlin` inside the existing volatile/synchronized block only when the knob is on. The class is instantiated with `new`, so no native-image reflection registration is added.

#### Scenario: Env var reaches the bridge profile

- **GIVEN** a route profile with `allow_expired_pinned_certificate = true`
- **WHEN** the bridge process env is generated
- **THEN** `CXF_PROFILE_<N>_ALLOW_EXPIRED_PINNED_CERTIFICATE=true` is present and the Java profile stores the opt-in

#### Scenario: Default off keeps stock Merlin behavior

- **GIVEN** a profile without the knob
- **WHEN** `getVerificationCrypto()` is called
- **THEN** the Crypto is built by `CryptoFactory` exactly as before the change

#### Scenario: Producer path unaffected

- **GIVEN** a producer profile with the opt-in on and a response signed with the pinned expired certificate
- **WHEN** the producer `ProfileInInterceptor` verifies the response
- **THEN** verification fails with `invalidCert`, using stock `CryptoFactory` Crypto without expiry relaxation
