//! Partner-script grammar: the endpoint-keyed scripting vocabulary of
//! a scenario document's `partners:` map (ADR-0069 section 9).
//!
//! Each entry maps an endpoint address to an ordered script list; a
//! script carries optional `method`, `path`, `times`, and `delay`
//! selectors plus exactly one of `response` / `fault`. The raw serde
//! stage keeps selectors as strings; conversion runs during document
//! validation so every failure names the entry key.
//!
//! The public grammar types are re-exported through
//! [`crate::document`], which owns the document model and the parse
//! entry point; this module stays private.

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::Value;
use noyalib::compat::serde_yaml;
use serde::Deserialize;

use crate::document::DocError;

// ---------------------------------------------------------------------------
// Public grammar
// ---------------------------------------------------------------------------

/// One partner script of a `partners:` entry: the response a partner
/// serves when the system under test reaches its endpoint, or the
/// fault it applies instead. Exactly one of `response` / `fault` must
/// be declared. Grammar only; the runner consumes the map.
#[derive(Debug, Clone)]
pub struct PartnerScript {
    /// Request method the script applies to; optional.
    pub method: Option<String>,
    /// Request path the script applies to; optional.
    pub path: Option<String>,
    /// How many requests the script applies to before it is
    /// exhausted; optional.
    pub times: Option<u32>,
    /// How long the partner waits before acting; optional.
    pub delay: Option<Duration>,
    /// The scripted response; mutually exclusive with `fault`.
    pub response: Option<PartnerScriptResponse>,
    /// The scripted fault; mutually exclusive with `response`.
    pub fault: Option<PartnerFault>,
}

/// The fault a partner script applies instead of serving a response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PartnerFault {
    /// Close the connection without answering.
    Close,
}

/// The response a partner script serves.
#[derive(Debug, Clone)]
pub struct PartnerScriptResponse {
    /// HTTP status code, validated to the 100-599 range at load.
    pub status: Option<u16>,
    /// Response headers.
    pub headers: Option<BTreeMap<String, String>>,
    /// Response body, encoded onto the wire with the client send
    /// path's `value_to_wire` semantics: a string serves as its exact
    /// bytes (no surrounding quotes, no escaping), null serves empty,
    /// any other value serves as compact JSON, and an absent body
    /// serves empty.
    pub body: Option<Value>,
}

// ---------------------------------------------------------------------------
// Raw serde stage
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPartnerScript {
    method: Option<String>,
    path: Option<String>,
    /// Raw repeat count; validation checks the range so the error can
    /// name the entry key.
    times: Option<u64>,
    /// Raw humantime string; parsed during validation so the error
    /// can name the entry key.
    delay: Option<String>,
    /// Raw fault name; validated during conversion so the error can
    /// name the entry key.
    fault: Option<String>,
    response: Option<RawPartnerScriptResponse>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPartnerScriptResponse {
    status: Option<u16>,
    headers: Option<BTreeMap<String, String>>,
    body: Option<Value>,
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Converts the raw `partners` map into the public script model.
/// Entries convert from the raw sequence with the entry key named on
/// every failure; an empty sequence is a valid, inert entry. `None`
/// stays `None`: the document declares no `partners:` section.
pub(crate) fn partners_from_raw(
    raw: Option<BTreeMap<String, serde_yaml::Value>>,
) -> Result<Option<BTreeMap<String, Vec<PartnerScript>>>, DocError> {
    let Some(raw_partners) = raw else {
        return Ok(None);
    };
    let mut partners = BTreeMap::new();
    for (endpoint, raw_scripts) in raw_partners {
        let entry_error = |message: String| DocError::Partners {
            endpoint: endpoint.clone(),
            message,
        };
        let scripts = serde_yaml::from_value::<Vec<RawPartnerScript>>(raw_scripts)
            .map_err(|e| entry_error(e.to_string()))?;
        let mut converted = Vec::with_capacity(scripts.len());
        for script in scripts {
            let times = match script.times {
                None => None,
                Some(times) => match u32::try_from(times) {
                    Ok(times) if times >= 1 => Some(times),
                    _ => {
                        return Err(entry_error(format!(
                            "`times` {times} is out of range; expected 1-4294967295"
                        )));
                    }
                },
            };
            let delay = match script.delay.as_deref() {
                None => None,
                Some(raw) => match humantime::parse_duration(raw) {
                    Ok(delay) => Some(delay),
                    Err(e) => {
                        return Err(entry_error(format!("invalid `delay` `{raw}`: {e}")));
                    }
                },
            };
            let fault = match script.fault.as_deref() {
                None => None,
                Some("close") => Some(PartnerFault::Close),
                Some(value) => {
                    return Err(entry_error(format!(
                        "unknown `fault` `{value}`; expected `close`"
                    )));
                }
            };
            match (&script.response, &fault) {
                (Some(_), Some(_)) => {
                    return Err(entry_error(
                        "`response` and `fault` are mutually exclusive; declare exactly one"
                            .to_string(),
                    ));
                }
                (None, None) => {
                    return Err(entry_error(
                        "a script requires `response` or `fault`; declare exactly one".to_string(),
                    ));
                }
                _ => {}
            }
            if let Some(response) = &script.response
                && let Some(status) = response.status
                && !(100..=599).contains(&status)
            {
                return Err(entry_error(format!(
                    "response `status` {status} is out of range; expected 100-599"
                )));
            }
            converted.push(PartnerScript {
                method: script.method,
                path: script.path,
                times,
                delay,
                response: script.response.map(|response| PartnerScriptResponse {
                    status: response.status,
                    headers: response.headers,
                    body: response.body,
                }),
                fault,
            });
        }
        partners.insert(endpoint, converted);
    }
    Ok(Some(partners))
}

/// Partner-script grammar parse tests, path-based like the document
/// parser suite: each test writes a temporary `.test.yaml` document
/// and parses it through [`crate::document::parse_scenario_document`].
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::PartnerFault;
    use crate::document::{DocError, ScenarioDocument, parse_scenario_document};

    /// Writes `text` to a fresh temporary `case.test.yaml` and parses
    /// it. Mirrors the helper of `doc_parse_test` so each parse-test
    /// module stays self-contained.
    fn parse_case(text: &str) -> Result<ScenarioDocument, DocError> {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("case.test.yaml");
        std::fs::write(&path, text).expect("write case file");
        parse_scenario_document(&path)
    }

    #[test]
    fn partners_section_parses() {
        let doc = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 201
      body:
        id: ord-7
"#,
        )
        .expect("parse must succeed");
        let partners = doc.partners.expect("partners map must be present");
        let scripts = partners
            .get("http://127.0.0.1:0/orders")
            .expect("the endpoint key must survive as the entry key");
        assert_eq!(scripts.len(), 1, "the entry must carry one script");
        let script = &scripts[0];
        assert_eq!(script.method.as_deref(), Some("POST"));
        assert_eq!(script.path.as_deref(), Some("/orders"));
        let response = script
            .response
            .as_ref()
            .expect("the script must carry a response");
        assert_eq!(response.status, Some(201));
        let body = response
            .body
            .as_ref()
            .expect("the script must carry a body");
        assert_eq!(
            body.get("id"),
            Some(&camel_api::Value::String("ord-7".to_string())),
            "the body must keep the id"
        );
    }

    #[test]
    fn partners_unknown_key_is_doc_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    responsez:
      status: 201
"#,
        )
        .expect_err("parse must fail");
        assert!(
            err.to_string().contains("responsez"),
            "error must name the offending key: {err}"
        );
    }

    #[test]
    fn partners_absent_keeps_none() {
        let doc = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
        )
        .expect("parse must succeed");
        assert!(doc.partners.is_none(), "absent partners must stay None");
    }

    #[test]
    fn partners_status_out_of_range_rejected() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - response:
      status: 999
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("999"),
            "error must name the offending status: {rendered}"
        );
    }

    #[test]
    fn times_zero_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - times: 0
    response:
      status: 201
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("times"),
            "error must name the `times` field: {rendered}"
        );
    }

    #[test]
    fn times_over_u32_max_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - times: 4294967296
    response:
      status: 201
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("times"),
            "error must name the `times` field: {rendered}"
        );
        assert!(
            rendered.contains("1-4294967295"),
            "error must name the valid range: {rendered}"
        );
    }

    #[test]
    fn both_response_and_fault_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - response:
      status: 201
    fault: close
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("response") && rendered.contains("fault"),
            "error must name both fields: {rendered}"
        );
    }

    #[test]
    fn neither_response_nor_fault_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("response") && rendered.contains("fault"),
            "error must name both missing fields: {rendered}"
        );
    }

    #[test]
    fn unknown_fault_name_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - fault: reset
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("fault") && rendered.contains("reset"),
            "error must name the `fault` field and the value: {rendered}"
        );
    }

    #[test]
    fn bad_delay_is_load_error() {
        let humantime_error = humantime::parse_duration("500xyz")
            .expect_err("500xyz must not parse as a duration")
            .to_string();
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - delay: 500xyz
    response:
      status: 201
"#,
        )
        .expect_err("parse must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("http://127.0.0.1:0/orders"),
            "error must name the entry key: {rendered}"
        );
        assert!(
            rendered.contains("delay"),
            "error must name the `delay` field: {rendered}"
        );
        assert!(
            rendered.contains(&humantime_error),
            "error must carry the humantime error text: {rendered}"
        );
    }

    #[test]
    #[cfg(feature = "http")]
    fn partner_client_body_parity() {
        use crate::adapters::http::value_to_wire;

        // The partner body vocabulary mirrors the client send path;
        // every expectation is a literal byte constant, never derived
        // from the function under test.
        let cases: Vec<(camel_api::Value, &[u8])> = vec![
            // A string serves as exact raw bytes: the inner quote is
            // not escaped, no quotes surround the body.
            (serde_json::json!("a\"b"), b"a\"b"),
            // Null serves empty.
            (serde_json::Value::Null, b""),
            // Structured values serve as compact JSON.
            (serde_json::json!({"k": 1}), b"{\"k\":1}"),
            (serde_json::json!([1, 2]), b"[1,2]"),
        ];
        for (value, wire) in &cases {
            assert_eq!(
                &value_to_wire(value),
                wire,
                "body {value} must serve as the exact client-path bytes"
            );
        }
    }

    #[test]
    fn times_delay_fault_parse() {
        let doc = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/orders:
  - times: 2
    delay: 300ms
    fault: close
  - method: GET
    response:
      status: 204
"#,
        )
        .expect("parse must succeed");
        let partners = doc.partners.expect("partners map must be present");
        let scripts = partners
            .get("http://127.0.0.1:0/orders")
            .expect("the endpoint key must survive as the entry key");
        assert_eq!(scripts.len(), 2, "both entries must survive");
        let fault_script = &scripts[0];
        assert_eq!(fault_script.times, Some(2), "times must parse as u32");
        assert_eq!(
            fault_script.delay,
            Some(Duration::from_millis(300)),
            "delay must parse as a humantime duration"
        );
        assert_eq!(fault_script.fault, Some(PartnerFault::Close));
        assert!(
            fault_script.response.is_none(),
            "a fault entry carries no response"
        );
        let response_script = &scripts[1];
        assert_eq!(response_script.times, None);
        assert_eq!(response_script.delay, None);
        assert_eq!(response_script.fault, None);
        assert_eq!(
            response_script
                .response
                .as_ref()
                .expect("a plain entry carries a response")
                .status,
            Some(204)
        );
    }
}
