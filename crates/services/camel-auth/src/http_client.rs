use std::net::IpAddr;
use std::time::Duration;

use camel_api::SsrfPolicy;

use crate::types::AuthError;

/// Options for building an SSRF-hardened `reqwest::Client`.
#[derive(Debug, Clone)]
pub struct SsrfClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub policy: SsrfPolicy,
}

impl SsrfClientOptions {
    pub fn new(policy: SsrfPolicy) -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            policy,
        }
    }

    pub fn with_connect_timeout(mut self, d: Duration) -> Self {
        self.connect_timeout = d;
        self
    }

    pub fn with_request_timeout(mut self, d: Duration) -> Self {
        self.request_timeout = d;
        self
    }
}

/// Builds an SSRF-pinned [`reqwest::Client`] for the given URI.
///
/// 1. **Validates** the URI syntax and scheme against `policy`
///    ([`validate_uri`]).
/// 2. **Resolves DNS** (with 5s timeout) and filters out any SSRF-blocked
///    IPs when policy is `PublicHttpsOnly`, or checks HTTP-over-public-IP
///    when policy is `AllowInternal`.
/// 3. **Pins** the validated addresses on the client so `reqwest` never
///    re-resolves DNS — closing the TOCTOU window between validation
///    and the first outbound request.
///
/// The returned client also has `.no_proxy()`, hardened timeouts, and
/// redirect disabled.
pub async fn build_ssrf_pinned_client(
    uri: &str,
    label: &str,
    options: &SsrfClientOptions,
) -> Result<reqwest::Client, AuthError> {
    let parsed = validate_uri(uri, label, options.policy)?;

    let host = match parsed.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(AuthError::ConfigError(format!("{label} URI missing host"))),
    };
    let port = parsed.port_or_known_default().unwrap_or(443);

    let resolved: Vec<std::net::SocketAddr> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| AuthError::ProviderUnavailable(format!("{label} DNS resolution timed out (5s)")))?
    .map_err(|e| AuthError::ProviderUnavailable(format!("{label} DNS resolution failed: {e}")))?
    .collect();

    if resolved.is_empty() {
        return Err(AuthError::ProviderUnavailable(format!(
            "{label} host '{host}' resolved to zero addresses"
        )));
    }

    let validated_addrs: Vec<std::net::SocketAddr> = match options.policy {
        SsrfPolicy::PublicHttpsOnly => resolved
            .into_iter()
            .filter(|sa| !camel_api::is_ssrf_blocked_ip(&sa.ip()))
            .collect(),
        SsrfPolicy::AllowInternal => {
            // HTTP over public IP is cleartext to internet — reject.
            if parsed.scheme() == "http"
                && resolved
                    .iter()
                    .any(|sa| !camel_api::is_ssrf_blocked_ip(&sa.ip()))
            {
                return Err(AuthError::ConfigError(format!(
                    "{label} host '{host}' resolves to a public IP — HTTP not permitted (use HTTPS)"
                )));
            }
            resolved
        }
    };

    if validated_addrs.is_empty() {
        return Err(AuthError::ConfigError(format!(
            "{label} host '{host}' resolves only to blocked IPs (SSRF)"
        )));
    }

    reqwest::Client::builder()
        .resolve_to_addrs(host.as_str(), &validated_addrs)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .build()
        .map_err(|e| AuthError::ConfigError(format!("failed to build {label} HTTP client: {e}")))
}

/// Validates that `uri` has an http(s) scheme and a host.
///
/// Sync syntax-level check only. Resolved-IP classification (HTTP-permitted-
/// if-all-internal) is enforced in the async `build_ssrf_pinned_client`.
pub fn validate_uri(uri: &str, label: &str, policy: SsrfPolicy) -> Result<url::Url, AuthError> {
    let parsed = uri
        .parse::<url::Url>()
        .map_err(|e| AuthError::ConfigError(format!("invalid {label} '{uri}': {e}")))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AuthError::ConfigError(format!(
            "{label} must use http/https (got scheme '{}')",
            parsed.scheme()
        )));
    }

    let host = parsed.host_str().unwrap_or("");

    match policy {
        SsrfPolicy::PublicHttpsOnly => {
            if parsed.scheme() != "https" {
                return Err(AuthError::ConfigError(format!(
                    "{label} must use HTTPS (got scheme '{}')",
                    parsed.scheme()
                )));
            }
            if is_private_or_loopback_host(host) {
                return Err(AuthError::ConfigError(format!(
                    "{label} host '{host}' is a private or loopback address (SSRF guard)"
                )));
            }
        }
        SsrfPolicy::AllowInternal => {
            // Accept http and https; IP-gated decision deferred to async layer.
            // Only reject obviously non-http(s) schemes (already handled above).
        }
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_uri_rejects_non_http_scheme() {
        let err =
            validate_uri("ftp://example.com", "test", SsrfPolicy::PublicHttpsOnly).unwrap_err();
        assert!(err.to_string().contains("http/https"));
    }

    #[test]
    fn validate_uri_public_https_only_rejects_http() {
        let err = validate_uri("http://1.1.1.1", "test", SsrfPolicy::PublicHttpsOnly).unwrap_err();
        assert!(err.to_string().contains("HTTPS"));
    }

    #[test]
    fn validate_uri_public_https_only_rejects_localhost() {
        let err =
            validate_uri("https://localhost", "test", SsrfPolicy::PublicHttpsOnly).unwrap_err();
        assert!(err.to_string().contains("private or loopback"));
    }

    #[test]
    fn validate_uri_allow_internal_accepts_http() {
        let result = validate_uri("http://localhost:11434", "test", SsrfPolicy::AllowInternal);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_uri_allow_internal_accepts_https() {
        let result = validate_uri("https://1.1.1.1", "test", SsrfPolicy::AllowInternal);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_uri_allow_internal_rejects_ftp() {
        let err = validate_uri("ftp://localhost", "test", SsrfPolicy::AllowInternal).unwrap_err();
        assert!(err.to_string().contains("http/https"));
    }

    #[test]
    fn ssrf_client_options_defaults() {
        let opts = SsrfClientOptions::new(SsrfPolicy::PublicHttpsOnly);
        assert_eq!(opts.connect_timeout, Duration::from_secs(10));
        assert_eq!(opts.request_timeout, Duration::from_secs(30));
        assert_eq!(opts.policy, SsrfPolicy::PublicHttpsOnly);
    }

    #[test]
    fn ssrf_client_options_with_methods() {
        let opts = SsrfClientOptions::new(SsrfPolicy::AllowInternal)
            .with_connect_timeout(Duration::from_secs(3))
            .with_request_timeout(Duration::from_secs(5));
        assert_eq!(opts.connect_timeout, Duration::from_secs(3));
        assert_eq!(opts.request_timeout, Duration::from_secs(5));
        assert_eq!(opts.policy, SsrfPolicy::AllowInternal);
    }
}

/// Returns `true` if the host string resolves to a loopback or private IP.
fn is_private_or_loopback_host(host: &str) -> bool {
    // Named loopback / unspecified — these don't parse as IP literals,
    // so we hard-reject the conventional names up front.
    if matches!(host, "localhost" | "localhost.localdomain" | "0.0.0.0") {
        return true;
    }
    // url::Url::host_str() wraps IPv6 addresses in brackets: "[::1]".
    // std::net::IpAddr::from_str rejects the bracket form, so strip them first.
    let ip_str = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = ip_str.parse::<IpAddr>() {
        // Delegate to the canonical SSRF block-list so this rule set
        // stays in lockstep with every other outbound HTTP client
        // (LLM, Keycloak, …) instead of drifting.
        return camel_api::is_ssrf_blocked_ip(&ip);
    }
    false
}
