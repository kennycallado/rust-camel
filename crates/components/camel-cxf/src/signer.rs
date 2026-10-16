//! Verified signer descriptor validation for the CXF consumer.
//!
//! The Java bridge exports a verified, Body-covering signer identity as four
//! bounded strings (`SignerIdentity`). This module re-validates every field on
//! the Rust side of the trust boundary (ADR-0032) before any value reaches an
//! exchange. A validation failure is a non-WSS `Receiver` fault: it is never an
//! auth or security fault, so the profile's `security.fault_status` override
//! (WSS faults only) cannot rewrite it.
//!
//! The validated values are stored as dedicated `camel.cxf.signer.*` exchange
//! properties only. This module SHALL NOT assemble an authenticated identity,
//! call the principal-property helper, or write any auth-scoped property key.

use camel_component_api::CamelError;

use crate::proto::SignerIdentity;

/// Exchange property carrying the signer's RFC 2253 subject DN.
pub const SIGNER_SUBJECT: &str = "camel.cxf.signer.subject";
/// Exchange property carrying the signer's RFC 2253 issuer DN.
pub const SIGNER_ISSUER: &str = "camel.cxf.signer.issuer";
/// Exchange property carrying the signer certificate's lowercase-hex serial.
pub const SIGNER_SERIAL: &str = "camel.cxf.signer.serial";
/// Exchange property carrying the 64-lowercase-hex SHA-256 certificate fingerprint.
pub const SIGNER_FINGERPRINT: &str = "camel.cxf.signer.fingerprint_sha256";

/// Maximum size of a distinguished name field, measured in UTF-8 bytes.
const MAX_DN_BYTES: usize = 4096;
/// Maximum size of the certificate serial, measured in UTF-8 bytes.
const MAX_SERIAL_BYTES: usize = 128;
/// Exact size of a SHA-256 fingerprint in lowercase hex characters.
const FINGERPRINT_HEX_LEN: usize = 64;

/// A signer descriptor that passed every bound in [`validate_signer`].
///
/// `#[non_exhaustive]` keeps the validated-type invariant: only
/// [`validate_signer`] can produce one, so every instance is bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedSigner {
    /// RFC 2253 distinguished name of the signer certificate subject.
    pub subject: String,
    /// RFC 2253 distinguished name of the signer certificate issuer.
    pub issuer: String,
    /// Unsigned lowercase hex certificate serial.
    pub serial: String,
    /// 64 lowercase hex characters of SHA-256(DER certificate).
    pub fingerprint_sha256: String,
}

/// Validates one bridge-exported signer descriptor against the field bounds.
///
/// Bounds (UTF-8 bytes, not characters):
/// - `subject`, `issuer`: at most 4096 bytes each;
/// - `serial`: non-empty, at most 128 bytes, lowercase hex only;
/// - `fingerprint_sha256`: exactly 64 lowercase hex characters.
///
/// Errors name the offending field and nothing else: the values themselves
/// (which may embed certificate subject data) never enter the message. The
/// error is a plain [`CamelError::ProcessorError`] — a non-WSS `Receiver`
/// fault, never an auth or security fault.
pub fn validate_signer(signer: &SignerIdentity) -> Result<ValidatedSigner, CamelError> {
    if signer.subject.len() > MAX_DN_BYTES {
        return Err(signer_error("subject", MAX_DN_BYTES, "UTF-8 bytes"));
    }
    if signer.issuer.len() > MAX_DN_BYTES {
        return Err(signer_error("issuer", MAX_DN_BYTES, "UTF-8 bytes"));
    }
    if signer.serial.is_empty() {
        return Err(invalid_hex("serial"));
    }
    if signer.serial.len() > MAX_SERIAL_BYTES {
        return Err(signer_error("serial", MAX_SERIAL_BYTES, "bytes"));
    }
    if !is_lowercase_hex(&signer.serial) {
        return Err(invalid_hex("serial"));
    }
    if signer.fingerprint_sha256.len() != FINGERPRINT_HEX_LEN
        || !is_lowercase_hex(&signer.fingerprint_sha256)
    {
        return Err(invalid_hex("fingerprint_sha256"));
    }
    Ok(ValidatedSigner {
        subject: signer.subject.clone(),
        issuer: signer.issuer.clone(),
        serial: signer.serial.clone(),
        fingerprint_sha256: signer.fingerprint_sha256.clone(),
    })
}

fn is_lowercase_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn signer_error(field: &str, bound: usize, unit: &str) -> CamelError {
    CamelError::ProcessorError(format!("cxf signer {field} exceeds {bound} {unit}"))
}

fn invalid_hex(field: &str) -> CamelError {
    if field == "fingerprint_sha256" {
        CamelError::ProcessorError(
            "cxf signer fingerprint_sha256 must be exactly 64 lowercase hex characters".to_string(),
        )
    } else {
        CamelError::ProcessorError(format!(
            "cxf signer {field} must be non-empty lowercase hex"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL_SUBJECT: &str = "CN=test-alice,O=RustCamel Test";
    const NORMAL_ISSUER: &str = "CN=test-alice,O=RustCamel Test";
    const NORMAL_SERIAL: &str = "0a1b2c";
    const NORMAL_FINGERPRINT: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn descriptor(subject: &str, issuer: &str, serial: &str, fingerprint: &str) -> SignerIdentity {
        SignerIdentity {
            subject: subject.to_string(),
            issuer: issuer.to_string(),
            serial: serial.to_string(),
            fingerprint_sha256: fingerprint.to_string(),
        }
    }

    fn valid() -> SignerIdentity {
        descriptor(
            NORMAL_SUBJECT,
            NORMAL_ISSUER,
            NORMAL_SERIAL,
            NORMAL_FINGERPRINT,
        )
    }

    #[test]
    fn valid_descriptor_yields_four_fields() {
        let signer = valid();
        let validated = validate_signer(&signer).expect("valid descriptor must be accepted");
        assert_eq!(validated.subject, NORMAL_SUBJECT);
        assert_eq!(validated.issuer, NORMAL_ISSUER);
        assert_eq!(validated.serial, NORMAL_SERIAL);
        assert_eq!(validated.fingerprint_sha256, NORMAL_FINGERPRINT);
    }

    #[test]
    fn oversized_subject_is_rejected() {
        let oversized = "a".repeat(4097);
        let boundary = "a".repeat(4096);
        // Table: (case, subject, issuer, expect_ok). UTF-8 bytes, not chars —
        // the boundary row is ASCII so bytes and chars coincide; the oversized
        // rows exercise the byte cap at 4097.
        let cases: [(&str, String, String, bool); 3] = [
            (
                "subject 4097 bytes",
                oversized.clone(),
                NORMAL_ISSUER.into(),
                false,
            ),
            ("issuer 4097 bytes", NORMAL_SUBJECT.into(), oversized, false),
            (
                "subject+issuer 4096 bytes",
                boundary.clone(),
                boundary,
                true,
            ),
        ];
        for (name, subject, issuer, expect_ok) in cases {
            let req = descriptor(&subject, &issuer, NORMAL_SERIAL, NORMAL_FINGERPRINT);
            let result = validate_signer(&req);
            if expect_ok {
                assert!(result.is_ok(), "{name} must be accepted: {result:?}");
            } else {
                let err = result.expect_err(&format!("{name} must be rejected"));
                let msg = err.to_string();
                assert!(
                    msg.contains("subject") || msg.contains("issuer"),
                    "{name}: error must name the offending DN field, got: {msg}"
                );
                assert!(!msg.contains("aaaa"), "{name}: error must not echo the DN");
            }
        }
    }

    #[test]
    fn non_lowercase_serial_is_rejected() {
        let long_hex = "a".repeat(129);
        // Table: (case, serial, expect_ok). Empty serial is rejected too — a
        // real certificate always has a non-empty serial.
        let cases: [(&str, &str, bool); 5] = [
            ("uppercase hex", "0A1B2C", false),
            ("mixed case hex", "0a1B2c", false),
            ("non-hex", "0x1b2c", false),
            ("empty", "", false),
            ("lowercase hex, 129 bytes", &long_hex, false),
        ];
        for (name, serial, expect_ok) in cases {
            let req = descriptor(NORMAL_SUBJECT, NORMAL_ISSUER, serial, NORMAL_FINGERPRINT);
            let result = validate_signer(&req);
            if expect_ok {
                assert!(result.is_ok(), "{name} must be accepted: {result:?}");
            } else {
                let err = result.expect_err(&format!("{name} must be rejected"));
                let msg = err.to_string();
                assert!(
                    msg.contains("serial"),
                    "{name}: error must name the serial field, got: {msg}"
                );
            }
        }
    }

    #[test]
    fn fingerprint_must_be_64_lowercase_hex() {
        let lower64 = NORMAL_FINGERPRINT;
        let upper64 = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";
        let nonhex64 = "g".repeat(64);
        let cases: [(&str, String, bool); 6] = [
            ("63 chars", "a".repeat(63), false),
            ("65 chars", "a".repeat(65), false),
            ("64 uppercase hex", upper64.to_string(), false),
            ("64 non-hex", nonhex64, false),
            ("empty", String::new(), false),
            ("64 lowercase hex", lower64.to_string(), true),
        ];
        for (name, fingerprint, expect_ok) in cases {
            let req = descriptor(NORMAL_SUBJECT, NORMAL_ISSUER, NORMAL_SERIAL, &fingerprint);
            let result = validate_signer(&req);
            if expect_ok {
                assert!(result.is_ok(), "{name} must be accepted: {result:?}");
            } else {
                let err = result.expect_err(&format!("{name} must be rejected"));
                let msg = err.to_string();
                assert!(
                    msg.contains("fingerprint_sha256"),
                    "{name}: error must name the fingerprint field, got: {msg}"
                );
            }
        }
    }
}
