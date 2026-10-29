# camel-language-xpath

> XPath Language for rust-camel (XPath 1.0 via sxd-xpath)

## Overview

XPath Language for rust-camel (XPath 1.0 via sxd-xpath).

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-language-xpath = "*"
```

## Number boundary

- Non-finite results (`+Inf`, `-Inf`, `NaN`) are refused with a typed
  evaluation error of class `arithmetic`. The step fails; no Null is
  produced.
- Finite numbers project integral values inside the exact signed and unsigned
  64-bit intervals to JSON integer form. Every other finite value uses float
  form.
- Predicates accept XPath boolean results only. There is no truthiness
  coercion; a match test must be wrapped in `boolean()`.
