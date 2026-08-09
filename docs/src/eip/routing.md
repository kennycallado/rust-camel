# Routing

Routing patterns decide where an exchange goes next. They pick a destination, branch on content, fan out to several endpoints, or distribute load. They correspond to the *Message Routing* category in Hohpe and Woolf.

- [Message Filter](filter.md) — pass or drop an exchange by predicate
- [Content-Based Router](content-based-router.md) — route an exchange to one of several destinations by predicate
- [Dynamic Router](dynamic-router.md) — compute the destination at runtime from exchange content
- [Recipient List](recipient-list.md) — broadcast an exchange to a list of endpoints computed at runtime
- [Routing Slip](routing-slip.md) — attach a sequence of endpoints and route through each in order
- [Scatter-Gather](scatter-gather.md) — broadcast to a fixed list of endpoints and collect the responses
- [Wire Tap](wire-tap.md) — send a copy of the exchange to a side endpoint without blocking the main flow
- [Multicast](multicast.md) — send the exchange to several destinations in parallel
- [Load Balancer](load-balancer.md) — distribute exchanges across destination endpoints by strategy

For the route structure that hosts these patterns, see [Routes and pipelines](../concepts/routes-pipelines.md).
