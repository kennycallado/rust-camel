use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use camel_component_api::CamelError;
use percent_encoding::percent_decode_str;

use crate::schema_set::MAX_SCHEMA_ALIASES;

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaType {
    Xml,
    Json,
    Yaml,
    RelaxNg,
    Schematron,
}

/// Maximum number of schemas retained in the XSD bridge cache before eviction.
pub const DEFAULT_SCHEMA_CACHE_MAX_ENTRIES: usize = 256;
/// Default for `fail_on_null_body`.
pub const DEFAULT_FAIL_ON_NULL_BODY: bool = true;
/// Default for `fail_on_null_header`.
pub const DEFAULT_FAIL_ON_NULL_HEADER: bool = true;
/// Default for `validate_formats`.
pub const DEFAULT_VALIDATE_FORMATS: bool = true;

#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    pub schema_path: PathBuf,
    pub schema_type: SchemaType,
    /// Maximum allowed body size in bytes. Bodies exceeding this limit are
    /// rejected before validation begins. `None` means no limit.
    pub max_payload_bytes: Option<usize>,
    /// Maximum number of entries in the XSD bridge schema cache. When the
    /// cache exceeds this limit, all entries are evicted before inserting the
    /// new one. Only relevant for XSD validation.
    pub schema_cache_max_entries: usize,
    /// When `true` (default), reject exchanges whose body is empty/null.
    /// When `false`, empty bodies pass through without validation.
    pub fail_on_null_body: bool,
    /// When set, validate the value of this header instead of the body.
    pub header_name: Option<String>,
    /// When `true` (default) and `header_name` is set, reject exchanges where
    /// the header is missing. When `false`, missing headers pass through.
    pub fail_on_null_header: bool,
    /// `true` (default) enforces `format` keywords on JSON/YAML schemas;
    /// `false` restores annotation-only behavior.
    pub validate_formats: bool,
    /// Operator-supplied absolute-URI -> root-relative virtual path aliases,
    /// parsed from the single `schemaAliases` query parameter (a
    /// percent-encoded JSON object of string keys to string values). Empty
    /// when the parameter is absent.
    pub schema_aliases: BTreeMap<String, String>,
}

impl ValidatorConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let without_scheme = uri.strip_prefix("validator:").ok_or_else(|| {
            CamelError::InvalidUri(format!(
                "invalid validator URI: must start with 'validator:' — got '{uri}'"
            ))
        })?;

        let (path_str, query) = match without_scheme.find('?') {
            Some(idx) => (&without_scheme[..idx], Some(&without_scheme[idx + 1..])),
            None => (without_scheme, None),
        };

        if path_str.is_empty() {
            return Err(CamelError::InvalidUri(
                "validator URI must specify a schema path".to_string(),
            ));
        }

        validate_percent_encoding(path_str)?;
        let decoded_path = percent_decode_str(path_str)
            .decode_utf8()
            .map_err(|e| CamelError::InvalidUri(format!("invalid UTF-8 in path: {e}")))?;
        let schema_path = PathBuf::from(decoded_path.as_ref());

        // Audit 2026-08-31, F4-6: reject traversal components AFTER percent
        // decoding (a raw `%2e%2e/` must not bypass the check). Defense-in-depth
        // parity with camel-template; the schema path is operator config, but a
        // `..` here only ever means a bad deploy.
        if schema_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(CamelError::InvalidUri(
                "schema path contains '..' traversal component".to_string(),
            ));
        }

        let schema_type = if let Some(q) = query {
            let type_val = q.split('&').find_map(|kv| kv.strip_prefix("type="));
            match type_val {
                Some("xml") | Some("xml-schema") | Some("xsd") => SchemaType::Xml,
                Some("json") | Some("json-schema") => SchemaType::Json,
                Some("yaml") | Some("yaml-schema") => SchemaType::Yaml,
                Some("rng") | Some("relaxng") => SchemaType::RelaxNg,
                Some("sch") | Some("schematron") => SchemaType::Schematron,
                Some(other) => {
                    return Err(CamelError::InvalidUri(format!(
                        "unknown schema type '{other}'; expected xml, json, yaml, rng, or schematron"
                    )));
                }
                None => detect_type_from_extension(&schema_path)?,
            }
        } else {
            detect_type_from_extension(&schema_path)?
        };

        let mut max_payload_bytes: Option<usize> = None;
        let mut schema_cache_max_entries: usize = DEFAULT_SCHEMA_CACHE_MAX_ENTRIES;
        let mut fail_on_null_body: bool = DEFAULT_FAIL_ON_NULL_BODY;
        let mut header_name: Option<String> = None;
        let mut fail_on_null_header: bool = DEFAULT_FAIL_ON_NULL_HEADER;
        let validate_formats: bool = DEFAULT_VALIDATE_FORMATS;
        let mut schema_aliases: BTreeMap<String, String> = BTreeMap::new();
        let mut schema_aliases_seen = false;

        if let Some(q) = query {
            for kv in q.split('&') {
                if let Some(val) = kv.strip_prefix("maxPayloadBytes=") {
                    max_payload_bytes = Some(val.parse::<usize>().map_err(|e| {
                        CamelError::InvalidUri(format!("invalid maxPayloadBytes '{val}': {e}"))
                    })?);
                } else if let Some(val) = kv.strip_prefix("maxPayloadBytes:") {
                    max_payload_bytes = Some(val.parse::<usize>().map_err(|e| {
                        CamelError::InvalidUri(format!("invalid maxPayloadBytes '{val}': {e}"))
                    })?);
                } else if let Some(val) = kv.strip_prefix("schemaCacheMaxEntries=") {
                    schema_cache_max_entries = val.parse::<usize>().map_err(|e| {
                        CamelError::InvalidUri(format!(
                            "invalid schemaCacheMaxEntries '{val}': {e}"
                        ))
                    })?;
                } else if let Some(val) = kv.strip_prefix("failOnNullBody=") {
                    fail_on_null_body = val.parse::<bool>().map_err(|e| {
                        CamelError::InvalidUri(format!("invalid failOnNullBody '{val}': {e}"))
                    })?;
                } else if let Some(val) = kv.strip_prefix("headerName=") {
                    header_name = Some(val.to_string());
                } else if let Some(val) = kv.strip_prefix("failOnNullHeader=") {
                    fail_on_null_header = val.parse::<bool>().map_err(|e| {
                        CamelError::InvalidUri(format!("invalid failOnNullHeader '{val}': {e}"))
                    })?;
                } else if let Some(val) = kv.strip_prefix("schemaAliases=") {
                    if schema_aliases_seen {
                        return Err(CamelError::InvalidUri(
                            "duplicate 'schemaAliases' query parameter".to_string(),
                        ));
                    }
                    schema_aliases_seen = true;
                    schema_aliases = parse_schema_aliases(val)?;
                }
            }
        }

        Ok(ValidatorConfig {
            schema_path,
            schema_type,
            max_payload_bytes,
            schema_cache_max_entries,
            fail_on_null_body,
            header_name,
            fail_on_null_header,
            validate_formats,
            schema_aliases,
        })
    }

    /// Rejects a config whose public `schema_aliases` field was mutated past
    /// the [`MAX_SCHEMA_ALIASES`] cap.
    ///
    /// [`ValidatorConfig::from_uri`] already enforces the cap at parse time
    /// (via `schema_alias_cap_violation`); this is the config-level defense
    /// for a programmatically constructed [`ValidatorConfig`], whose fields are
    /// public and can be set to any length. It is called by
    /// `CompiledValidator::compile` before any schema-type dispatch or
    /// filesystem access, so an over-cap config fails with a bounded
    /// cap-naming error rather than a generic missing-schema error.
    pub fn validate_schema_aliases(&self) -> Result<(), CamelError> {
        match schema_alias_cap_violation(self.schema_aliases.len()) {
            Some(msg) => Err(CamelError::Config(msg)),
            None => Ok(()),
        }
    }
}

fn detect_type_from_extension(path: &Path) -> Result<SchemaType, CamelError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("xsd") => Ok(SchemaType::Xml),
        Some("json") => Ok(SchemaType::Json),
        Some("yaml") | Some("yml") => Ok(SchemaType::Yaml),
        Some("rng") | Some("rnc") => Ok(SchemaType::RelaxNg),
        Some("sch") => Ok(SchemaType::Schematron),
        ext => Err(CamelError::InvalidUri(format!(
            "cannot infer schema type from extension {ext:?}; use ?type=xml|json|yaml|rng|schematron"
        ))),
    }
}

/// Bounded diagnostic for an over-cap alias map, or `None` when within cap.
///
/// Single source for the cap wording and the one [`MAX_SCHEMA_ALIASES`]
/// constant, shared by the URI parse ([`CamelError::InvalidUri`]) and the
/// config-level guard ([`ValidatorConfig::validate_schema_aliases`],
/// [`CamelError::Config`]).
fn schema_alias_cap_violation(len: usize) -> Option<String> {
    (len > MAX_SCHEMA_ALIASES).then(|| {
        format!("invalid 'schemaAliases': {len} entries exceed the {MAX_SCHEMA_ALIASES}-entry cap")
    })
}

/// Parses the single `schemaAliases` query value: a percent-encoded JSON
/// object of string keys to string values.
///
/// Encoding is validated STRICTLY before the value is decoded exactly once, so
/// a malformed `%ZZ` fails closed with no partial decode. A double-encoded
/// value therefore decodes to a still-encoded string and fails JSON parsing
/// rather than being silently decoded twice. The shared strict scanner is
/// reused; its path-specific diagnostic is replaced by a bounded, option-named
/// error that never echoes the raw value.
fn parse_schema_aliases(raw: &str) -> Result<BTreeMap<String, String>, CamelError> {
    validate_percent_encoding(raw).map_err(|_| {
        CamelError::InvalidUri("invalid percent-encoding in 'schemaAliases'".to_string())
    })?;
    let decoded = percent_decode_str(raw).decode_utf8().map_err(|_| {
        CamelError::InvalidUri(
            "invalid 'schemaAliases': value is not valid UTF-8 after percent-decoding".to_string(),
        )
    })?;
    let aliases: BTreeMap<String, String> = serde_json::from_str(&decoded).map_err(|_| {
        CamelError::InvalidUri(
            "invalid 'schemaAliases': value must be a percent-encoded JSON object of \
             string keys to string values"
                .to_string(),
        )
    })?;
    if let Some(msg) = schema_alias_cap_violation(aliases.len()) {
        return Err(CamelError::InvalidUri(msg));
    }
    Ok(aliases)
}

fn validate_percent_encoding(input: &str) -> Result<(), CamelError> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(CamelError::InvalidUri(format!(
                    "invalid percent-encoding in path: '{input}'"
                )));
            }
            let is_hex = |b: u8| b.is_ascii_hexdigit();
            if !is_hex(bytes[i + 1]) || !is_hex(bytes[i + 2]) {
                return Err(CamelError::InvalidUri(format!(
                    "invalid percent-encoding in path: '{input}'"
                )));
            }
            i += 3;
            continue;
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_xml_from_xsd_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd").unwrap();
        assert_eq!(cfg.schema_path, PathBuf::from("schemas/order.xsd"));
        assert_eq!(cfg.schema_type, SchemaType::Xml);
    }

    #[test]
    fn detects_json_from_json_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.json").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Json);
    }

    #[test]
    fn detects_yaml_from_yaml_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.yaml").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Yaml);
    }

    #[test]
    fn detects_yaml_from_yml_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.yml").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Yaml);
    }

    #[test]
    fn type_param_overrides_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd?type=json").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Json);
    }

    #[test]
    fn wrong_scheme_errors() {
        assert!(ValidatorConfig::from_uri("timer:tick").is_err());
    }

    #[test]
    fn empty_path_errors() {
        assert!(ValidatorConfig::from_uri("validator:").is_err());
    }

    #[test]
    fn unknown_type_param_errors() {
        assert!(ValidatorConfig::from_uri("validator:schema.xsd?type=csv").is_err());
    }

    #[test]
    fn no_extension_no_type_param_errors() {
        assert!(ValidatorConfig::from_uri("validator:schema").is_err());
    }

    #[test]
    fn percent_encoded_path_decoded() {
        let cfg = ValidatorConfig::from_uri("validator:/path/to/my%20schema.xsd").unwrap();
        assert!(
            cfg.schema_path.to_str().unwrap().contains("my schema.xsd"),
            "expected decoded path, got {:?}",
            cfg.schema_path
        );
    }

    #[test]
    fn normal_path_unchanged() {
        let cfg = ValidatorConfig::from_uri("validator:/path/to/schema.xsd").unwrap();
        assert_eq!(cfg.schema_path, PathBuf::from("/path/to/schema.xsd"));
    }

    #[test]
    fn percent_encoded_multiple_segments() {
        let cfg = ValidatorConfig::from_uri("validator:/my%20dir/my%20file.xsd").unwrap();
        assert!(
            cfg.schema_path.to_str().unwrap().contains("my dir"),
            "expected decoded 'my dir', got {:?}",
            cfg.schema_path
        );
        assert!(
            cfg.schema_path.to_str().unwrap().contains("my file.xsd"),
            "expected decoded 'my file.xsd', got {:?}",
            cfg.schema_path
        );
    }

    #[test]
    fn percent_encoded_with_query_params() {
        let cfg =
            ValidatorConfig::from_uri("validator:/path/to/my%20schema.xsd?type=json").unwrap();
        assert!(
            cfg.schema_path.to_str().unwrap().contains("my schema.xsd"),
            "expected decoded path, got {:?}",
            cfg.schema_path
        );
        assert_eq!(cfg.schema_type, SchemaType::Json);
    }

    #[test]
    fn invalid_percent_encoding_errors() {
        // %ZZ is not a valid percent-encoding sequence
        let result = ValidatorConfig::from_uri("validator:/path/%ZZfile.xsd");
        assert!(
            matches!(result, Err(CamelError::InvalidUri(msg)) if msg.contains("percent-encoding"))
        );
    }

    #[test]
    fn test_fail_on_null_body_default_true() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd").unwrap();
        assert!(cfg.fail_on_null_body);
    }

    #[test]
    fn validate_formats_default_true() {
        let cfg = ValidatorConfig::from_uri("validator:/x.json?type=json").unwrap();
        assert!(cfg.validate_formats);
    }

    #[test]
    fn test_fail_on_null_body_false_passes_empty() {
        let cfg =
            ValidatorConfig::from_uri("validator:schemas/order.xsd?failOnNullBody=false").unwrap();
        assert!(!cfg.fail_on_null_body);
    }

    #[test]
    fn test_header_name_validation_option_parsed() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd?headerName=X-My-Header")
            .unwrap();
        assert_eq!(cfg.header_name.as_deref(), Some("X-My-Header"));
    }

    #[test]
    fn test_header_name_defaults_to_none() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd").unwrap();
        assert!(cfg.header_name.is_none());
    }

    #[test]
    fn test_fail_on_null_header_default_true() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd?headerName=X-H").unwrap();
        assert!(cfg.fail_on_null_header);
    }

    /// Audit 2026-08-31, F4-6: traversal rejected even percent-encoded.
    #[test]
    fn schema_path_rejects_traversal_raw_and_percent_encoded() {
        assert!(ValidatorConfig::from_uri("validator:../etc/passwd.xsd").is_err());
        assert!(ValidatorConfig::from_uri("validator:%2e%2e/etc/passwd.xsd").is_err());
        assert!(ValidatorConfig::from_uri("validator:a/../../b.xsd").is_err());
        assert!(ValidatorConfig::from_uri("validator:schemas/order.xsd").is_ok());
    }

    #[test]
    fn test_fail_on_null_header_false_passes_missing_header() {
        let cfg = ValidatorConfig::from_uri(
            "validator:schemas/order.xsd?headerName=X-H&failOnNullHeader=false",
        )
        .unwrap();
        assert!(!cfg.fail_on_null_header);
    }

    // ── schemaAliases query parameter ───────────────────────────────────

    #[test]
    fn schema_aliases_json_parsed() {
        let uri = "validator:schemas/order.xsd?schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fparent.xsd%22%3A%22a%2Fparent.xsd%22%7D";
        let cfg = ValidatorConfig::from_uri(uri).expect("percent-encoded JSON map must parse");
        assert_eq!(
            cfg.schema_aliases
                .get("http://example.com/parent.xsd")
                .map(String::as_str),
            Some("a/parent.xsd"),
            "decoded alias map must match the JSON object"
        );
        assert_eq!(cfg.schema_aliases.len(), 1);

        // Absent parameter leaves the map empty and existing formats unchanged.
        let plain = ValidatorConfig::from_uri("validator:schemas/order.xsd").unwrap();
        assert!(plain.schema_aliases.is_empty());
    }

    #[test]
    fn schema_aliases_duplicate_param_errors() {
        let uri = "validator:schemas/order.xsd?schemaAliases=%7B%7D&schemaAliases=%7B%7D";
        let result = ValidatorConfig::from_uri(uri);
        assert!(
            matches!(result, Err(CamelError::InvalidUri(_))),
            "repeated schemaAliases query parameter must be rejected"
        );
    }

    #[test]
    fn schema_aliases_malformed_json_errors() {
        // Table: not JSON at all, a JSON array, and a non-string member value.
        for value in ["not-json", "%5B%5D", "%7B%22urn%3Aa%22%3A1%7D"] {
            let uri = format!("validator:schemas/order.xsd?schemaAliases={value}");
            let result = ValidatorConfig::from_uri(&uri);
            assert!(
                matches!(result, Err(CamelError::InvalidUri(_))),
                "schemaAliases value '{value}' must be rejected"
            );
        }
    }

    #[test]
    fn schema_aliases_malformed_percent_errors() {
        let uri = "validator:schemas/order.xsd?schemaAliases=%7B%22urn%3Aa%22%3A%22%ZZ%22%7D";
        let result = ValidatorConfig::from_uri(uri);
        let Err(CamelError::InvalidUri(m)) = result else {
            panic!("expected Err(InvalidUri) for malformed percent-encoding");
        };
        assert!(
            m.contains("schemaAliases"),
            "error must name the schemaAliases option, got: {m}"
        );
    }

    #[test]
    fn schema_aliases_percent_decoded_exactly_once() {
        // Double-encoded: one decode yields the still-encoded `%7B...`, which
        // is not JSON. The parser must decode exactly once, never twice.
        let uri = "validator:schemas/order.xsd?schemaAliases=%257B%2522urn%253Aa%2522%253A%2522a%252Fparent.xsd%2522%257D";
        let result = ValidatorConfig::from_uri(uri);
        assert!(
            matches!(result, Err(CamelError::InvalidUri(_))),
            "a double-encoded schemaAliases value must fail after exactly one decode"
        );
    }

    #[test]
    fn schema_aliases_cap_enforced_at_parse() {
        // 129 entries exceeds the 128-entry alias cap.
        let mut obj = serde_json::Map::new();
        for i in 0..=crate::schema_set::MAX_SCHEMA_ALIASES {
            obj.insert(
                format!("urn:a{i}"),
                serde_json::Value::String("a/parent.xsd".to_string()),
            );
        }
        let json = serde_json::Value::Object(obj).to_string();
        let encoded =
            percent_encoding::utf8_percent_encode(&json, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let uri = format!("validator:schemas/order.xsd?schemaAliases={encoded}");
        let result = ValidatorConfig::from_uri(&uri);
        let Err(CamelError::InvalidUri(m)) = result else {
            panic!("expected Err(InvalidUri) for an over-cap alias map");
        };
        assert!(
            m.contains("128"),
            "error must name the 128-entry cap, got: {m}"
        );
    }
}
