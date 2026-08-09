# Transformation

Transformation patterns change the content, format, or type of the exchange body. They convert between wire formats, enrich the body with external data, or set the body from an expression. They correspond to the *Message Transformation* category in Hohpe and Woolf.

- [Convert Body](convert-body.md) — convert the exchange body to a new type in the pipeline
- [Marshal and Unmarshal](marshal-unmarshal.md) — serialize and deserialize the body to or from a named format
- [Transform](transform.md) — set the body from a Simple expression or a literal value
- [Script](script.md) — run an inline script that can modify the exchange
- [Poll Enrich](poll-enrich.md) — poll a resource and replace the body with the result
- [Content Enricher](content-enricher.md) — call a resource and replace the body with the enriched result

For the data types these steps operate on, see [Exchange and Message](../concepts/exchange-message.md).
