//! Local broker-failure diagnostics (task 8.1).
//!
//! Two responsibilities, both remote-text-free by construction:
//!
//! - [`ensure_tls_capability`]: the startup `broker-tls-v1` capability gate.
//! - [`render_failure_message`]: actionable operator text per
//!   `crate::proto::BrokerFailureClass`, built only from the class
//!   discriminant and operator-owned configuration strings.
//!
//! Remote bridge text never reaches these functions: the wire carries only the
//! failure class, and every message below is authored in this file.

use camel_bridge::process::BridgeError;

use crate::config::redact_broker_url;

/// Missing-capability clause shared by the gate error and the permanent
/// startup error the call site surfaces. The surfaced message is
/// `bridge {version_or_path} {MISSING_BROKER_TLS_CAPABILITY}` where
/// `version_or_path` is the launched bridge binary path the call site tracks.
pub(crate) const MISSING_BROKER_TLS_CAPABILITY: &str = "lacks broker-tls-v1; use the pinned bridge";

/// Missing-capability clause for the message-selector gate (task 1.2). The
/// surfaced message is `bridge {pinned_version} {missing_message_selector_capability()}`
/// where `pinned_version` is `crate::BRIDGE_VERSION`, the build-pinned bridge
/// version the call site substitutes (same shape as the TLS gate text).
pub(crate) fn missing_message_selector_capability() -> String {
    format!(
        "lacks message-selector-v1; requires a jms bridge >= {}",
        crate::BRIDGE_VERSION
    )
}

/// Last broker failure observed from a bridge `Health` response (task 8.1).
///
/// Stored on the broker handle by the health monitor and read by the consumer
/// error path. All three strings are local, operator-owned values: the class
/// is the wire discriminant, the expected identity comes from the broker's
/// `tls.server_name` or URL host, and the connected host is the redacted
/// broker URL. No remote text is ever carried here.
#[derive(Clone, Debug)]
pub struct LastBrokerFailure {
    /// `BrokerFailureClass` discriminant; `0` (UNSPECIFIED) means no class.
    pub failure_class: i32,
    /// Identity the certificate must name: `tls.server_name` when configured,
    /// else the broker URL host.
    pub expected_identity: String,
    /// Broker URL passed through `redact_broker_url`.
    pub connected_host: String,
}

/// Startup capability gate for broker TLS (task 8.1).
///
/// A broker configured with a `tls` table requires a bridge build that reports
/// the `broker-tls-v1` capability. An older bridge reports no capabilities
/// (proto3 default: empty), which fails closed. When no `tls` table is
/// configured, a missing capability is not fatal and the gate returns `Ok`.
///
/// The error is a permanent (config-class) rejection
/// ([`BridgeError::PermanentFailure`]): call sites kill the launched bridge and
/// map it to their permanent-error channel without retry.
pub fn ensure_tls_capability(
    capabilities: &[String],
    tls_configured: bool,
) -> Result<(), BridgeError> {
    if tls_configured && !capabilities.iter().any(|c| c == "broker-tls-v1") {
        return Err(BridgeError::PermanentFailure(format!(
            "bridge {MISSING_BROKER_TLS_CAPABILITY}"
        )));
    }
    Ok(())
}

/// Capability gate for message selectors (task 1.2).
///
/// A route configuring a message selector requires a bridge build that reports
/// the `message-selector-v1` capability. An older bridge reports no
/// capabilities (proto3 default: empty), which fails closed. Without a
/// configured selector a missing capability is not fatal and the gate returns
/// `Ok`.
///
/// Same error shape as [`ensure_tls_capability`]: a permanent (config-class)
/// rejection ([`BridgeError::PermanentFailure`]) whose text names the missing
/// capability and the required bridge version. Call sites kill the launched
/// bridge and map it to their permanent-error channel without retry.
pub(crate) fn ensure_selector_capability(
    capabilities: &[String],
    selector_required: bool,
) -> Result<(), BridgeError> {
    if selector_required && !capabilities.iter().any(|c| c == "message-selector-v1") {
        return Err(BridgeError::PermanentFailure(format!(
            "bridge {}", missing_message_selector_capability()
        )));
    }
    Ok(())
}

/// Startup capability gate for message selectors (task 1.2).
///
/// The shared startup-gate decision seam: the real launch path
/// (`start_bridge_process_with_config`) calls this over the freshly collected
/// startup Health evidence and surfaces the error as its permanent
/// (config-class) rejection. The text is the three-fragment diagnostic — the
/// missing capability `message-selector-v1`, the required bridge version
/// the required bridge version (same as the pinned one), and the pinned bridge version the component was built for
/// (`crate::BRIDGE_VERSION`, the same substitution shape as the TLS startup
/// text). Evidence is re-collected per launch, so a previous bridge instance
/// never authorizes its replacement.
pub(crate) fn evaluate_startup_selector_gate(
    capabilities: &[String],
    selector_required: bool,
) -> Result<(), BridgeError> {
    if selector_required && !capabilities.iter().any(|c| c == "message-selector-v1") {
        return Err(BridgeError::PermanentFailure(format!(
            "bridge {} {}",
            crate::BRIDGE_VERSION,
            missing_message_selector_capability()
        )));
    }
    Ok(())
}

/// Renders a local, actionable message for one broker failure class (task
/// 8.1).
///
/// `failure_class` is the `BrokerFailureClass` wire discriminant; unknown
/// values render as the OTHER catch-all. `expected_identity` is the name the
/// certificate must carry (`tls.server_name` when configured, else the URL
/// host) and `connected_host` is the broker URL, redacted here through
/// `redact_broker_url`. Only the enum discriminant and these config strings
/// enter the message: no remote bridge text is ever included. The
/// `TLS_HOSTNAME` rendering is the fixed D1 contract — broker name, expected
/// identity, redacted connected host, and both remediations.
pub fn render_failure_message(
    failure_class: i32,
    broker_name: &str,
    expected_identity: &str,
    connected_host: &str,
) -> String {
    use crate::proto::BrokerFailureClass as Class;

    let connected = redact_broker_url(connected_host);
    match failure_class {
        c if c == Class::TlsHostname as i32 => format!(
            "broker '{broker_name}': TLS certificate does not match '{expected_identity}' \
             (connected to {connected}). Use a broker address the certificate names \
             (DNS or IP SAN), or set brokers.{broker_name}.tls.server_name to a DNS name \
             in the certificate."
        ),
        c if c == Class::Network as i32 => format!(
            "broker '{broker_name}': broker unreachable; check the broker address and \
             network reachability"
        ),
        c if c == Class::TlsTrust as i32 => format!(
            "broker '{broker_name}': TLS certificate trust check failed; check ca_cert_path \
             against the broker's CA"
        ),
        c if c == Class::TlsClientAuth as i32 => format!(
            "broker '{broker_name}': broker requires a client certificate; configure the \
             client_cert/client_key pair"
        ),
        c if c == Class::TlsProtocol as i32 => format!(
            "broker '{broker_name}': TLS protocol/cipher mismatch; the bridge uses JDK defaults"
        ),
        c if c == Class::TlsPlaintextPeer as i32 => format!(
            "broker '{broker_name}': the connected port is not TLS; use a TLS port for this broker"
        ),
        c if c == Class::UnsupportedTransport as i32 => {
            format!("broker '{broker_name}': the broker URL scheme has no transport; check the URL")
        }
        c if c == Class::Auth as i32 => {
            format!(
                "broker '{broker_name}': broker authentication failed; check the configured \
                 broker credentials"
            )
        }
        c if c == Class::ClientIdInUse as i32 => format!(
            "broker '{broker_name}': another connection holds the client id; stop it or use a \
             different client id"
        ),
        // OTHER (10), UNSPECIFIED (0), and unknown discriminants share the
        // catch-all rendering; the renderer never panics on the wire value.
        _ => format!("broker '{broker_name}': broker connection failed; see bridge logs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::BrokerFailureClass;

    /// A TLS-configured broker whose Health response reports no capabilities
    /// fails the gate with a permanent error naming `broker-tls-v1`.
    #[test]
    fn gate_kills_bridge_when_capability_missing() {
        let result = ensure_tls_capability(&[], true);
        let err = result.expect_err("empty capabilities must fail when tls is configured");
        assert!(
            err.to_string().contains("broker-tls-v1"),
            "gate error must name the missing capability: {err}"
        );
        assert!(
            matches!(err, BridgeError::PermanentFailure(_)),
            "the gate must classify as a permanent (config-class) rejection: {err:?}"
        );
    }

    #[test]
    fn gate_passes_when_capability_present() {
        let caps = vec!["broker-tls-v1".to_string()];
        assert!(
            ensure_tls_capability(&caps, true).is_ok(),
            "broker-tls-v1 present must pass the gate"
        );
    }

    #[test]
    fn gate_skipped_without_tls() {
        assert!(
            ensure_tls_capability(&[], false).is_ok(),
            "a missing capability is not fatal without a tls table"
        );
    }

    /// D1 contract: the TLS_HOSTNAME rendering names the expected identity,
    /// the connected host, and both remediations.
    #[test]
    fn tls_hostname_message_names_both_remedies() {
        let msg = render_failure_message(
            BrokerFailureClass::TlsHostname as i32,
            "main",
            "mq.internal",
            "ssl://10.0.0.5:61617",
        );
        assert!(msg.contains("mq.internal"), "must name the identity: {msg}");
        assert!(msg.contains("10.0.0.5"), "must name the host: {msg}");
        assert!(
            msg.contains("Use a broker address the certificate names"),
            "must name the certificate-naming remediation: {msg}"
        );
        assert!(
            msg.contains("brokers.main.tls.server_name to a DNS name in the certificate"),
            "must name the server_name remediation: {msg}"
        );
    }

    /// The TLS_TRUST rendering carries the local ca_cert_path remedy and
    /// nothing that could be remote text (no JDK/bridge markers, no leftover
    /// format placeholders).
    #[test]
    fn tls_trust_message_is_local_only() {
        let msg = render_failure_message(
            BrokerFailureClass::TlsTrust as i32,
            "main",
            "mq.internal",
            "ssl://10.0.0.5:61617",
        );
        assert!(
            msg.contains("ca_cert_path"),
            "must name the ca_cert_path remedy: {msg}"
        );
        assert!(!msg.contains('%'), "no format placeholder may leak: {msg}");
        for marker in ["Exception", "javax.", "sun.security", "alert"] {
            assert!(
                !msg.contains(marker),
                "no remote-text marker '{marker}' may appear: {msg}"
            );
        }
    }

    /// The connected host passes through `redact_broker_url`: only the
    /// redacted form reaches the message.
    #[test]
    fn redacted_host_in_message() {
        let msg = render_failure_message(
            BrokerFailureClass::TlsHostname as i32,
            "main",
            "mq.internal",
            "ssl://10.0.0.5:61617?password=secret-value",
        );
        assert!(
            msg.contains("password=<redacted>"),
            "must contain the redacted form: {msg}"
        );
        assert!(
            !msg.contains("secret-value"),
            "must not contain the raw credential: {msg}"
        );
    }

    /// Task 1.2: the `message-selector-v1` gate is a no-op without a
    /// configured selector, passes when the capability is advertised, and
    /// fails closed (same permanent error shape as the TLS gate) with text
    /// naming the capability and the required bridge version.
    #[test]
    fn ensure_selector_capability_unit() {
        assert!(
            ensure_selector_capability(&[], false).is_ok(),
            "a missing capability is not fatal without a configured selector"
        );
        assert!(
            ensure_selector_capability(&["message-selector-v1".to_string()], true).is_ok(),
            "message-selector-v1 present must pass the gate"
        );
        let err = ensure_selector_capability(&["broker-tls-v1".to_string()], true)
            .expect_err("a configured selector requires message-selector-v1");
        let text = err.to_string();
        assert!(
            text.contains("message-selector-v1"),
            "gate error must name the missing capability: {text}"
        );
        assert!(
            text.contains(crate::BRIDGE_VERSION),
            "gate error must name the required bridge version: {text}"
        );
        assert!(
            matches!(err, BridgeError::PermanentFailure(_)),
            "the gate must classify as a permanent (config-class) rejection: {err:?}"
        );
    }

    /// Unknown discriminants render the OTHER catch-all without panicking.
    #[test]
    fn unknown_class_renders_other() {
        let unknown = render_failure_message(99, "main", "mq.internal", "ssl://10.0.0.5:61617");
        let other = render_failure_message(
            BrokerFailureClass::Other as i32,
            "main",
            "mq.internal",
            "ssl://10.0.0.5:61617",
        );
        assert_eq!(unknown, other, "class 99 must render as OTHER");
        assert!(
            unknown.contains("see bridge logs"),
            "OTHER must point at the bridge logs: {unknown}"
        );
    }
}
