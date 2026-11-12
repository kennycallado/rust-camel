//! Expired-certificate allowlist enforcement for camel-http TLS clients (bd
//! rc-ishar).
//!
//! Tolerance is decided by **host + fingerprint** only: the rustls
//! `ServerCertVerifier::verify_server_cert` signature carries a
//! `ServerName` but no port, so the `port` of a
//! `crate::config::ExpiredCertAllowEntry` is declarative (config clarity
//! and duplicate rejection), never enforced. An expired INTERMEDIATE
//! certificate is tolerated the same way when the pinned leaf is the
//! presented end entity, because rustls maps webpki chain-level expiry
//! (`CertExpired`, leaf or intermediate) to
//! `rustls::CertificateError::ExpiredContext`. Before tolerating anything
//! the end entity's hostname is independently re-verified with
//! rustls-webpki (`EndEntityCert::verify_is_valid_for_subject_name`), which
//! keeps hostname verification independent of the webpki check order
//! (chain and validity run before hostname).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camel_component_api::CamelError;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::config::ExpiredCertAllowEntry;

/// Minimum spacing between duplicate accepted-expired-certificate warns for
/// the same host + fingerprint.
pub(crate) const WARN_INTERVAL: Duration = Duration::from_secs(60);

/// Parsed host form for allowlist entries and SNI server names: an IP
/// literal stays an IP, anything else is a lowercased DNS name compared
/// exactly (no suffix or wildcard logic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AllowHost {
    /// Lowercased exact DNS name.
    Dns(String),
    /// IP literal (`ServerName::IpAddress` and IP SANs compare here).
    Ip(std::net::IpAddr),
}

impl AllowHost {
    /// Parses an entry host string: an IP literal when `IpAddr`-parseable,
    /// otherwise a lowercased DNS name.
    fn parse(host: &str) -> Self {
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            Self::Ip(ip)
        } else {
            Self::Dns(host.to_lowercase())
        }
    }

    /// Maps a rustls server name to the same form (`DnsName` lowercased,
    /// IP kept as `std::net::IpAddr`). None for name forms this crate does
    /// not know (the enum is `#[non_exhaustive]`) — fail-closed: no match.
    fn from_server_name(server_name: &ServerName<'_>) -> Option<Self> {
        match server_name {
            ServerName::DnsName(dns) => Some(Self::Dns(dns.as_ref().to_lowercase())),
            ServerName::IpAddress(ip) => Some(Self::Ip(std::net::IpAddr::from(*ip))),
            _ => None,
        }
    }

    /// Fully owned comparison/log key string: the lowercased name or the IP
    /// literal rendered as text.
    fn key_string(&self) -> String {
        match self {
            Self::Dns(name) => name.clone(),
            Self::Ip(ip) => ip.to_string(),
        }
    }
}

/// One parsed allowlist entry: exact host form and the SHA-256 of the
/// pinned end-entity DER.
#[derive(Clone, Debug)]
pub(crate) struct AllowKey {
    /// Exact host the pin applies to.
    host: AllowHost,
    /// SHA-256 of the end-entity DER (`sha256:` hex from config, decoded).
    fingerprint: [u8; 32],
}

/// Parsed `tls.expired_allowlist`: a small Vec of pinned entries scanned
/// linearly per handshake.
#[derive(Clone, Debug)]
pub(crate) struct ExpiredCertAllowlist {
    /// Pinned host + fingerprint pairs (small N: linear scan).
    entries: Vec<AllowKey>,
}

impl ExpiredCertAllowlist {
    /// Re-runs the same textual rules as
    /// `config::validate_expired_allowlist` (defense in depth: validate()
    /// already rejects malformed entries at endpoint creation; parse must
    /// not panic on anything that slipped through) and hex-decodes each
    /// fingerprint into its 32-byte form.
    pub(crate) fn parse(entries: &[ExpiredCertAllowEntry]) -> Result<Self, CamelError> {
        let mut seen = std::collections::HashSet::new();
        let mut parsed = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let host = entry.host.trim();
            if host.is_empty() {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].host must not be empty"
                )));
            }
            if host.chars().any(char::is_whitespace) {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].host must not contain whitespace"
                )));
            }
            if host.contains('*') {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].host must not contain wildcard '*': exact names and IP literals only"
                )));
            }
            if host.contains('/') {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].host must not contain '/'"
                )));
            }
            if host.starts_with('.') {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].host must not start with '.'"
                )));
            }

            if entry.port == 0 {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].port must be non-zero (1-65535)"
                )));
            }

            let Some(hex_part) = entry.fingerprint.strip_prefix("sha256:") else {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].fingerprint must start with \"sha256:\""
                )));
            };
            if hex_part.len() != 64 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].fingerprint must be \"sha256:\" followed by exactly 64 hex characters"
                )));
            }

            let mut fingerprint = [0u8; 32];
            hex::decode_to_slice(hex_part, &mut fingerprint).map_err(|_| {
                CamelError::Config(format!(
                    "tls.expired_allowlist[{index}].fingerprint must be \"sha256:\" followed by exactly 64 hex characters"
                ))
            })?;

            let key = (host.to_lowercase(), entry.port, hex_part.to_lowercase());
            if !seen.insert(key) {
                return Err(CamelError::Config(format!(
                    "tls.expired_allowlist[{index}] is a duplicate entry (same normalized host, port, and fingerprint)"
                )));
            }

            parsed.push(AllowKey {
                host: AllowHost::parse(host),
                fingerprint,
            });
        }
        Ok(Self { entries: parsed })
    }

    /// Returns true iff BOTH the host (exact match: case-insensitive for
    /// DNS names, exact for IP literals) and the SHA-256 fingerprint of
    /// `end_entity_der` match one entry. No suffix or wildcard logic.
    pub(crate) fn matches(&self, server_name: &ServerName<'_>, end_entity_der: &[u8]) -> bool {
        let Some(host) = AllowHost::from_server_name(server_name) else {
            return false;
        };
        let digest = Sha256::digest(end_entity_der);
        self.entries
            .iter()
            .any(|entry| entry.host == host && entry.fingerprint.as_slice() == digest.as_slice())
    }
}

/// Fully owned rate-limit key: lowercased host string (or IP literal text)
/// plus the pinned end-entity fingerprint bytes.
pub(crate) type RateKey = (String, [u8; 32]);

/// Suppresses duplicate "accepted expired certificate" warns for the same
/// host + fingerprint within one interval.
#[derive(Debug)]
pub(crate) struct WarnRateLimiter {
    /// Minimum spacing between two warns for the same key.
    interval: Duration,
    /// First-sighting instant per key.
    seen: Mutex<HashMap<RateKey, Instant>>,
}

impl WarnRateLimiter {
    /// Limiter with an explicit interval (tests inject short/zero windows).
    pub(crate) fn with_interval(interval: Duration) -> Self {
        Self {
            interval,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true on first sighting of `key` (and again once
    /// `elapsed >= interval`), false while the key stays within the
    /// window.
    pub(crate) fn allows(&mut self, key: &RateKey) -> bool {
        let now = Instant::now();
        let mut seen = self.seen.lock().expect("warn rate limiter mutex poisoned"); // allow-unwrap
        let reallow = match seen.get(key) {
            Some(&first) => now.duration_since(first) >= self.interval,
            None => true,
        };
        if reallow {
            seen.insert(key.clone(), now);
        }
        reallow
    }
}

impl Default for WarnRateLimiter {
    fn default() -> Self {
        Self::with_interval(WARN_INTERVAL)
    }
}

/// Wrapper verifier: delegates every check to the configured inner
/// verifier, tolerating only expired-certificate errors for allowlisted
/// host + fingerprint pairs.
#[derive(Debug)]
pub(crate) struct ExpiredCertAllowVerifier {
    /// Underlying verifier (e.g. `WebPkiServerVerifier`) doing the full
    /// webpki chain validation.
    inner: Arc<dyn ServerCertVerifier>,
    /// Pinned host + fingerprint entries.
    allowlist: Arc<ExpiredCertAllowlist>,
    /// Warn suppression state for repeated accepts of the same pin.
    limiter: Mutex<WarnRateLimiter>,
}

impl ExpiredCertAllowVerifier {
    /// Wraps `inner` with the parsed `allowlist`; `limiter` carries the
    /// warn interval (tests inject it via
    /// `WarnRateLimiter::with_interval`).
    pub(crate) fn new(
        inner: Arc<dyn ServerCertVerifier>,
        allowlist: Arc<ExpiredCertAllowlist>,
        limiter: WarnRateLimiter,
    ) -> Self {
        Self {
            inner,
            allowlist,
            limiter: Mutex::new(limiter),
        }
    }

    /// Test-only: number of distinct keys the warn limiter has seen within
    /// their window (one entry per emitted warn).
    #[cfg(test)]
    pub(crate) fn suppressed_warn_count(&self) -> usize {
        self.limiter
            .lock()
            .expect("expired-cert warn limiter mutex poisoned") // allow-unwrap
            .seen
            .lock()
            .expect("warn rate limiter mutex poisoned") // allow-unwrap
            .len()
    }

    /// Independent webpki hostname check for the end entity: true only when
    /// the certificate is valid for `server_name` (DNS or IP SAN).
    fn hostname_recheck_ok(
        &self,
        end_entity: &CertificateDer<'_>,
        server_name: &ServerName<'_>,
    ) -> bool {
        webpki::EndEntityCert::try_from(end_entity)
            .is_ok_and(|cert| cert.verify_is_valid_for_subject_name(server_name).is_ok())
    }
}

impl ServerCertVerifier for ExpiredCertAllowVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(verified) => Ok(verified),
            Err(error) => {
                // `CertificateError` is `#[non_exhaustive]`: every
                // non-expiry variant (NotValidYet, unknown issuer,
                // revocation, ...) falls through unchanged.
                let expired = matches!(
                    &error,
                    Error::InvalidCertificate(
                        CertificateError::Expired | CertificateError::ExpiredContext { .. }
                    )
                );
                // Independently re-verify the hostname before tolerating:
                // the webpki chain/validity check runs before its hostname
                // check, so an expired cert never reaches hostname
                // verification there.
                if expired
                    && self.hostname_recheck_ok(end_entity, server_name)
                    && self.allowlist.matches(server_name, end_entity.as_ref())
                    // matches() true implies the server name mapped to an
                    // AllowHost; None stays fail-closed (returns the
                    // original error below).
                    && let Some(host) = AllowHost::from_server_name(server_name)
                {
                    let digest = Sha256::digest(end_entity.as_ref());
                    let rate_key: RateKey = (host.key_string(), digest.into());
                    // The limiter gates ONLY the warn, never the
                    // acceptance: rustls may re-call verify_server_cert
                    // within the window (session-store LRU eviction, no
                    // session tickets, TLS 1.2 resumption misses), and a
                    // certificate the allowlist tolerates must be
                    // accepted on every such call.
                    let warn_now = self
                        .limiter
                        .lock()
                        .expect("expired-cert warn limiter mutex poisoned") // allow-unwrap
                        .allows(&rate_key);
                    if warn_now {
                        tracing::warn!(
                            host = %camel_api::redact::redact_host(&host.key_string()),
                            fingerprint = %format!("sha256:{}", hex::encode(digest)),
                            "accepted expired TLS certificate — allowlist entry matched"
                        );
                    }
                    return Ok(ServerCertVerified::assertion());
                }
                Err(error)
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{CertificateError, DigitallySignedStruct, Error, SignatureScheme};

    use sha2::Digest;

    use super::{ExpiredCertAllowVerifier, ExpiredCertAllowlist, RateKey, WarnRateLimiter};

    /// Self-signed test leaf with the given SANs, valid 2024-01-01 through
    /// 2024-01-02 (already expired relative to any real validation time).
    fn gen_test_leaf(dns_san: Option<&str>, ip_san: Option<IpAddr>) -> rcgen::Certificate {
        let mut params = rcgen::CertificateParams::default();
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2024, 1, 2);
        let mut sans = Vec::new();
        if let Some(dns) = dns_san {
            sans.push(rcgen::SanType::DnsName(dns.try_into().expect("dns san")));
        }
        if let Some(ip) = ip_san {
            sans.push(rcgen::SanType::IpAddress(ip));
        }
        params.subject_alt_names = sans;
        let key = rcgen::KeyPair::generate().expect("leaf keygen"); // allow-unwrap(test)
        params.self_signed(&key).expect("leaf self-sign") // allow-unwrap(test)
    }

    /// `sha256:`-prefixed hex fingerprint of a DER-encoded certificate.
    fn fingerprint_hex(der: &[u8]) -> String {
        format!("sha256:{}", hex::encode(sha2::Sha256::digest(der)))
    }

    /// Config entry with the loopback declarative port.
    fn allow_entry(host: &str, fingerprint: String) -> crate::config::ExpiredCertAllowEntry {
        crate::config::ExpiredCertAllowEntry {
            host: host.to_string(),
            port: 8443,
            fingerprint,
        }
    }

    /// Fingerprint with its first hex digit flipped: never equal to the
    /// input fingerprint.
    fn flipped_fingerprint(fingerprint: &str) -> String {
        let mut chars: Vec<char> = fingerprint.chars().collect();
        let first = &mut chars[7]; // first hex digit after "sha256:"
        *first = if *first == '0' { '1' } else { '0' };
        chars.into_iter().collect()
    }

    /// Wrapper whose inner verifier fails `verify_server_cert` with
    /// `error`; warn interval 1 h so a single call always warns.
    fn wrapper_with_error(
        error: Error,
        entries: &[crate::config::ExpiredCertAllowEntry],
    ) -> ExpiredCertAllowVerifier {
        ExpiredCertAllowVerifier::new(
            std::sync::Arc::new(FailingVerifier(error)),
            std::sync::Arc::new(
                ExpiredCertAllowlist::parse(entries).expect("allowlist parse"), // allow-unwrap(test)
            ),
            WarnRateLimiter::with_interval(Duration::from_secs(3600)),
        )
    }

    /// The variant rustls emits for an expired certificate on the presented
    /// path: webpki `CertExpired` (chain-level expiry, leaf or intermediate)
    /// maps to `ExpiredContext` (rustls 0.23.46 webpki/mod.rs:66-69). Leaf
    /// NotAfter is 2024-01-02; validation time 2026-01-01.
    fn expired_context_error() -> Error {
        Error::InvalidCertificate(CertificateError::ExpiredContext {
            time: UnixTime::since_unix_epoch(Duration::from_secs(1_767_225_600)),
            not_after: UnixTime::since_unix_epoch(Duration::from_secs(1_704_153_600)),
        })
    }

    /// Inner verifier stub failing `verify_server_cert` with a fixed
    /// error; the signature hooks are never called by these tests.
    #[derive(Debug)]
    struct FailingVerifier(Error);

    impl ServerCertVerifier for FailingVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Err(self.0.clone())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Err(Error::General("unused by these tests".to_string()))
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Err(Error::General("unused by these tests".to_string()))
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            Vec::new()
        }
    }

    #[test]
    fn allowlist_matches_ip_server_name_and_fingerprint() {
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let list = ExpiredCertAllowlist::parse(&[allow_entry("127.0.0.1", fingerprint_hex(der))])
            .expect("allowlist parse"); // allow-unwrap(test)
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)

        assert!(list.matches(&server_name, der));

        let wrong_fp = flipped_fingerprint(&fingerprint_hex(der));
        let wrong = ExpiredCertAllowlist::parse(&[allow_entry("127.0.0.1", wrong_fp)])
            .expect("allowlist parse"); // allow-unwrap(test)
        assert!(!wrong.matches(&server_name, der));
    }

    #[test]
    fn allowlist_matches_dns_name_case_insensitively() {
        let cert = gen_test_leaf(Some("backend.internal"), None);
        let der = cert.der().as_ref();
        let list =
            ExpiredCertAllowlist::parse(&[allow_entry("BACKEND.Internal", fingerprint_hex(der))])
                .expect("allowlist parse"); // allow-unwrap(test)

        assert!(list.matches(
            &ServerName::try_from("backend.internal").expect("server name"), // allow-unwrap(test)
            der
        ));
        assert!(!list.matches(
            &ServerName::try_from("sub.backend.internal").expect("server name"), // allow-unwrap(test)
            der
        ));
    }

    #[test]
    fn rate_limiter_suppresses_within_window() {
        let mut limiter = WarnRateLimiter::with_interval(Duration::from_secs(3600));
        let key: RateKey = ("backend.internal".to_string(), [7u8; 32]);

        assert!(limiter.allows(&key));
        assert!(!limiter.allows(&key));

        let other: RateKey = ("other.internal".to_string(), [8u8; 32]);
        assert!(limiter.allows(&other));
    }

    #[test]
    fn rate_limiter_reallows_after_interval() {
        let mut limiter = WarnRateLimiter::with_interval(Duration::ZERO);
        let key: RateKey = ("127.0.0.1".to_string(), [9u8; 32]);

        assert!(limiter.allows(&key));
        assert!(limiter.allows(&key));
    }

    #[test]
    fn wrapper_accepts_expired_listed_cert() {
        // Spec scenario "Expired intermediate with pinned leaf is
        // tolerated": `ExpiredContext` is exactly the variant rustls emits
        // for an expired certificate on the presented path (webpki
        // `CertExpired` → `ExpiredContext`), so tolerating it pins that
        // semantics end-to-end.
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let verifier = wrapper_with_error(
            expired_context_error(),
            &[allow_entry("127.0.0.1", fingerprint_hex(der))],
        );

        let result =
            verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());

        assert!(result.is_ok(), "got: {result:?}");
    }

    #[test]
    fn wrapper_accepts_expired_cert_on_every_call_within_window() {
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let verifier = wrapper_with_error(
            expired_context_error(),
            &[allow_entry("127.0.0.1", fingerprint_hex(der))],
        );

        // Acceptance is unconditional: rustls can re-call verify_server_cert
        // within the warn window (session-store eviction, no tickets, TLS
        // 1.2 resumption misses), and every call must accept.
        for _ in 0..3 {
            let result =
                verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());
            assert!(result.is_ok(), "got: {result:?}");
        }

        // Exactly ONE warn across the three accepted calls: the limiter
        // registered only the first sighting (1 h window still open).
        assert_eq!(verifier.suppressed_warn_count(), 1);
    }

    #[test]
    fn wrapper_rejects_expired_cert_when_hostname_mismatches() {
        let cert = gen_test_leaf(Some("backend.internal"), None);
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let verifier = wrapper_with_error(
            expired_context_error(),
            &[allow_entry("127.0.0.1", fingerprint_hex(der))],
        );

        let result =
            verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());

        assert_eq!(result.err(), Some(expired_context_error()));
    }

    #[test]
    fn wrapper_rejects_expired_cert_with_wrong_fingerprint() {
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let wrong_fp = flipped_fingerprint(&fingerprint_hex(der));
        let verifier = wrapper_with_error(
            expired_context_error(),
            &[allow_entry("127.0.0.1", wrong_fp)],
        );

        let result =
            verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());

        assert_eq!(result.err(), Some(expired_context_error()));
    }

    #[test]
    fn wrapper_propagates_not_valid_yet() {
        let error = Error::InvalidCertificate(CertificateError::NotValidYetContext {
            time: UnixTime::since_unix_epoch(Duration::from_secs(1_704_067_200)),
            not_before: UnixTime::since_unix_epoch(Duration::from_secs(1_704_153_600)),
        });
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let verifier = wrapper_with_error(
            error.clone(),
            &[allow_entry("127.0.0.1", fingerprint_hex(der))],
        );

        let result =
            verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());

        assert_eq!(result.err(), Some(error));
    }

    #[test]
    fn wrapper_propagates_unknown_issuer() {
        let error = Error::InvalidCertificate(CertificateError::UnknownIssuer);
        let cert = gen_test_leaf(None, Some(IpAddr::V4([127, 0, 0, 1].into())));
        let der = cert.der().as_ref();
        let server_name = ServerName::try_from("127.0.0.1").expect("server name"); // allow-unwrap(test)
        let verifier = wrapper_with_error(
            error.clone(),
            &[allow_entry("127.0.0.1", fingerprint_hex(der))],
        );

        let result =
            verifier.verify_server_cert(cert.der(), &[], &server_name, &[], UnixTime::now());

        assert_eq!(result.err(), Some(error));
    }
}
