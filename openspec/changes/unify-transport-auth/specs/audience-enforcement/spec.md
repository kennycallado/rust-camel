# Spec Delta: audience-enforcement

## ADDED Requirements

### Requirement: Provider audience binding enforcement

Named provider registry entries SHALL declare accepted issuers and
audiences. Authentication requests SHALL carry the route's audience,
the provider's accepted issuer set, and the transport context, and
token validation SHALL be bound to the authenticating provider: a
provider accepts a token only when its own validation rules
(including issuer and audience) accept it independently — no
provider may accept a token on the grounds that another provider's
validation succeeded. Authentication caches SHALL be provider-local:
cache keys SHALL include the provider identity in addition to route
audience, issuer, and transport context, so providers sharing
audience, issuer, and transport cannot read each other's entries.

#### Scenario: cross-provider token substitution rejected

- **GIVEN** provider A and provider B configured with the same
  accepted issuer and the same audience, and a token minted for a
  route of provider A
- **WHEN** the token is presented to an `Authenticated` route of
  provider B on the same transport
- **THEN** provider B's independent validation rejects the token
  (per-provider key material or signature verification differs) and
  authentication fails; no cache entry from provider A's
  authentication of that token is reused

#### Scenario: identical provider configs still isolate caches

- **GIVEN** two providers with identical accepted issuers and
  audiences and a token both would independently accept
- **WHEN** the token authenticates on a route of provider A and is
  then presented to a route of provider B on the same transport
- **THEN** both authentications may succeed but hit separate cache
  entries keyed by provider identity

#### Scenario: issuer isolation holds across providers

- **GIVEN** two providers configured with disjoint accepted-issuer
  sets and a token signed by an issuer accepted only by the first
- **WHEN** the token is presented to a route of the second provider
  on the same transport
- **THEN** authentication fails

#### Scenario: same audience, different transport, isolated cache

- **GIVEN** the same valid token presented on http and ws routes
  sharing one audience binding and one provider
- **WHEN** authentication results are cached
- **THEN** the cache stores separate entries per transport context
  and both requests succeed

#### Scenario: cache entries distinguish audiences

- **GIVEN** two routes with different audience bindings accepting
  tokens from the same issuer and provider, and a token valid only
  for the first audience
- **WHEN** the token is presented to the second route on the same
  transport
- **THEN** authentication fails and no cache entry from the first
  route's authentication is reused
