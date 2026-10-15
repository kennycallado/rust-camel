# Proposal: http-duplicate-consumers

## Why

Two HTTP Consumers on the same shared listener can register the same path or the same method and parsed path template. The second silently replaces the first; cancelling the first then removes the second. Plain HTTP routes bypass the DSL's duplicate checks. Bd: rc-om77p.

## What Changes

- In camel-http, reject occupied legacy path keys and occupied method/template keys atomically with CamelError::Config.
- Propagate registration failure before Consumer readiness and before entering the receive/cleanup loop.
- Cover original-consumer preservation, concurrent registration, sibling verbs, and restart after orderly cancellation.

## Acceptance criteria

- Duplicate registration returns a configuration error and preserves the original sender.
- Duplicate Consumer startup fails before readiness; cancelling the rejected Consumer leaves the original dispatch intact.
- GET and POST coexist; cancellation of one leaves the other.
- Orderly shutdown frees the key for a replacement Consumer.

## Risk budget

Existing last-write-wins duplicate configuration will now fail at startup. Keep HTTP co-hosting precedence (ADR-0009), template ambiguity behavior, static mounts, and Runtime lifecycle contracts unchanged. Domain language follows CONTEXT-MAP.md.
