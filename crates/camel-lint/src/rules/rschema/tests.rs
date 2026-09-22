use super::*;
use crate::test_support::StubCatalog;

fn slice<'a>(raw: &'a str, span: &Span) -> &'a str {
    &raw[span.start..span.end]
}

fn analyze(source: &str) -> Vec<Diagnostic> {
    let doc = Document::parse(source);
    assert!(
        doc.parse_failure.is_none(),
        "test fixtures must parse cleanly (got: {:?})",
        doc.parse_failure
    );
    RSchemaRule.analyze(&doc, &StubCatalog::empty())
}

fn rschema_only(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::RSchema)
        .collect()
}

#[test]
fn rschema_wrong_type_reports_value() {
    // `steps` must be an array; a string violates the `type` keyword.
    let source = "id: r1\nfrom: direct:start\nsteps: notanarray\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema
            .iter()
            .any(|d| slice(source, &d.span) == "notanarray"),
        "expected a RSchema diagnostic on the `steps` string value; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_missing_required_reports_parent() {
    // `RouteDslRoute` requires both `id` and `from`; omitting `from`
    // yields a `required` error whose instance_path is the route object
    // itself (the missing key has no node). The span anchors on that
    // parent mapping — the whole route, which contains `id`.
    let source = "id: r1\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected at least one RSchema diagnostic for the missing `from`"
    );
    // The parent-object span must be non-empty and cover the route body.
    let covers_parent = rschema
        .iter()
        .any(|d| d.span.start == 0 && source[d.span.start..d.span.end].contains("id"));
    assert!(
        covers_parent,
        "expected the diagnostic to anchor on the parent route object"
    );
}

#[test]
fn rschema_minimum_reports_numeric_value() {
    // `concurrent` is a direct `RouteDslRoute` property with `minimum: 0`
    // (not wrapped in anyOf, so the keyword fires at the leaf). `-1`
    // violates it; the diagnostic must anchor on the offending value.
    let source = "id: r1\nfrom: direct:start\nconcurrent: -1\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "-1"),
        "expected a RSchema diagnostic on `-1`; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_anyof_failure_reports_value() {
    // `circuit_breaker` is `anyOf: [RouteDslCircuitBreaker, null]`; an
    // integer matches neither branch. The diagnostic anchors on the value.
    let source = "id: r1\nfrom: direct:start\ncircuit_breaker: 123\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "123"),
        "expected a RSchema diagnostic on `123`; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_additional_properties_reports_key() {
    // `RouteDslRoute` has `additionalProperties: false`; `bogus` is not
    // allowed. The diagnostic must anchor on the offending KEY.
    let source = "id: r1\nfrom: direct:start\nbogus: 1\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "bogus"),
        "expected a RSchema diagnostic on the `bogus` key; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_items_violation_reports_array() {
    // `RouteDslRoute.steps` has `items: { $ref: "#/$defs/RouteDslStep" }`
    // and `type: "array"`. A numeric element violates the `items`
    // subschema (it does not match any `anyOf` branch of RouteDslStep).
    // jsonschema points the instance_path at the offending element, so the
    // diagnostic must anchor on the `123` element, not the whole array.
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - 123\n";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "123"),
        "expected an RSchema diagnostic anchoring on the offending array element `123`; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_skips_when_parse_failure() {
    let source = "steps:\n  - to: timer:foo\n  bad: [";
    let doc = Document::parse(source);
    assert!(doc.parse_failure.is_some(), "fixture must fail to parse");
    let diags = RSchemaRule.analyze(&doc, &StubCatalog::empty());
    assert!(
        diags.is_empty(),
        "R-SCHEMA must skip a parse-failed document"
    );
}

#[test]
fn rschema_envelope_form_valid_is_silent() {
    // Regression for the false-positive cluster: a valid multi-route
    // envelope `{routes: [...]}` must be validated AS-IS (not re-wrapped).
    // Re-wrapping produced `{routes: [{routes: [...]}]}` and dozens of
    // bogus "id/from required" + "'routes' was unexpected" errors.
    let source = "\
routes:
  - id: r1
    from: direct:start
    steps:
      - to: log:info
  - id: r2
    from: timer:tick?period=1000
    steps:
      - to: log:info
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a valid multi-route envelope must produce no R-SCHEMA errors; got: {:?}",
        rschema
    );
}

#[test]
fn rschema_envelope_form_reports_real_defect() {
    // Envelope form must still catch a genuine defect in route N>0; the
    // span-strip must handle any route index (instance_path /routes/1/...).
    let source = "\
routes:
  - id: r1
    from: direct:start
  - id: r2
    from: direct:other
    steps: notanarray
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected an R-SCHEMA error for the malformed `steps` in route 2"
    );
}

#[test]
fn rschema_legacy_array_form_is_silent_when_valid() {
    // Legacy array form `[ {...}, {...} ]` is normalised to
    // `{routes: <array>}`; valid bare routes must pass.
    let source = "\
- id: r1
  from: direct:start
  steps:
    - to: log:info
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a valid legacy-array route must produce no R-SCHEMA errors; got: {:?}",
        rschema
    );
}

#[test]
fn rschema_bare_route_valid_is_silent() {
    // Bare single-route form (depth 2) with a clean document must produce
    // no R-SCHEMA diagnostics — proves the depth-2 normalisation path
    // validates cleanly, not just defects.
    let source = "\
id: r1
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a valid bare single route must produce no R-SCHEMA errors; got: {:?}",
        rschema
    );
}

#[test]
fn rschema_rest_form_valid_is_silent() {
    // A `rest:`-block document is a valid DSL form (camel-dsl
    // `RouteDslRest`, lowered by `expand_rest_into`). ROUTE_SCHEMA models
    // the rest block in its envelope (rc-p86s), so a well-formed rest
    // document validates cleanly at envelope depth 0. (rc-xmbi previously
    // skipped the form entirely because the schema had no RestDsl defs —
    // the bare-route normalisation wrapped it as `{routes: [{rest: ...}]}`
    // and `RouteDslRoute`'s `additionalProperties: false` rejected the
    // `rest` key.)
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    security_policy:
      roles: [\"user\"]
      provider: native-demo
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a well-formed rest-block document must validate cleanly; got: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_rest_form_defect_anchors_value() {
    // Depth-0 rest form with a type defect: `port` must be an integer.
    // The diagnostic must anchor on the offending value, proving span
    // resolution works through the envelope-depth-0 path mapping.
    let source = "\
rest:
  - host: 0.0.0.0
    port: notaport
    path: /api/users
    operations:
      - method: GET
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "notaport"),
        "expected a RSchema diagnostic on the `port` string value; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_rest_form_missing_method_reports_parent() {
    // `method` is the only required field of a rest operation; omitting it
    // yields a `required` error whose instance path is the operation
    // object itself, anchoring on the parent mapping.
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected an RSchema diagnostic for the missing `method`"
    );
}

#[test]
fn rschema_rest_form_unknown_key_reports_key() {
    // `deny_unknown_fields` on `RouteDslRestOperation`: an undeclared
    // operation key is an `additionalProperties` error anchored on the
    // KEY (not the parent).
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        verb: GET
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "verb"),
        "expected the diagnostic to anchor on the `verb` key; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mixed_routes_and_rest_document() {
    // An envelope carrying BOTH `routes` and `rest` validates both sides;
    // a defect inside the rest block is flagged while the valid route
    // stays silent (one diagnostic, not a cascade).
    let source = "\
routes:
  - id: r1
    from: direct:start
rest:
  - host: 0.0.0.0
    port: notaport
    path: /api
    operations:
      - method: GET
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly the `port` type error; got: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
    assert!(slice(source, &rschema[0].span) == "notaport");
}

#[test]
fn rschema_rest_form_steps_defect_anchors_value() {
    // An operation's `steps` reuse the shared `RouteDslStep` defs; a type
    // defect inside the nested steps is flagged at the leaf.
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api
    operations:
      - method: GET
        steps: notanarray
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema
            .iter()
            .any(|d| slice(source, &d.span) == "notanarray"),
        "expected a RSchema diagnostic on the `steps` string value; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_rest_wrong_shape_reports_mapping() {
    // `rest:` must be an ARRAY of blocks. A mapping under `rest:` is a
    // type error at the `rest` key itself — the depth-0 instance path is
    // `/rest` (noyalib path `rest`), a distinct span-mapping case from
    // the per-block paths like `/rest/0/port`.
    let source = "\
rest:
  host: 0.0.0.0
  port: 9090
  path: /api
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected an RSchema diagnostic for the mapping-shaped `rest` value"
    );
    assert!(
        rschema
            .iter()
            .any(|d| slice(source, &d.span).contains("host")),
        "expected the diagnostic to anchor on the `rest` mapping; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_rest_port_env_default_carves_silently() {
    // Integer-position carve-out through the rest form: `port` is a
    // bounded integer (u16, maximum 65535) in the rest-block defs; a
    // whole-scalar `${env:X:-d}` token with a clean-integer default
    // validates as the NUMBER — no Error, no Info note (the carved leaf's
    // note is suppressed). Pins the form-agnostic typing-mirror claim.
    let source = "\
rest:
  - host: 0.0.0.0
    port: ${env:REST_PORT:-9090}
    path: /api
    operations:
      - method: GET
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a clean-integer default at the rest `port` position must carve out \
             silently; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_form_valid_is_silent() {
    // A `mcp:`-block document is a valid DSL form (camel-dsl
    // `RouteDslMcp`, lowered by `expand_mcp_into`). ROUTE_SCHEMA models
    // the mcp block in its envelope (rc-6pikg), so a well-formed mcp
    // document validates cleanly at envelope depth 0. Before the fold,
    // the bare-route normalisation wrapped it as `{routes: [{mcp: ...}]}`
    // and `RouteDslRoute`'s `additionalProperties: false` rejected the
    // `mcp` key — the exact rc-p86s gap class, for mcp.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      security_policy:
        roles: [\"admin\"]
      tls:
        cert_path: /etc/certs/crm.pem
        key_path: /etc/certs/crm-key.pem
      max_tools: 200
      max_resources: 64
    tools:
      - name: lookup
        input_schema:
          type: object
          properties:
            id:
              type: string
          required: [id]
    resources:
      - name: customers
        uri: crm://customers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a well-formed mcp-block document must validate cleanly; got: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_form_defect_anchors_value() {
    // Depth-0 mcp form with a type defect: `bind` must be a string; an
    // integer there is a type error. The diagnostic must anchor on the
    // offending value, proving span resolution works through the
    // envelope-depth-0 `/mcp/0/server/bind` path mapping.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 9090
    tools:
      - name: lookup
        input_schema:
          type: object
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "9090"),
        "expected a RSchema diagnostic on the integer `bind` value; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_form_missing_bind_reports_parent() {
    // `name` and `bind` are the required fields of `RouteDslMcpServer`
    // (`tls`/`security_policy`/caps all have serde defaults); omitting
    // `bind` yields a `required` error whose instance path is the server
    // object itself, anchoring on the parent mapping.
    let source = "\
mcp:
  - server:
      name: crm
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected an RSchema diagnostic for the missing `bind`"
    );
}

#[test]
fn rschema_mcp_tls_blank_cert_path_empty_rejected() {
    // mcpcert: ROUTE_SCHEMA carries `\S` patterns on the MCP TLS path
    // fields, so a blank `cert_path` (empty string) is a pattern
    // violation. With `key_path` valid, the only possible error site is
    // cert_path. The span anchors on the raw YAML token INCLUDING the
    // quotes (`""`), and the message carries jsonschema's pattern shape
    // (`does not match`) — never a field name.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: \"\"
        key_path: /etc/certs/crm-key.pem
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly one diagnostic for the blank cert_path; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let d = rschema[0];
    let raw = slice(source, &d.span);
    assert_eq!(raw, "\"\"", "span must anchor on the raw quoted token");
    assert!(
        raw.trim_matches('"').trim().is_empty(),
        "the anchored value must be blank; sliced: {raw:?}"
    );
    assert!(
        d.message.contains("does not match"),
        "pattern violation message must say `does not match`; got: {}",
        d.message
    );
}

#[test]
fn rschema_mcp_tls_blank_cert_path_whitespace_rejected() {
    // Same defect shape as the empty cert_path, but the blank is three
    // spaces inside quotes: the raw slice is `"   "` (quote + 3 spaces +
    // quote) and must be blank after quote/whitespace trimming.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: \"   \"
        key_path: /etc/certs/crm-key.pem
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly one diagnostic for the whitespace cert_path; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let d = rschema[0];
    let raw = slice(source, &d.span);
    assert_eq!(raw, "\"   \"", "span must anchor on the raw quoted token");
    assert!(
        raw.trim_matches('"').trim().is_empty(),
        "the anchored value must be blank; sliced: {raw:?}"
    );
    assert!(
        d.message.contains("does not match"),
        "pattern violation message must say `does not match`; got: {}",
        d.message
    );
}

#[test]
fn rschema_mcp_tls_blank_key_path_empty_rejected() {
    // Mirror of the empty-cert_path pin: with `cert_path` valid, the only
    // possible error site is key_path, so the single diagnostic pins the
    // blank `""` key value.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: /etc/certs/crm.pem
        key_path: \"\"
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly one diagnostic for the blank key_path; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let d = rschema[0];
    let raw = slice(source, &d.span);
    assert_eq!(raw, "\"\"", "span must anchor on the raw quoted token");
    assert!(
        raw.trim_matches('"').trim().is_empty(),
        "the anchored value must be blank; sliced: {raw:?}"
    );
    assert!(
        d.message.contains("does not match"),
        "pattern violation message must say `does not match`; got: {}",
        d.message
    );
}

#[test]
fn rschema_mcp_tls_blank_key_path_whitespace_rejected() {
    // Mirror of the whitespace cert_path pin for key_path: `"   "` is a
    // pattern violation anchored on the raw quoted token.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: /etc/certs/crm.pem
        key_path: \"   \"
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly one diagnostic for the whitespace key_path; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let d = rschema[0];
    let raw = slice(source, &d.span);
    assert_eq!(raw, "\"   \"", "span must anchor on the raw quoted token");
    assert!(
        raw.trim_matches('"').trim().is_empty(),
        "the anchored value must be blank; sliced: {raw:?}"
    );
    assert!(
        d.message.contains("does not match"),
        "pattern violation message must say `does not match`; got: {}",
        d.message
    );
}

#[test]
fn rschema_mcp_tls_padded_valid_path_stays_silent() {
    // Trimmed-valid pin: a real path wrapped in leading/trailing spaces
    // still matches `\S` (the pattern is not anchored), and boot trims
    // and accepts — so the lint must stay completely silent.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: \" /etc/certs/crm.pem \"
        key_path: /etc/certs/crm-key.pem
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a padded-but-valid cert_path must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_tls_blank_cert_path_empty_env_default_rejected() {
    // Boot parity through the env canon: a whole-scalar
    // `${env:CERT:-}` token (EMPTY default) substitutes to `""` in the
    // validation copy, which fails the `\S` pattern — the diagnostic
    // anchors on the AUTHORED token (boot substitutes then rejects
    // there too). With `key_path` valid, no error may anchor anywhere
    // else.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      tls:
        cert_path: ${env:CERT:-}
        key_path: /etc/certs/crm-key.pem
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        !errors.is_empty(),
        "expected at least one Error for the empty-default cert token; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:CERT:-}")
                && d.message.contains("does not match")),
        "expected a pattern Error anchored on the authored token; got: {:?}",
        errors
            .iter()
            .map(|d| (slice(source, &d.span), d.message.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(
        errors
            .iter()
            .all(|d| slice(source, &d.span).contains("${env:CERT:-}")),
        "no error may anchor outside the blank cert token; got: {:?}",
        errors
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_form_unknown_key_reports_key() {
    // `deny_unknown_fields` on `RouteDslMcpServer`: the DSL carries no
    // session/protocol-version keys, so an undeclared server key is an
    // `additionalProperties` error anchored on the KEY (not the parent).
    // Mirrors the camel-dsl `unknown_server_key_rejected` parse pin.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      session: true
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| slice(source, &d.span) == "session"),
        "expected the diagnostic to anchor on the `session` key; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mixed_routes_rest_and_mcp_document() {
    // An envelope carrying `routes`, `rest`, AND `mcp` validates all three
    // sides in place (single depth-0 pass, no per-form branching); a defect
    // inside the mcp block is flagged while the valid route and rest block
    // stay silent (one diagnostic, not a cascade).
    let source = "\
routes:
  - id: r1
    from: direct:start
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api
    operations:
      - method: GET
        to: direct:listUsers
mcp:
  - server:
      name: crm
      bind: 9090
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert_eq!(
        rschema.len(),
        1,
        "expected exactly the `bind` type error; got: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
    assert!(slice(source, &rschema[0].span) == "9090");
}

#[test]
fn rschema_mcp_wrong_shape_reports_mapping() {
    // `mcp:` must be an ARRAY of blocks. A mapping under `mcp:` is a type
    // error at the `mcp` key itself — the depth-0 instance path is `/mcp`
    // (noyalib path `mcp`), the same span-mapping case the rest form pins
    // for `/rest`.
    let source = "\
mcp:
  server:
    name: crm
    bind: 127.0.0.1:9100
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected an RSchema diagnostic for the mapping-shaped `mcp` value"
    );
    assert!(
        rschema
            .iter()
            .any(|d| slice(source, &d.span).contains("server")),
        "expected the diagnostic to anchor on the `mcp` mapping; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mcp_caps_env_default_carves_silently() {
    // Integer-position carve-out through the mcp form: `max_tools` is a
    // `usize` cap (presence-based, unbounded) in the mcp-block defs; a
    // whole-scalar `${env:X:-d}` token with a clean-integer default
    // validates as the NUMBER — no Error, no Info note. Pins the
    // form-agnostic typing-mirror claim for the third envelope form.
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      max_tools: ${env:MCP_MAX_TOOLS:-200}
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a clean-integer default at the mcp `max_tools` position must carve \
             out silently; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_legacy_array_defect_anchors_element() {
    // Legacy array form (depth 1) with a defect: `steps` is a string
    // instead of an array. The diagnostic must anchor on the offending
    // value (`notanarray`), proving span resolution works through the
    // envelope_depth=1 wrapper.
    let source = "\
- id: r1
  from: direct:start
  steps: notanarray
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        !rschema.is_empty(),
        "expected at least one RSchema diagnostic for the malformed `steps`"
    );
    assert!(
        rschema
            .iter()
            .any(|d| slice(source, &d.span) == "notanarray"),
        "expected the diagnostic to anchor on `notanarray`; got spans: {:?}",
        rschema
            .iter()
            .map(|d| slice(source, &d.span))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_string_position_default_silent_with_info() {
    // rc-93wct rev 2: a string-typed field (`id` is strict "string" in
    // ROUTE_SCHEMA) carrying a whole-scalar `${env:X:-d}` token
    // validates against the substituted default — which the typing
    // mirror keeps a STRING — so zero Errors and exactly one Info note
    // anchored on the authored placeholder.
    let source = "\
id: ${env:MY_TITLE:-hello}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "string-position default must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    let info = infos[0];
    assert!(
        info.message.contains("MY_TITLE") && info.message.contains(":-hello"),
        "Info message must name the variable and default; got: {}",
        info.message
    );
    assert!(
        slice(source, &info.span).contains("${env:MY_TITLE:-hello}"),
        "Info span must anchor on the placeholder value node; sliced: {:?}",
        slice(source, &info.span)
    );
}

#[test]
fn rschema_int_position_clean_default_no_diagnostic() {
    // Integer-position carve-out (typing mirror int arm): `max_requests`
    // wants an integer and the default `2` is a clean integer, so the
    // validation copy carries the NUMBER — exactly as the boot loader
    // coerces the leaf. No Error, no Info note, nothing.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-2}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "int-position clean-integer default must produce no diagnostic; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_numeric_default_in_string_field_stays_string() {
    // Typing-mirror false-positive fix (rc-93wct rev 2): a
    // numeric-looking default substituted into a STRING field must not
    // be re-inferred as a number by the whole-text splice (the raw copy
    // `id: 8080` would type-error). The mirror forces the whole-scalar
    // leaf to the JSON string "8080" — clean pass, one Info note.
    let source = "\
id: ${env:RC93WCT_T:-8080}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "numeric default in a string field must stay a string; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    assert!(
        infos[0].message.contains(":-8080"),
        "Info must report the substituted `:-8080` default; got: {}",
        infos[0].message
    );
}

#[test]
fn rschema_structure_changing_default_no_false_positive() {
    // Structure-changing-default ceiling: an unquoted flow-style default
    // (`[a,b]`) splices into a NON-scalar node in the validation copy, so
    // the scalar String-forcing arm never fires. The typing mirror must
    // not let the splice decide the type — boot keeps the substituted
    // leaf a STRING (the route loads), so this shape at a string position
    // must stay silent (zero Errors), never a false-positive type Error.
    let source = "\
id: ${env:RC_CEIL:-[a,b]}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "structure-changing default at a string position must not be \
             type-flagged (boot string typing); got: {errors:?}"
    );
}

#[test]
fn rschema_no_default_flagged_at_string_position() {
    // A whole-scalar `${env:X}` (no default) is an Error even where the
    // literal placeholder text is a valid string — boot hard-fails on
    // the unresolved variable, so lint must not stay silent. The
    // `$${env:X}` escape is exempt (boot emits the literal text).
    let source = "\
id: ${env:NO_DEF_xyz}
from: $${env:ESCAPED_FROM}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error flagging the unresolved token; got: {errors:?}"
    );
    assert!(
        errors[0].message.contains("NO_DEF_xyz"),
        "Error message must name the unresolved variable; got: {}",
        errors[0].message
    );
    assert!(
        slice(source, &errors[0].span).contains("${env:NO_DEF_xyz}"),
        "Error must anchor on the token; sliced: {:?}",
        slice(source, &errors[0].span)
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ESCAPED_FROM")),
        "escaped $${{env:...}} must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mixed_document_per_token() {
    // Per-token semantics under the typing canon + integer-position
    // carve-out: the string-position WITH_DEF token validates cleanly
    // with exactly one Info note, while the int-position ALSO_WITH_DEF
    // token (clean integer default) is carved out — no diagnostic at
    // all. Each token is judged at its own position. (A whole-document
    // Err→raw fallback would re-literal BOTH tokens and lose the
    // defaulted one's clean pass.)
    let source = "\
id: ${env:WITH_DEF:-hello}
from: direct:start
steps:
  - throttle:
      max_requests: ${env:ALSO_WITH_DEF:-2}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let with_def_infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info && d.message.contains("${env:WITH_DEF}"))
        .collect();
    assert_eq!(
        with_def_infos.len(),
        1,
        "expected exactly one Info note for WITH_DEF; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "int-position clean default must not be type-flagged; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ALSO_WITH_DEF")),
        "carved-out int leaf must produce no diagnostic at all; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_commented_tokens_no_diagnostics() {
    // Comments are not part of the parsed instance: tokens inside them
    // resolve no value-leaf span and are skipped entirely — no Info
    // note with a (0,0) miss span, no unresolved Error, nothing.
    let source = "\
# ${env:C_NOD} unresolved token in a comment
id: commented
from: direct:start
# ${env:C_DEF:-d} defaulted token in a comment
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "comment tokens must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

// NOTE: the ambient-env hermeticity witness lives in
// `crates/camel-lint/tests/env_hermeticity.rs` — poisoning the process
// environment needs the standard-library env API, which the crate
// purity gate forbids anywhere under `src/`.

#[test]
fn rschema_no_default_int_position_still_error() {
    // A whole-scalar token with no default keeps its literal instance
    // value and is explicitly flagged by the typing-mirror walk (boot
    // hard-fails on the unresolved variable). At an int position the
    // schema type check fires on the same authored placeholder too —
    // the assertion holds for either source of the Error. The
    // integer-position carve-out never applies: there is no default to
    // coerce.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:NO_DEFAULT_xyz}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| d.severity == Severity::Error
            && slice(source, &d.span).contains("${env:NO_DEFAULT_xyz}")),
        "expected an Error-severity diagnostic on the literal placeholder; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_position_notanumber_still_error() {
    // The carve-out needs a CLEAN integer default: `notanumber` stays on
    // the STRING validation copy → a schema type Error anchored on the
    // authored placeholder (boot parity: the route fails there too).
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-notanumber}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:MY_LIMIT:-notanumber}")),
        "expected a type Error anchored on the placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_position_leading_zero_still_error() {
    // Leading zeros are not a clean integer (YAML 1.1 octal ambiguity):
    // `007` keeps string typing → schema type Error, like boot.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-007}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:MY_LIMIT:-007}")),
        "expected a type Error anchored on the placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_default_at_polymorphic_position_keeps_info() {
    // Guard-arm pin for the integer-position carve-out (Condition A):
    // `set_header.value` is polymorphic in ROUTE_SCHEMA (no `type`
    // constraint), so the STRING copy validates cleanly and the carve-out
    // must NOT fire — the numeric-looking default `${env:H:-123}` keeps
    // today's string-position behavior: exactly one Info note, zero
    // Errors. A refactor that carved unconditionally (dropping the
    // Condition A guard) would suppress the Info and fail this test.
    let source = "\
id: r1
from: direct:start
steps:
  - set_header:
      key: X-Custom
      value: ${env:H:-123}
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "polymorphic-position default must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    assert!(
        infos[0].message.contains(":-123"),
        "Info must report the substituted `:-123` default; got: {}",
        infos[0].message
    );
    assert!(
        slice(source, &infos[0].span).contains("${env:H:-123}"),
        "Info span must anchor on the authored placeholder; sliced: {:?}",
        slice(source, &infos[0].span)
    );
}

#[test]
fn bool_position_placeholder_still_error() {
    // Guard-arm pin for the integer-position carve-out (Condition B):
    // `auto_startup` is a strict boolean field in ROUTE_SCHEMA. The
    // clean-integer default `1` makes the leaf an IntCandidate, but the
    // NUMBER copy still type-errors at the boolean position — Condition B
    // keeps the STRING copy, so the schema type Error anchored on the
    // authored placeholder survives (boot parity: the loader rejects the
    // coerced number there too).
    let source = "\
id: r1
from: direct:start
auto_startup: ${env:AS:-1}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:AS:-1}")),
        "expected a type Error anchored on the bool-position placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    // The Info note must ALSO survive: the candidate reached the carve-out
    // loop, Condition A fired (string copy errors), and Condition B kept
    // the STRING copy (the number copy errors too) — so the leaf is not
    // carved and the substituted-default note is not suppressed. A
    // refactor that let the number copy win at non-integer positions
    // would carve the leaf, drop this Info, and fail the assertion.
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected the substituted-default Info note to survive the bool \
             position; got: {infos:?}"
    );
}

#[test]
fn clean_integer_lexical_gate() {
    // SYNC gate pin: mirrors camel-dsl env_int_probe::clean_integer /
    // camel-config clean_i64 — lexical `-?(0|[1-9][0-9]*)` (no leading
    // zeros, no whitespace, no plus), then i64-or-u64 parse.
    let clean = [
        "0",
        "2",
        "-3",
        "9223372036854775807",  // i64::MAX
        "9223372036854775808",  // i64 overflow, u64 magnitude
        "18446744073709551615", // u64::MAX
    ];
    for s in clean {
        assert!(clean_integer(s).is_some(), "`{s}` must be a clean integer");
    }
    let not_clean = [
        "",
        "-",
        "+2",
        "007",
        "-007",
        "1e3",
        " 2",
        "2 ",
        "1_000",
        "2.0",
        "true",
        "99999999999999999999999", // > u64::MAX — overflow, boot rejects
    ];
    for s in not_clean {
        assert!(
            clean_integer(s).is_none(),
            "`{s}` must NOT be a clean integer"
        );
    }
}

#[test]
fn rschema_duplicate_tokens_anchor_distinct_spans() {
    // The SAME `${env:DUP_VAR:-same}` token in two string-typed fields
    // must emit one Info note PER authored occurrence, each anchored on
    // its own value node (occurrence queue — a first-match search
    // collapsed both notes onto the first route's `id`). String
    // positions keep the document Error-free under the typing canon.
    let source = "\
routes:
  - id: ${env:DUP_VAR:-same}
    from: direct:a
  - id: ${env:DUP_VAR:-same}
    from: direct:b
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "string-position duplicates must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        2,
        "expected two Info notes (one per authored occurrence); got: {infos:?}"
    );
    let (a, b) = (slice(source, &infos[0].span), slice(source, &infos[1].span));
    assert!(
        a.contains("${env:DUP_VAR:-same}") && b.contains("${env:DUP_VAR:-same}"),
        "both Info spans must slice to an authored occurrence of the token; \
             got: {:?} and {:?}",
        a,
        b
    );
    assert_ne!(
        infos[0].span, infos[1].span,
        "the two Info notes must anchor on DIFFERENT authored occurrences"
    );
}

#[test]
fn lint_accepts_remove_header_route() {
    // `remove_header` is a first-class RouteDslStep variant: a route using
    // it must validate with zero errors against ROUTE_SCHEMA. Direct
    // jsonschema validation (not the full Rule) isolates the schema shape
    // from rule-level span anchoring.
    let schema: serde_json::Value =
        serde_json::from_str(ROUTE_SCHEMA).expect("embedded route schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("embedded route schema must compile");
    let doc: serde_json::Value = serde_json::from_str(
            r#"{"routes": [{"id": "r1", "from": "direct://test", "steps": [{"remove_header": {"key": "CamelHttpPath"}}]}]}"#,
        )
        .expect("remove_header fixture must parse as JSON");
    let errors: Vec<_> = validator.iter_errors(&doc).collect();
    assert!(
        errors.is_empty(),
        "a remove_header step must validate cleanly; got: {:?}",
        errors
    );
}

#[test]
fn rschema_embedded_no_default_flagged() {
    // Boot-parity for EMBEDDED no-default tokens: `svc-${env:EMBED_UNDEF}`
    // is not a whole-scalar token, but the boot tree-walk still
    // Unresolved-fails on it — the mirror walk's embedded scan must flag
    // it with exactly one explicit Error naming the variable, anchored on
    // the token inside the scalar. The `$${env:...}` escape stays silent.
    let source = "\
id: svc-${env:EMBED_UNDEF}
from: $${env:ESCAPED_FROM}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error for the embedded no-default token; got: {errors:?}"
    );
    assert!(
        errors[0].message.contains("EMBED_UNDEF"),
        "Error must name the unresolved variable; got: {}",
        errors[0].message
    );
    assert!(
        slice(source, &errors[0].span).contains("${env:EMBED_UNDEF}"),
        "Error must anchor on the embedded token; sliced: {:?}",
        slice(source, &errors[0].span)
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ESCAPED_FROM")),
        "escaped $${{env:...}} must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_arg_no_default_flagged() {
    // jobargs Task 2.1: the lint tree has no argument context, so a
    // whole-scalar `${arg:NAME}` Unresolved-fails exactly like an
    // unresolved env token (boot parity). The arg message carries NO
    // "(no default)" qualifier — the arg grammar has no substitutable
    // default form at all, so the qualifier would be a lie.
    let source = "\
id: ${arg:JOB_NAME}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error flagging the unresolved arg token; got: {errors:?}"
    );
    assert!(
        errors[0]
            .message
            .contains("unresolved ${arg:JOB_NAME} placeholder"),
        "Error message must name the arg namespace and variable; got: {}",
        errors[0].message
    );
    assert!(
        !errors[0].message.contains("(no default)"),
        "arg tokens must carry no default qualifier; got: {}",
        errors[0].message
    );
    assert!(
        slice(source, &errors[0].span).contains("${arg:JOB_NAME}"),
        "Error must anchor on the token; sliced: {:?}",
        slice(source, &errors[0].span)
    );
}

#[test]
fn rschema_arg_fallback_rejected() {
    // jobargs Task 2.1: `${arg:NAME:-gold}` is NOT the env default form —
    // the arg grammar rejects `:-fallback`, so the boot tree-walk fails on
    // it and lint must flag it as an unresolved arg instead of emitting a
    // substituted-default Info note (no arg substitution ever happens
    // lint-side).
    let source = "\
id: ${arg:NAME:-gold}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error rejecting the arg fallback form; got: {errors:?}"
    );
    assert!(
        errors[0]
            .message
            .contains("unresolved ${arg:NAME} placeholder"),
        "Error message must name the arg variable; got: {}",
        errors[0].message
    );
    assert!(
        rschema.iter().all(|d| d.severity != Severity::Info),
        "the arg fallback must never be substituted (no Info note); got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(
        slice(source, &errors[0].span).contains("${arg:NAME:-gold}"),
        "Error must anchor on the full token including the fallback; sliced: {:?}",
        slice(source, &errors[0].span)
    );
}

#[test]
fn rschema_arg_escaped_silent() {
    // jobargs Task 2.1: `$${arg:NAME}` is the escape form — boot emits the
    // literal `${arg:NAME}` text, so lint must stay completely silent
    // (same policy as `$${env:...}`).
    let source = "\
id: $${arg:ESCAPED_ARG}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "escaped $${{arg:...}} must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}
