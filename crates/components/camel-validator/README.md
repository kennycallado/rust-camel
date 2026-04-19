# camel-component-validator

Validator component for rust-camel supporting XSD, JSON Schema, and YAML schema validation.

## URI format

`validator:path/to/schema[?type=xml|json|yaml]`

If `type` is omitted, schema type is inferred from file extension.

## Startup behavior

Schema is compiled and cached when the validator endpoint is created.

## Build requirement

XSD validation is delegated to `xml-bridge` (gRPC backend) and no longer depends on `libxml2` in this crate.
