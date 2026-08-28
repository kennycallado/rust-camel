## ADDED Requirements

### Requirement: CXF producer inbound security processing is wired and functional

The bridge-side producer's inbound WSS4J interceptor SHALL pass crypto
configuration in the property shape WSS4J requires — String reference-id
properties naming keys that hold the crypto `Properties` objects — so
that configured inbound Signature verification and Encrypt decryption of
peer responses function at runtime.

#### Scenario: signed peer response verifies through the inbound interceptor

- **GIVEN** a profile with a truststore and in-actions "Signature"
- **WHEN** a document signed by the outbound interceptor chain is processed by the profile's inbound interceptor on a real in-phase chain
- **THEN** processing completes without exception and the Body content survives verification

#### Scenario: encrypted peer response decrypts through the inbound interceptor

- **GIVEN** a profile with a keystore and in-actions "Encrypt"
- **WHEN** an encrypted document is processed by the profile's inbound interceptor on a real in-phase chain
- **THEN** processing completes without exception and the original content round-trips

#### Scenario: crypto properties use the reference-id shape

- **GIVEN** a profile with inbound security actions configured
- **WHEN** the inbound interceptor is created
- **THEN** SIG_PROP_REF_ID and DEC_PROP_REF_ID hold String reference-ids, and the crypto Properties objects are stored under those reference-id keys
