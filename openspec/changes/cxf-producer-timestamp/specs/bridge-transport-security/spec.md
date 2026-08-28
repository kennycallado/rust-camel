## ADDED Requirements

### Requirement: CXF producer emits Timestamp when out-actions request it

The bridge-side producer security profile SHALL honor a `Timestamp` token
in an accepted outbound action set (Timestamp paired with Signature) by
emitting a wsu:Timestamp in the outbound message. The outbound action
list SHALL order Timestamp before Signature so that signature-part
resolution covers an already-materialized element, mirroring the consumer
path. When both Signature and Timestamp are active and no explicit
signature parts are configured, the applied coverage SHALL default to
Body + Timestamp.

#### Scenario: Timestamp token produces an emitting, correctly ordered interceptor

- **GIVEN** a security profile with out-actions "Signature Timestamp" and a signing keystore
- **WHEN** the profile builds its outbound WSS4J interceptor
- **THEN** the interceptor's action list contains both tokens with Timestamp preceding Signature

#### Scenario: signed message carries a covered Timestamp

- **GIVEN** out-actions "Signature Timestamp", a signing keystore, and no SIGNATURE_PARTS configured
- **WHEN** a message is processed by the outbound interceptor
- **THEN** the output XML contains a wsu:Timestamp element and the signature's references cover the SOAP Body and the wsu:Timestamp element

#### Scenario: explicit parts are respected verbatim on the wire

- **GIVEN** out-actions "Signature Timestamp", a signing keystore, and an explicit SIGNATURE_PARTS of Body-only ("Body")
- **WHEN** a message is processed by the outbound interceptor
- **THEN** the signature's references cover exactly the SOAP Body element and include no Timestamp reference

#### Scenario: outbound wire format is unchanged for Timestamp-free profiles

- **GIVEN** a security profile with out-actions "Signature", a signing keystore, and no SIGNATURE_PARTS configured
- **WHEN** the outbound interceptor signs a message
- **THEN** the output carries no wsu:Timestamp and the signature's references cover the SOAP Body only, identical to pre-change behavior
