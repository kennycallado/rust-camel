# XPath language

XPath 1.0 implementation of the Language SPI over `sxd-document` and
`sxd-xpath`. It evaluates expressions and predicates against an Exchange XML
body.

## Trust model

- The XPath query is trusted operator configuration. Exchange data is not
  interpolated into the query.
- The XML body is untrusted, adversary-controlled data under ADR-0032. It is
  only the target document for the trusted query.

`max_input_bytes` bounds the raw XML body before parsing. The default is 1 MiB.
Setting it to `None` removes this bound and is an explicit operator choice.

## Security posture

The current `sxd-document` and `sxd-xpath` boundary has these properties:

- Both libraries are pure Rust and register no filesystem or network resolver.
- `sxd-document` has no `<!ENTITY>` declaration parser. It resolves only the
  five predefined XML entities. Recursive internal entity expansion, including
  a billion-laughs payload, is structurally unavailable.
- External entity declarations cannot trigger a file or network fetch. The
  parser does not resolve their system identifiers.
- `sxd_xpath::Context::new()` registers the XPath 1.0 core functions only. It
  does not register the URI-loading `document()` function.
- Parse and evaluation failures use generic messages. Logs do not include
  document-derived error text and stay at `warn!` under ADR-0012.

These properties are replacement requirements, not general guarantees for all
XML or XPath libraries.

## Known limitations

- **XPH-001:** Namespace prefixes are unsupported because the evaluation
  context has no prefix-to-URI map.
- **XPH-002:** `sxd-xpath` is unmaintained. A replacement must preserve the
  security posture above.
- Evaluation has no wall-clock timeout. The query is trusted operator
  configuration, while the untrusted XML input has a byte bound.

## API evolution

ADR-0049 applies `#[non_exhaustive]` to contract enums in the three contract
crates. This leaf crate defines no contract enum. `XPathConfig` is a public
configuration struct, so the contract-enum policy does not apply to it.

## Authority

- ADR-0012: handler-owned log levels
- ADR-0032: exchange-data trust boundary
- ADR-0049: workspace `#[non_exhaustive]` policy
