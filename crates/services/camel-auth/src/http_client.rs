use std::net::IpAddr;
use std::time::Duration;

use crate::types::AuthError;

/// Builds an SSRF-pinned [`reqwest::Client`] for the given URI.
///
/// 1. **Validates** the URI is a public HTTPS endpoint
///    ([`validate_https_public_uri`]).
/// 2. **Resolves DNS** (with 5s timeout) and filters out any SSRF-blocked IPs.
/// 3. **Pins** the validated addresses on the client so `reqwest` never
///    re-resolves DNS — closing the TOCTOU window between validation
///    and the first outbound request.
///
/// The returned client also has hardened timeouts and redirect disabled.
pub async fn build_ssrf_pinned_client(
    uri: &str,
    label: &str,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<reqwest::Client, AuthError> {
    let parsed = validate_https_public_uri(uri, label)?;

    // Use url::Host enum for proper IPv6 bracket handling.
    let host = match parsed.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(AuthError::ConfigError(format!("{label} URI missing host"))),
    };
    let port = parsed.port_or_known_default().unwrap_or(443);

    // Resolve with timeout, filtering out SSRF-blocked IPs.
    let validated_addrs: Vec<std::net::SocketAddr> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| AuthError::ProviderUnavailable(format!("{label} DNS resolution timed out (5s)")))?
    .map_err(|e| AuthError::ProviderUnavailable(format!("{label} DNS resolution failed: {e}")))?
    .filter(|sa| !camel_api::is_ssrf_blocked_ip(&sa.ip()))
    .collect();

    if validated_addrs.is_empty() {
        return Err(AuthError::ConfigError(format!(
            "{label} host '{host}' resolves only to blocked IPs (SSRF)"
        )));
    }

    // Pin validated IPs so reqwest never re-resolves DNS.
    reqwest::Client::builder()
        .resolve_to_addrs(host.as_str(), &validated_addrs)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .map_err(|e| AuthError::ConfigError(format!("failed to build {label} HTTP client: {e}")))
}

/// Validates that `uri` is a public HTTPS endpoint safe for outbound requests.
///
/// Rules:
/// - Scheme must be `https`
/// - Host must not be a loopback, private, link-local, CGN, benchmark,
///   reserved, or otherwise SSRF-blocked address (delegated to the
///   canonical [`camel_api::is_ssrf_blocked_ip`] helper).
/// - A small set of well-known loopback hostnames (`localhost`,
///   `localhost.localdomain`, `0.0.0.0`) is rejected even though they
///   don't parse as IP literals — DNS for those names conventionally
///   resolves to a blocked address on every sane system.
///
/// Returns the parsed [`url::Url`] on success so callers can reuse it.
pub fn validate_https_public_uri(uri: &str, label: &str) -> Result<url::Url, AuthError> {
    let parsed = uri
        .parse::<url::Url>()
        .map_err(|e| AuthError::ConfigError(format!("invalid {label} '{uri}': {e}")))?;

    if parsed.scheme() != "https" {
        return Err(AuthError::ConfigError(format!(
            "{label} must use HTTPS (got scheme '{}')",
            parsed.scheme()
        )));
    }

    let host = parsed.host_str().unwrap_or("");
    if is_private_or_loopback_host(host) {
        return Err(AuthError::ConfigError(format!(
            "{label} host '{host}' is a private or loopback address (SSRF guard)"
        )));
    }

    Ok(parsed)
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
