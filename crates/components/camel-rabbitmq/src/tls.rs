//! RabbitMQ TLS configuration types and boot validation.
//!
//! Parsing lives here; transport wiring lives in `connection.rs`. Boot
//! validation is fail-closed: `server_name` is rejected (lapin 4.12.0 derives
//! SNI and certificate verification from the URL host), `tls.*` requires an
//! `amqps://` URL, and every configured `ca_cert` is read and DER-validated so
//! a broken trust anchor cannot reach the transport.

use camel_component_api::CamelError;
use rustls_pki_types::CertificateDer;
use serde::Deserialize;

use crate::config::RabbitBrokerConfig;
use crate::config::RabbitComponentConfig;

/// Verbatim `tls.server_name` rejection template; `{broker}` is the map key.
pub const SERVER_NAME_UNSUPPORTED_ERROR: &str = "rabbitmq broker '{broker}': tls.server_name is unsupported by lapin 4.12.0; TLS SNI and certificate verification use the broker URL host; remove tls.server_name.";

fn server_name_unsupported(broker: &str) -> CamelError {
    CamelError::Config(SERVER_NAME_UNSUPPORTED_ERROR.replace("{broker}", broker))
}

fn tls_requires_amqps(broker: &str, detail: &str) -> CamelError {
    CamelError::Config(format!(
        "rabbitmq broker '{broker}': tls.* parameters require an amqps:// URL; {detail}"
    ))
}

fn ca_load_failed(broker: &str, path: &str, reason: &str) -> CamelError {
    CamelError::Config(format!(
        "rabbitmq broker '{broker}': failed to load tls.ca_cert from '{path}': {reason}"
    ))
}

/// Map a PEM parse failure to a fixed, payload-free reason.
///
/// `rustls_pki_types::pem::Error`'s `Display` embeds the offending source bytes
/// (`IllegalSectionStart.line`, `MissingSectionEnd.end_marker`), so forwarding
/// it would leak PEM contents into diagnostics. Match the concrete variants and
/// return static strings only.
fn pem_error_reason(error: &rustls_pki_types::pem::Error) -> &'static str {
    use rustls_pki_types::pem::Error;

    match error {
        Error::IllegalSectionStart { .. } => "invalid PEM framing",
        Error::MissingSectionEnd { .. } => "missing PEM end marker",
        Error::Base64Decode(_) => "invalid base64 in PEM body",
        Error::SectionTooLarge => "PEM section too large",
        Error::NoItemsFound => "no certificates found in file",
        Error::Io(_) => "I/O error reading PEM",
        // `Error` is `#[non_exhaustive]`: never forward unknown payloads.
        _ => "invalid PEM",
    }
}

/// Validate one broker's TLS configuration, fail-closed.
///
/// Precedence is binding: `server_name` is rejected first and unconditionally
/// (even when empty or equal to the URL host); then any `tls` section requires
/// an `amqps://` URL. Certificate loading is a separate step so programmatic
/// construction can validate cheaply.
pub fn validate_broker(name: &str, broker: &RabbitBrokerConfig) -> Result<(), CamelError> {
    let Some(tls) = broker.tls.as_ref() else {
        return Ok(());
    };

    if tls.server_name.is_some() {
        return Err(server_name_unsupported(name));
    }

    match url::Url::parse(&broker.url) {
        Ok(parsed) if parsed.scheme() == "amqps" => Ok(()),
        Ok(parsed) => Err(tls_requires_amqps(
            name,
            &format!("the URL scheme is '{}'", parsed.scheme()),
        )),
        Err(_) => Err(tls_requires_amqps(
            name,
            "the broker URL is not a valid URL",
        )),
    }
}

/// Read and validate a `tls.ca_cert` PEM bundle.
///
/// Every certificate must decode as PEM and parse as an X.509 trust anchor;
/// the original file contents are returned because tcp-stream consumes a PEM
/// string through `OwnedTLSConfig.cert_chain`. Errors name the broker and the
/// configured path only — never PEM contents, credentials, or the URL.
pub fn load_ca_bundle(name: &str, ca_cert: &str) -> Result<String, CamelError> {
    use rustls_pki_types::pem::PemObject;

    let contents = std::fs::read_to_string(ca_cert)
        .map_err(|error| ca_load_failed(name, ca_cert, &error.to_string()))?;

    let mut certificates = 0usize;
    for item in CertificateDer::pem_slice_iter(contents.as_bytes()) {
        let certificate =
            item.map_err(|error| ca_load_failed(name, ca_cert, pem_error_reason(&error)))?;
        webpki::anchor_from_trusted_cert(&certificate)
            .map_err(|_| ca_load_failed(name, ca_cert, "malformed certificate DER"))?;
        certificates += 1;
    }

    if certificates == 0 {
        return Err(ca_load_failed(
            name,
            ca_cert,
            "no certificates found in file",
        ));
    }

    Ok(contents)
}

/// Validate every configured broker through [`validate_broker`] and load every
/// configured `ca_cert` bundle, so an unused broker cannot escape boot checks.
///
/// Brokers are visited in name order so multi-broker failures are deterministic.
pub fn validate(config: &RabbitComponentConfig) -> Result<(), CamelError> {
    let mut brokers: Vec<(&str, &RabbitBrokerConfig)> = config
        .brokers
        .iter()
        .map(|(name, broker)| (name.as_str(), broker))
        .collect();
    brokers.sort_unstable_by_key(|(name, _)| *name);

    for (name, broker) in brokers {
        validate_broker(name, broker)?;
        if let Some(ca_cert) = broker.tls.as_ref().and_then(|tls| tls.ca_cert.as_deref()) {
            load_ca_bundle(name, ca_cert)?;
        }
    }

    Ok(())
}

/// One `[components.rabbitmq.brokers.<name>.tls]` section.
///
/// `ca_cert` is a PEM path for a custom trust anchor (valid only with
/// `amqps://`). `server_name` is rejected at boot: lapin 4.12.0 derives the
/// TLS SNI and certificate-verification name from the broker URL host and
/// exposes no override. `deny_unknown_fields` keeps unknown `tls.*` keys
/// failing closed.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RabbitTlsConfig {
    /// PEM path for a custom trust anchor; valid only with `amqps://`.
    pub ca_cert: Option<String>,
    /// Rejected at boot: unsupported by lapin 4.12.0 (no SNI override seam).
    pub server_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::config::RabbitBrokerConfig;
    use crate::config::RabbitComponentConfig;

    fn broker(url: &str, tls: Option<RabbitTlsConfig>) -> RabbitBrokerConfig {
        RabbitBrokerConfig {
            url: url.to_string(),
            username: None,
            password: None,
            vhost: None,
            tls,
        }
    }

    fn tls_config(ca_cert: Option<&str>, server_name: Option<&str>) -> RabbitTlsConfig {
        RabbitTlsConfig {
            ca_cert: ca_cert.map(str::to_string),
            server_name: server_name.map(str::to_string),
        }
    }

    fn component(name: &str, broker: RabbitBrokerConfig) -> RabbitComponentConfig {
        let mut brokers = std::collections::HashMap::new();
        brokers.insert(name.to_string(), broker);
        RabbitComponentConfig {
            brokers,
            reconnect: None,
        }
    }

    fn config_error<T>(result: Result<T, CamelError>) -> String {
        match result {
            Ok(_) => panic!("expected a config error, got Ok"),
            Err(CamelError::Config(message)) => message,
            Err(other) => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    fn temp_dir() -> TempDir {
        tempfile::tempdir().expect("test temp dir must be creatable")
    }

    fn write_ca(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).expect("test CA file must be writable");
        path
    }

    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls/test-ca.pem")
    }

    fn utf8(path: &Path) -> &str {
        path.to_str().expect("test path must be UTF-8")
    }

    #[test]
    fn tls_section_parses_ca_cert_and_server_name() {
        let raw = "url = \"amqps://host:5671\"\n\
                   [tls]\n\
                   ca_cert = \"/tmp/ca.pem\"\n\
                   server_name = \"broker.internal\"\n";
        let parsed =
            toml::from_str::<RabbitBrokerConfig>(raw).expect("a supported tls section must parse");
        let tls = parsed.tls.expect("tls section must be present");
        assert_eq!(tls.ca_cert.as_deref(), Some("/tmp/ca.pem"));
        assert_eq!(tls.server_name.as_deref(), Some("broker.internal"));
    }

    #[test]
    fn tls_section_parses_ca_cert_only() {
        let raw = "url = \"amqps://host:5671\"\n\
                   [tls]\n\
                   ca_cert = \"/tmp/ca.pem\"\n";
        let parsed = toml::from_str::<RabbitBrokerConfig>(raw).expect("ca_cert alone must parse");
        let tls = parsed.tls.expect("tls section must be present");
        assert_eq!(tls.ca_cert.as_deref(), Some("/tmp/ca.pem"));
        assert_eq!(tls.server_name, None);
    }

    #[test]
    fn tls_section_rejects_unknown_key() {
        let raw = "url = \"amqps://host:5671\"\n\
                   [tls]\n\
                   ca_cert = \"/tmp/ca.pem\"\n\
                   skip_verify = true\n";
        let parsed = toml::from_str::<RabbitBrokerConfig>(raw);
        assert!(
            parsed.is_err(),
            "an unknown tls sub-key must fail closed via deny_unknown_fields"
        );
    }

    #[test]
    fn broker_without_tls_section_parses() {
        let raw = "url = \"amqp://localhost:5672\"\n\
                   username = \"guest\"\n\
                   password = \"guest\"\n\
                   vhost = \"/\"\n";
        let parsed = toml::from_str::<RabbitBrokerConfig>(raw).expect("plain broker must parse");
        assert!(parsed.tls.is_none(), "tls must default to None");
    }

    #[test]
    fn server_name_rejected_with_verbatim_text_before_other_checks() {
        let broker = broker(
            "amqp://h:5672",
            Some(tls_config(Some("/nonexistent/ca.pem"), Some(""))),
        );
        let message = config_error(validate(&component("payments", broker)));
        let expected = SERVER_NAME_UNSUPPORTED_ERROR.replace("{broker}", "payments");
        assert_eq!(message, expected);
        assert!(
            !message.contains("/nonexistent/ca.pem"),
            "server_name rejection must not leak the CA path: {message}"
        );
        assert!(
            !message.contains("amqp://"),
            "server_name rejection must not leak the broker URL: {message}"
        );
    }

    #[test]
    fn server_name_rejected_even_when_equal_to_host_and_amqps() {
        let ca = fixture_path();
        let broker = broker(
            "amqps://broker.internal:5671",
            Some(tls_config(Some(utf8(&ca)), Some("broker.internal"))),
        );
        let message = config_error(validate(&component("payments", broker)));
        assert_eq!(
            message,
            SERVER_NAME_UNSUPPORTED_ERROR.replace("{broker}", "payments")
        );
    }

    #[test]
    fn tls_on_amqp_url_rejected_naming_scheme() {
        let broker = broker(
            "amqp://h:5672",
            Some(tls_config(Some("ignored-because-not-loaded"), None)),
        );
        let message = config_error(validate(&component("orders", broker)));
        assert!(
            message.contains("require an amqps:// URL"),
            "got: {message}"
        );
        assert!(
            message.contains("the URL scheme is 'amqp'"),
            "got: {message}"
        );
        assert!(message.contains("orders"), "got: {message}");
        assert!(
            !message.contains("amqp://h:5672"),
            "the broker URL must not be echoed: {message}"
        );
        assert!(
            !message.contains("ignored-because-not-loaded"),
            "the ca_cert value must not be echoed: {message}"
        );
    }

    #[test]
    fn tls_on_unparseable_url_rejected() {
        let broker = broker("not a url", Some(tls_config(Some("x"), None)));
        let message = config_error(validate(&component("orders", broker)));
        assert!(message.contains("not a valid URL"), "got: {message}");
    }

    #[test]
    fn missing_ca_file_fails_closed_naming_broker() {
        let dir = temp_dir();
        let path = dir.path().join("missing.pem");
        let broker = broker("amqps://h:5671", Some(tls_config(Some(utf8(&path)), None)));
        let message = config_error(validate(&component("payments", broker)));
        assert!(message.contains("payments"), "got: {message}");
        assert!(message.contains(utf8(&path)), "got: {message}");
        assert!(
            message.contains("failed to load tls.ca_cert"),
            "got: {message}"
        );
    }

    #[test]
    fn non_pem_ca_file_fails_closed() {
        let dir = temp_dir();
        let path = write_ca(&dir, "not-pem.pem", "this is not pem\n");
        let broker = broker("amqps://h:5671", Some(tls_config(Some(utf8(&path)), None)));
        let message = config_error(validate(&component("payments", broker)));
        assert!(message.contains("no certificates"), "got: {message}");
        assert!(message.contains("payments"), "got: {message}");
    }

    #[test]
    fn zero_certificate_pem_fails_closed() {
        let dir = temp_dir();
        let path = write_ca(
            &dir,
            "private-key.pem",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
        );
        let broker = broker("amqps://h:5671", Some(tls_config(Some(utf8(&path)), None)));
        let message = config_error(validate(&component("payments", broker)));
        assert!(message.contains("payments"), "got: {message}");
        assert!(message.contains("no certificates"), "got: {message}");
    }

    #[test]
    fn malformed_der_ca_fails_closed() {
        let dir = temp_dir();
        let path = write_ca(
            &dir,
            "malformed-der.pem",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        );
        let broker = broker("amqps://h:5671", Some(tls_config(Some(utf8(&path)), None)));
        let message = config_error(validate(&component("payments", broker)));
        assert!(
            message.contains("malformed certificate DER"),
            "got: {message}"
        );
        assert!(message.contains("payments"), "got: {message}");
    }

    #[test]
    fn pem_error_does_not_disclose_file_contents() {
        // Malformed section start: the rustls Display would echo this line.
        let dir = temp_dir();
        let illegal = write_ca(
            &dir,
            "illegal-start.pem",
            "-----BEGIN secret-sentinel-XYZ\nAAAA\n-----END CERTIFICATE-----\n",
        );
        let message = config_error(load_ca_bundle("payments", utf8(&illegal)));
        assert!(message.contains("payments"), "got: {message}");
        assert!(message.contains("invalid PEM framing"), "got: {message}");
        assert!(
            !message.contains("secret-sentinel-XYZ"),
            "PEM line contents leaked into the error: {message}"
        );

        // Missing END marker: the body must not reach diagnostics either.
        let missing_end = write_ca(
            &dir,
            "missing-end.pem",
            "-----BEGIN CERTIFICATE-----\npayload-sentinel-ABC\n",
        );
        let message = config_error(load_ca_bundle("payments", utf8(&missing_end)));
        assert!(message.contains("payments"), "got: {message}");
        assert!(message.contains("missing PEM end marker"), "got: {message}");
        assert!(
            !message.contains("payload-sentinel-ABC"),
            "PEM body contents leaked into the error: {message}"
        );
    }

    #[test]
    fn valid_self_signed_ca_loads() {
        let path = fixture_path();
        let contents = std::fs::read_to_string(&path).expect("fixture CA must be readable");
        let loaded =
            load_ca_bundle("payments", utf8(&path)).expect("a valid self-signed CA must load");
        assert_eq!(loaded, contents);
    }
}
