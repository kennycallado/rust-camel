//! Local broker-failure diagnostics (task 8.1).
//!
//! Three responsibilities, all remote-text-free by construction:
//!
//! - [`CapabilityGate`]: the one descriptor for the JMS capability gates
//!   (TLS, selector, durable), with the per-capability constants
//!   [`CapabilityGate::TLS`], [`CapabilityGate::SELECTOR`],
//!   [`CapabilityGate::DURABLE`] (rc-06dy1).
//! - [`ensure_capability`] / [`evaluate_startup_capability_gate`]: the one
//!   attach and one startup evaluator every gate runs through.
//! - [`render_failure_message`]: actionable operator text per
//!   `crate::proto::BrokerFailureClass`, built only from the class
//!   discriminant and operator-owned configuration strings.
//!
//! Remote bridge text never reaches these functions: the wire carries only the
//! failure class, and every message below is authored in this file.

use camel_bridge::process::BridgeError;

use crate::config::redact_broker_url;

/// Minimum bridge release that advertises `durable-subscribers-v1` (task 1.3).
///
/// Distinct from [`crate::BRIDGE_VERSION`]: the pinned build may predate the
/// minimum that supports durable subscription identity, so the diagnostic
/// names both the requirement and the version the component was built for.
pub(crate) const DURABLE_SUBSCRIBERS_MIN_BRIDGE_VERSION: &str = "0.9.0";

/// One JMS capability gate descriptor (rc-06dy1).
///
/// The per-capability data every capability gate shares: the wire capability
/// string the bridge `Health` response must advertise, and the minimum bridge
/// release that advertises it. `None` means the requirement is the pinned
/// build itself. The runtime gate is capability advertisement (the bridge
/// either advertises the string or fails closed on an older build), so
/// `min_bridge_version` drives the authored diagnostic text, not a semver
/// compare.
///
/// The per-capability constants [`Self::TLS`], [`Self::SELECTOR`], and
/// [`Self::DURABLE`] replace the three shape-copied gate functions; a fourth
/// capability adds one constant, never a fourth copy of the version-compare +
/// message shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapabilityGate {
    /// The `Health` capability string the bridge must advertise.
    pub capability: &'static str,
    /// Minimum bridge release advertising the capability; `None` when the
    /// requirement is the pinned build itself.
    pub min_bridge_version: Option<&'static str>,
}

impl CapabilityGate {
    /// The startup `broker-tls-v1` gate (task 8.1): a broker with a `tls`
    /// table requires the pinned build to advertise the capability.
    pub(crate) const TLS: Self = Self {
        capability: "broker-tls-v1",
        min_bridge_version: None,
    };

    /// The `message-selector-v1` gate (task 1.2): a route configuring a
    /// selector requires a bridge at least at the pinned build.
    pub(crate) const SELECTOR: Self = Self {
        capability: "message-selector-v1",
        min_bridge_version: Some(crate::BRIDGE_VERSION),
    };

    /// The `durable-subscribers-v1` gate (task 1.3): a route configuring
    /// `clientId`/`subscriptionName` requires at least the release that
    /// supports durable subscription identity.
    pub(crate) const DURABLE: Self = Self {
        capability: "durable-subscribers-v1",
        min_bridge_version: Some(DURABLE_SUBSCRIBERS_MIN_BRIDGE_VERSION),
    };

    /// The missing-capability diagnostic clause, authored here and
    /// byte-identical to the pre-refactor per-capability copies:
    ///
    /// - TLS (`None`): `lacks {capability}; use the pinned bridge`
    /// - SELECTOR (`Some(min == pinned)`): `lacks {capability}; requires a
    ///   jms bridge >= {pinned}` — the selector's minimum IS the pinned
    ///   build, so the text names it once.
    /// - DURABLE (`Some(min != pinned)`): `lacks {capability}; requires a
    ///   jms bridge >= {min} (pinned {pinned})`.
    pub(crate) fn missing_clause(&self) -> String {
        match self.min_bridge_version {
            None => format!("lacks {}; use the pinned bridge", self.capability),
            Some(min) if min == crate::BRIDGE_VERSION => {
                format!(
                    "lacks {}; requires a jms bridge >= {}",
                    self.capability,
                    crate::BRIDGE_VERSION
                )
            }
            Some(min) => format!(
                "lacks {}; requires a jms bridge >= {} (pinned {})",
                self.capability,
                min,
                crate::BRIDGE_VERSION
            ),
        }
    }

    /// The support noun the consumer-attach verification texts use
    /// ("message-selector support", "durable-subscriber support"). Authored
    /// per capability: the noun does not derive mechanically from the wire
    /// string (`durable-subscribers-v1` verifies "durable-subscriber
    /// support").
    pub(crate) fn attach_support(&self) -> &'static str {
        match self.capability {
            "message-selector-v1" => "message-selector support",
            "durable-subscribers-v1" => "durable-subscriber support",
            _ => "capability",
        }
    }

    /// The tail of the consumer-attach timeout reason naming the requirement
    /// source: "the pinned jms bridge" for a pinned-build requirement
    /// (TLS, SELECTOR), "a jms bridge >= {min}" for the durable release
    /// floor. Authored per capability — the selector names the pinned build
    /// even though its minimum equals it, matching the pre-refactor text.
    pub(crate) fn attach_timeout_tail(&self) -> String {
        match self.capability {
            "durable-subscribers-v1" => {
                format!("a jms bridge >= {DURABLE_SUBSCRIBERS_MIN_BRIDGE_VERSION}")
            }
            _ => "the pinned jms bridge".to_string(),
        }
    }
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

/// The one attach evaluator behind every capability gate (rc-06dy1).
///
/// Fails closed when `required` and the bridge `Health` response does not
/// advertise `gate`'s capability (proto3 default: empty capabilities fails an
/// older bridge). The error is a permanent (config-class) rejection
/// ([`BridgeError::PermanentFailure`]) whose text is `bridge {clause}` with
/// the authored [`CapabilityGate::missing_clause`]; call sites kill the
/// launched bridge and map it to their permanent-error channel without retry.
pub(crate) fn ensure_capability(
    capabilities: &[String],
    gate: &CapabilityGate,
    required: bool,
) -> Result<(), BridgeError> {
    if required && !capabilities.iter().any(|c| c == gate.capability) {
        return Err(BridgeError::PermanentFailure(format!(
            "bridge {}",
            gate.missing_clause()
        )));
    }
    Ok(())
}

/// The one startup evaluator behind the selector/durable startup gates
/// (rc-06dy1).
///
/// The shared startup-gate decision seam: the real launch path
/// (`start_bridge_process_with_config`) calls this over the freshly collected
/// startup Health evidence and surfaces the error as its permanent
/// (config-class) rejection. The text is the three-fragment diagnostic — the
/// pinned bridge version `crate::BRIDGE_VERSION`, the missing capability, and
/// the required version. Evidence is re-collected per launch, so a previous
/// bridge instance never authorizes its replacement.
pub(crate) fn evaluate_startup_capability_gate(
    capabilities: &[String],
    gate: &CapabilityGate,
    required: bool,
) -> Result<(), BridgeError> {
    if required && !capabilities.iter().any(|c| c == gate.capability) {
        return Err(BridgeError::PermanentFailure(format!(
            "bridge {} {}",
            crate::BRIDGE_VERSION,
            gate.missing_clause()
        )));
    }
    Ok(())
}

/// Startup capability gate for broker TLS (task 8.1).
///
/// A broker configured with a `tls` table requires a bridge build that reports
/// the `broker-tls-v1` capability. An older bridge reports no capabilities
/// (proto3 default: empty), which fails closed. When no `tls` table is
/// configured, a missing capability is not fatal and the gate returns `Ok`.
///
/// One-line delegation to [`ensure_capability`] over
/// [`CapabilityGate::TLS`] (rc-06dy1); the surfaced message is unchanged.
/// The error is a permanent (config-class) rejection
/// ([`BridgeError::PermanentFailure`]): call sites kill the launched bridge and
/// map it to their permanent-error channel without retry.
pub fn ensure_tls_capability(
    capabilities: &[String],
    tls_configured: bool,
) -> Result<(), BridgeError> {
    ensure_capability(capabilities, &CapabilityGate::TLS, tls_configured)
}

/// Capability gate for message selectors (task 1.2).
///
/// A route configuring a message selector requires a bridge build that reports
/// the `message-selector-v1` capability. An older bridge reports no
/// capabilities (proto3 default: empty), which fails closed. Without a
/// configured selector a missing capability is not fatal and the gate returns
/// `Ok`.
///
/// One-line delegation to [`ensure_capability`] over
/// [`CapabilityGate::SELECTOR`] (rc-06dy1); the surfaced message is unchanged.
/// Kept for the message-pinning unit tests that call it directly.
#[cfg(test)]
pub(crate) fn ensure_selector_capability(
    capabilities: &[String],
    selector_required: bool,
) -> Result<(), BridgeError> {
    ensure_capability(capabilities, &CapabilityGate::SELECTOR, selector_required)
}

/// Capability gate for durable subscription identity (task 1.3).
///
/// A route configuring `clientId`/`subscriptionName` requires a bridge build
/// that reports the `durable-subscribers-v1` capability. An older bridge
/// reports no capabilities (proto3 default: empty), which fails closed. Without
/// a durable identity a missing capability is not fatal and the gate returns
/// `Ok`.
///
/// One-line delegation to [`ensure_capability`] over [`CapabilityGate::DURABLE`]
/// (rc-06dy1); the surfaced message is unchanged. Kept for the message-pinning
/// unit tests that call it directly.
#[cfg(test)]
pub(crate) fn ensure_durable_capability(
    capabilities: &[String],
    durable_required: bool,
) -> Result<(), BridgeError> {
    ensure_capability(capabilities, &CapabilityGate::DURABLE, durable_required)
}

/// Startup capability gate for message selectors (task 1.2).
///
/// One-line delegation to [`evaluate_startup_capability_gate`] over
/// [`CapabilityGate::SELECTOR`] (rc-06dy1); the surfaced message is unchanged.
/// Kept for the message-pinning unit tests that call it directly.
#[cfg(test)]
pub(crate) fn evaluate_startup_selector_gate(
    capabilities: &[String],
    selector_required: bool,
) -> Result<(), BridgeError> {
    evaluate_startup_capability_gate(capabilities, &CapabilityGate::SELECTOR, selector_required)
}

/// Startup capability gate for durable subscription identity (task 1.3).
///
/// One-line delegation to [`evaluate_startup_capability_gate`] over
/// [`CapabilityGate::DURABLE`] (rc-06dy1); the surfaced message is unchanged.
/// Kept for the message-pinning unit tests that call it directly.
#[cfg(test)]
pub(crate) fn evaluate_startup_durable_gate(
    capabilities: &[String],
    durable_required: bool,
) -> Result<(), BridgeError> {
    evaluate_startup_capability_gate(capabilities, &CapabilityGate::DURABLE, durable_required)
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

    // ── rc-06dy1: the descriptor table drives every gate ────────────────────

    /// One row per capability gate: the descriptor constant, its wire
    /// capability string, its minimum bridge release, and the byte-exact
    /// missing-capability clause the evaluators must surface. Adding a
    /// fourth capability extends this table — never another gate copy.
    fn gate_table() -> Vec<(
        &'static CapabilityGate,
        &'static str,
        Option<&'static str>,
        String,
    )> {
        vec![
            (
                &CapabilityGate::TLS,
                "broker-tls-v1",
                None,
                "lacks broker-tls-v1; use the pinned bridge".to_string(),
            ),
            (
                &CapabilityGate::SELECTOR,
                "message-selector-v1",
                Some(crate::BRIDGE_VERSION),
                format!(
                    "lacks message-selector-v1; requires a jms bridge >= {}",
                    crate::BRIDGE_VERSION
                ),
            ),
            (
                &CapabilityGate::DURABLE,
                "durable-subscribers-v1",
                Some(DURABLE_SUBSCRIBERS_MIN_BRIDGE_VERSION),
                format!(
                    "lacks durable-subscribers-v1; requires a jms bridge >= 0.9.0 (pinned {})",
                    crate::BRIDGE_VERSION
                ),
            ),
        ]
    }

    /// The descriptor constants carry the authored wire strings and minimum
    /// releases, and `missing_clause` reproduces the pre-refactor
    /// diagnostics byte for byte (rc-06dy1: messages unchanged).
    #[test]
    fn descriptor_constants_and_clauses_are_byte_identical() {
        for (gate, capability, min, clause) in gate_table() {
            assert_eq!(gate.capability, capability, "wire string drifted");
            assert_eq!(gate.min_bridge_version, min, "min version drifted");
            let rendered = gate.missing_clause();
            assert_eq!(
                rendered, clause,
                "clause drifted for {capability}: {rendered}"
            );
        }
    }

    /// The attach evaluator's gate-selection table: all three capabilities ×
    /// required/not-required × capability advertised / absent / other-only /
    /// empty. Required + advertised passes; anything else fails closed with
    /// the byte-exact `bridge {clause}` reason; not-required is a no-op.
    #[test]
    fn attach_evaluator_gate_selection_table() {
        for (gate, capability, _min, clause) in gate_table() {
            let advertised = vec![capability.to_string()];
            let other_only = vec!["broker-tls-v1".to_string()];
            for caps in [&advertised, &other_only, &Vec::new()] {
                // Not required: never fatal, regardless of the evidence.
                assert!(
                    ensure_capability(caps, gate, false).is_ok(),
                    "{capability}: not-required must pass on {caps:?}"
                );
                let result = ensure_capability(caps, gate, true);
                if caps.as_slice() == advertised.as_slice() {
                    assert!(
                        result.is_ok(),
                        "{capability}: advertised evidence must pass: {result:?}"
                    );
                } else {
                    let err =
                        result.expect_err("{capability}: un-advertised evidence must fail closed");
                    assert!(
                        matches!(err, BridgeError::PermanentFailure(_)),
                        "must classify permanent: {err:?}"
                    );
                    let expected = format!("bridge {clause}");
                    assert_eq!(
                        err.to_string(),
                        format!("Bridge permanent failure: {expected}"),
                        "{capability}: reason bytes drifted"
                    );
                }
            }
        }
    }

    /// The startup evaluator surfaces the same clause behind the pinned
    /// `crate::BRIDGE_VERSION` prefix; not-required stays a no-op.
    #[test]
    fn startup_evaluator_pins_pinned_prefix_and_clause() {
        for (gate, capability, _min, clause) in gate_table() {
            let expected = format!("bridge {} {clause}", crate::BRIDGE_VERSION);
            let err = evaluate_startup_capability_gate(&[], gate, true)
                .expect_err("{capability}: startup gate must fail closed on empty evidence");
            assert_eq!(
                err.to_string(),
                format!("Bridge permanent failure: {expected}"),
                "{capability}: startup reason bytes drifted"
            );
            let caps = vec![capability.to_string()];
            assert!(
                evaluate_startup_capability_gate(&caps, gate, true).is_ok(),
                "{capability}: advertised evidence must pass the startup gate"
            );
            assert!(
                evaluate_startup_capability_gate(&[], gate, false).is_ok(),
                "{capability}: not-required startup gate is a no-op"
            );
        }
    }

    /// The five per-capability wrappers delegate to the descriptor
    /// evaluators: same outcomes, same reason bytes (rc-06dy1).
    #[test]
    fn per_capability_wrappers_match_the_descriptor_evaluators() {
        let selector_caps = vec!["message-selector-v1".to_string()];
        let durable_caps = vec!["durable-subscribers-v1".to_string()];
        let tls_caps = vec!["broker-tls-v1".to_string()];

        let wrapper = ensure_selector_capability(&[], true)
            .expect_err("selector wrapper must fail closed on empty evidence");
        let direct = ensure_capability(&[], &CapabilityGate::SELECTOR, true)
            .expect_err("selector evaluator must fail closed on empty evidence");
        assert_eq!(wrapper.to_string(), direct.to_string());

        let wrapper = ensure_durable_capability(&[], true)
            .expect_err("durable wrapper must fail closed on empty evidence");
        let direct = ensure_capability(&[], &CapabilityGate::DURABLE, true)
            .expect_err("durable evaluator must fail closed on empty evidence");
        assert_eq!(wrapper.to_string(), direct.to_string());

        assert!(ensure_selector_capability(&selector_caps, true).is_ok());
        assert!(ensure_durable_capability(&durable_caps, true).is_ok());
        assert!(ensure_tls_capability(&tls_caps, true).is_ok());
        assert!(ensure_tls_capability(&[], false).is_ok());

        let wrapper = evaluate_startup_selector_gate(&[], true)
            .expect_err("startup selector wrapper must fail closed");
        let direct = evaluate_startup_capability_gate(&[], &CapabilityGate::SELECTOR, true)
            .expect_err("startup selector evaluator must fail closed");
        assert_eq!(wrapper.to_string(), direct.to_string());

        let wrapper = evaluate_startup_durable_gate(&[], true)
            .expect_err("startup durable wrapper must fail closed");
        let direct = evaluate_startup_capability_gate(&[], &CapabilityGate::DURABLE, true)
            .expect_err("startup durable evaluator must fail closed");
        assert_eq!(wrapper.to_string(), direct.to_string());
    }

    /// One descriptor drives BOTH evaluators: a test-only capability rides
    /// the same attach and startup paths end to end — the proof that a
    /// fourth capability needs one constant, not one copy.
    #[test]
    fn one_descriptor_drives_both_evaluators() {
        const FAKE: CapabilityGate = CapabilityGate {
            capability: "fake-capability-v1",
            min_bridge_version: Some("9.9.9"),
        };
        let expected_clause = format!(
            "lacks fake-capability-v1; requires a jms bridge >= 9.9.9 (pinned {})",
            crate::BRIDGE_VERSION
        );
        // Attach path consults the descriptor.
        let attach_err = ensure_capability(&[], &FAKE, true)
            .expect_err("fake capability must fail the attach path");
        assert_eq!(
            attach_err.to_string(),
            format!("Bridge permanent failure: bridge {expected_clause}"),
            "attach path must surface the fake clause byte-exactly"
        );
        // Startup path consults the same descriptor.
        let startup_err = evaluate_startup_capability_gate(&[], &FAKE, true)
            .expect_err("fake capability must fail the startup path");
        assert_eq!(
            startup_err.to_string(),
            format!(
                "Bridge permanent failure: bridge {} {expected_clause}",
                crate::BRIDGE_VERSION
            ),
            "startup path must surface the fake clause byte-exactly"
        );
        // Advertising the fake capability passes both paths.
        let caps = vec!["fake-capability-v1".to_string()];
        assert!(ensure_capability(&caps, &FAKE, true).is_ok());
        assert!(evaluate_startup_capability_gate(&caps, &FAKE, true).is_ok());
    }

    /// The version ladder is capability advertisement (rc-06dy1: semantics
    /// unchanged): an empty Health response is "below" every gate's minimum
    /// and fails closed; the exact capability string is "at"; extra
    /// unrelated capabilities alongside it are "above". No semver compare
    /// exists at runtime — `min_bridge_version` drives the authored text.
    #[test]
    fn capability_advertisement_is_the_version_ladder() {
        assert_eq!(
            CapabilityGate::SELECTOR.min_bridge_version,
            Some(crate::BRIDGE_VERSION),
            "selector's minimum is the pinned build"
        );
        assert_eq!(
            CapabilityGate::DURABLE.min_bridge_version,
            Some("0.9.0"),
            "durable's minimum is the durable release floor"
        );
        assert_eq!(CapabilityGate::TLS.min_bridge_version, None);
        for (gate, capability, _min, _clause) in gate_table() {
            // Below: no evidence at all.
            assert!(
                ensure_capability(&[], gate, true).is_err(),
                "{capability}: empty evidence is below the gate"
            );
            // At: exactly the required capability.
            let at = vec![capability.to_string()];
            assert!(
                ensure_capability(&at, gate, true).is_ok(),
                "{capability}: exact advertisement is at the gate"
            );
            // Above: the capability plus unrelated advertisements.
            let above = vec![
                "unrelated-capability-v9".to_string(),
                capability.to_string(),
            ];
            assert!(
                ensure_capability(&above, gate, true).is_ok(),
                "{capability}: advertisement plus extras is at/above the gate"
            );
        }
    }
}
