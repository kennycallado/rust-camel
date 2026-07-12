//! SSRF (Server-Side Request Forgery) protection functions.
//!
//! Validates URLs, resolves hostnames, and enforces DNS pinning to prevent
//! attackers from using the HTTP producer to reach internal/private networks.

use std::time::Duration;

use camel_api::is_ssrf_blocked_ip;
use camel_component_api::CamelError;

use crate::config::HttpConfig;
use crate::{HttpEndpointConfig, build_client};

/// Validate a URL against SSRF rules: blocked hosts and private IP ranges.
pub(crate) fn validate_url_for_ssrf(
    url: &str,
    config: &HttpEndpointConfig,
) -> Result<(), CamelError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| CamelError::ProcessorError(format!("Invalid URL: {}", e)))?;

    // Reject non-http(s) schemes under both policies
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CamelError::ProcessorError(format!(
                "Scheme '{}' is not allowed (only http and https)",
                other
            )));
        }
    }

    // Check blocked hosts
    if let Some(host) = parsed.host_str()
        && config.blocked_hosts.iter().any(|blocked| host == blocked)
    {
        return Err(CamelError::ProcessorError(format!(
            "Host '{}' is blocked",
            host
        )));
    }

    // Check IP literals
    if let Some(host) = parsed.host() {
        match host {
            url::Host::Ipv4(ip) => {
                let ip_addr = std::net::IpAddr::V4(ip);
                let is_blocked = is_ssrf_blocked_ip(&ip_addr);
                if !config.allow_internal && is_blocked {
                    return Err(CamelError::ProcessorError(format!(
                        "Private IP '{}' not allowed (set allow_internal=true to override)",
                        ip
                    )));
                }
                // Under allow_internal: reject public IPs with HTTP (no cleartext to internet)
                if config.allow_internal && !is_blocked && parsed.scheme() == "http" {
                    return Err(CamelError::ProcessorError(format!(
                        "Public IP '{}' not allowed over HTTP (use HTTPS for public IPs)",
                        ip
                    )));
                }
            }
            url::Host::Ipv6(ip) => {
                let ip_addr = std::net::IpAddr::V6(ip);
                let is_blocked = is_ssrf_blocked_ip(&ip_addr);
                if !config.allow_internal && is_blocked {
                    return Err(CamelError::ProcessorError(format!(
                        "Blocked IP '{}' not allowed",
                        ip
                    )));
                }
                if config.allow_internal && !is_blocked && parsed.scheme() == "http" {
                    return Err(CamelError::ProcessorError(format!(
                        "Public IP '{}' not allowed over HTTP (use HTTPS for public IPs)",
                        ip
                    )));
                }
            }
            url::Host::Domain(domain) => {
                // Block common internal domains when not allowing internal
                if !config.allow_internal {
                    let blocked_domains = ["localhost", "127.0.0.1", "0.0.0.0", "local"];
                    if blocked_domains.contains(&domain) {
                        return Err(CamelError::ProcessorError(format!(
                            "Domain '{}' is not allowed",
                            domain
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Resolve hostname and optionally validate IPs via SSRF check.
/// Always resolves for DNS pinning (TOCTOU prevention).
/// When `allow_internal` is true, all resolved addresses are returned as-is.
/// When false, only non-blocked IPs are returned.
/// Returns a String error so callers can map to the appropriate `CamelError` variant.
pub(crate) async fn resolve_and_validate_host(
    host: &str,
    port: u16,
    allow_internal: bool,
) -> Result<Vec<std::net::SocketAddr>, String> {
    let resolved: Vec<std::net::SocketAddr> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "DNS resolution timed out (5s)".to_string())?
    .map_err(|e| format!("DNS resolution failed: {e}"))?
    .collect();

    if allow_internal {
        return Ok(resolved);
    }

    let validated: Vec<std::net::SocketAddr> = resolved
        .into_iter()
        .filter(|sa| !is_ssrf_blocked_ip(&sa.ip()))
        .collect();

    if validated.is_empty() {
        return Err(format!("host '{host}' resolves only to blocked IPs (SSRF)"));
    }

    Ok(validated)
}

/// Validates a redirect target URL for SSRF. If the host is a domain name,
/// resolves it and checks all resulting IPs. Returns the resolved socket
/// addresses on success so the caller can pin them via `resolve_to_addrs`.
pub(crate) async fn validate_redirect_target_for_ssrf(
    url: &url::Url,
    allow_internal: bool,
) -> Result<Vec<std::net::SocketAddr>, CamelError> {
    let Some(host_str) = url.host_str() else {
        return Err(CamelError::ProcessorError(
            "Redirect URL has no host".to_string(),
        ));
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| CamelError::ProcessorError("Redirect URL has no port".to_string()))?;

    // If the host is an IP literal, check it directly
    if let Ok(ip) = host_str.parse::<std::net::IpAddr>() {
        let is_blocked = is_ssrf_blocked_ip(&ip);
        if !allow_internal && is_blocked {
            return Err(CamelError::ProcessorError(format!(
                "Redirect target is a blocked IP: {}",
                ip
            )));
        }
        // Under allow_internal: reject public IPs with HTTP
        if allow_internal && !is_blocked && url.scheme() == "http" {
            return Err(CamelError::ProcessorError(format!(
                "Redirect to public IP '{}' not allowed over HTTP (use HTTPS)",
                ip
            )));
        }
        return Ok(vec![std::net::SocketAddr::new(ip, port)]);
    }

    // Domain name: use shared resolver with DNS timeout (always resolves for pinning)
    let addrs = resolve_and_validate_host(host_str, port, allow_internal)
        .await
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to resolve redirect host '{host_str}': {e}"))
        })?;

    // Under allow_internal with HTTP: reject if any resolved IP is public
    if allow_internal
        && url.scheme() == "http"
        && let Some(public_addr) = addrs.iter().find(|sa| !is_ssrf_blocked_ip(&sa.ip()))
    {
        return Err(CamelError::ProcessorError(format!(
            "Redirect host '{host_str}' resolves to public IP {} — not allowed over HTTP (use HTTPS)",
            public_addr.ip()
        )));
    }

    Ok(addrs)
}

/// Resolves the initial request URL's hostname, validates all resolved IPs against the
/// SSRF blocklist (`is_ssrf_blocked_ip`), and returns the host + socket addresses for
/// DNS pinning.
///
/// DNS pinning via reqwest's `resolve_to_addrs` closes the TOCTOU window between
/// validation and connection: an attacker cannot rebind the DNS to a private IP after
/// validation succeeds, because reqwest connects directly to the validated addresses.
///
/// Returns `None` when no pinning is needed:
/// - Host is an IP literal (already validated directly in `validate_url_for_ssrf`)
/// - URL has no host
///
/// Under `allow_internal=true`, resolution STILL happens for DNS pinning, but:
/// - If scheme is HTTP and any resolved IP is public → reject
/// - If all resolved IPs are internal → return `Some((host, addrs))` for pinning
///
/// Returns `Some((host, addrs))` with validated addresses + extracted host string
/// so the caller can pass both directly to `build_client(…, Some((&host, &addrs)))`
/// without re-parsing the URL.
pub(crate) async fn resolve_initial_url_for_ssrf(
    url: &str,
    allow_internal: bool,
) -> Result<Option<(String, Vec<std::net::SocketAddr>)>, CamelError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| CamelError::ProcessorError(format!("Invalid URL: {}", e)))?;

    let Some(host_str) = parsed.host_str() else {
        return Ok(None);
    };

    // IP literals are validated directly in validate_url_for_ssrf — no pinning needed
    if host_str.parse::<std::net::IpAddr>().is_ok() {
        return Ok(None);
    }

    let port = parsed.port_or_known_default().ok_or_else(|| {
        CamelError::ProcessorError(format!("URL '{}' has no recognizable port", url))
    })?;

    let host_str_clone = host_str.to_string();
    // Always resolve for DNS pinning — even under allow_internal
    let addrs = resolve_and_validate_host(host_str, port, allow_internal)
        .await
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to resolve host '{host_str_clone}': {e}"))
        })?;

    // Under allow_internal with HTTP: reject if any resolved IP is public
    if allow_internal
        && parsed.scheme() == "http"
        && let Some(public_addr) = addrs.iter().find(|sa| !is_ssrf_blocked_ip(&sa.ip()))
    {
        return Err(CamelError::ProcessorError(format!(
            "Host '{host_str_clone}' resolves to public IP {} — not allowed over HTTP (use HTTPS)",
            public_addr.ip()
        )));
    }

    Ok(Some((host_str.to_string(), addrs)))
}

/// Sends an HTTP request with manual redirect following and per-hop SSRF validation.
///
/// This replaces reqwest's built-in redirect following, which cannot perform
/// async DNS resolution or SSRF checks on redirect targets. Each redirect hop:
/// 1. Parses the Location header
/// 2. Rewrites method for 303/301/302 (POST → GET)
/// 3. Strips Authorization/Cookie on cross-origin redirects
/// 4. Resolves the target hostname and validates all IPs against SSRF blocklist
/// 5. Builds a per-hop client with `resolve_to_addrs` to pin the validated IPs
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_with_ssrf_safe_redirects(
    initial_client: &reqwest::Client,
    http_config: &HttpConfig,
    endpoint_config: &HttpEndpointConfig,
    method: reqwest::Method,
    initial_url: &str,
    headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    body: Option<Vec<u8>>,
    max_redirects: usize,
    response_timeout: Option<std::time::Duration>,
) -> Result<reqwest::Response, CamelError> {
    use std::net::SocketAddr;

    let mut current_client = initial_client.clone();
    let mut current_method = method;
    let mut current_url = initial_url.to_string();
    let mut current_headers = headers;
    let mut current_body = body;

    for redirect_count in 0..=max_redirects {
        let mut request = current_client.request(current_method.clone(), &current_url);

        // Apply per-hop response timeout (prevents slow-hop hang)
        if let Some(timeout) = response_timeout {
            request = request.timeout(timeout);
        }

        // Apply headers
        for (name, value) in &current_headers {
            request = request.header(name, value);
        }

        // Apply body
        if let Some(ref body_bytes) = current_body
            && !body_bytes.is_empty()
        {
            request = request.body(body_bytes.clone());
        }

        let response = request
            .send()
            .await
            .map_err(|e| CamelError::ProcessorError(format!("HTTP request failed: {e}")))?;

        let status = response.status().as_u16();

        // Only follow actual redirect statuses. Other 3xx (e.g. 304 Not Modified)
        // are returned as-is — they don't carry a Location header.
        if ![301, 302, 303, 307, 308].contains(&status) {
            return Ok(response);
        }

        // Check if redirect limit reached AFTER receiving the response.
        // max_redirects=0 means "send once, return redirect as-is".
        if redirect_count == max_redirects {
            return Ok(response);
        }

        // Extract Location header
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                CamelError::ProcessorError("Redirect response has no Location header".to_string())
            })?;

        // Parse the redirect target URL relative to the current URL
        let current_parsed = url::Url::parse(&current_url)
            .map_err(|e| CamelError::ProcessorError(format!("Invalid current URL: {}", e)))?;
        let redirect_url = current_parsed
            .join(location)
            .map_err(|e| CamelError::ProcessorError(format!("Invalid redirect Location: {}", e)))?;

        // Determine if this is a cross-origin redirect
        let is_cross_origin = redirect_url.scheme() != current_parsed.scheme()
            || redirect_url.host_str() != current_parsed.host_str()
            || redirect_url.port_or_known_default() != current_parsed.port_or_known_default();

        // Method rewrite: 303 always → GET; 301/302 with POST → GET
        let new_method = if status == 303
            || ((status == 301 || status == 302) && current_method == reqwest::Method::POST)
        {
            reqwest::Method::GET
        } else {
            current_method.clone()
        };

        // Strip sensitive headers on cross-origin redirects
        let new_headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> =
            if is_cross_origin {
                current_headers
                    .into_iter()
                    .filter(|(name, _)| {
                        name != reqwest::header::AUTHORIZATION && name != reqwest::header::COOKIE
                    })
                    .collect()
            } else {
                current_headers.clone()
            };

        // Drop body on method change to GET
        let new_body = if new_method == reqwest::Method::GET {
            None
        } else {
            current_body.clone()
        };

        // SSRF validation: resolve and validate the redirect target
        let resolved_addrs =
            validate_redirect_target_for_ssrf(&redirect_url, endpoint_config.allow_internal)
                .await?;

        // Build a per-hop client with DNS pinning
        let redirect_host = redirect_url.host_str().unwrap_or("");
        let resolved_slice: &[SocketAddr] = &resolved_addrs;
        current_client = build_client(
            http_config,
            endpoint_config.cookie_handling,
            Some((redirect_host, resolved_slice)),
        );

        current_method = new_method;
        current_url = redirect_url.to_string();
        current_headers = new_headers;
        current_body = new_body;
    }

    unreachable!("loop exits via return inside")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpEndpointConfig;
    use camel_component_api::UriConfig;

    #[test]
    fn test_validate_url_for_ssrf_blocks_and_allows_hosts() {
        let mut cfg = HttpEndpointConfig::from_uri("http://example.com").unwrap();
        cfg.blocked_hosts = vec!["blocked.local".to_string()];
        cfg.allow_internal = false;

        let blocked = validate_url_for_ssrf("http://blocked.local/api", &cfg);
        assert!(blocked.is_err());

        let private_ip = validate_url_for_ssrf("http://127.0.0.1/api", &cfg);
        assert!(private_ip.is_err());

        cfg.allow_internal = true;
        let allowed = validate_url_for_ssrf("http://127.0.0.1/api", &cfg);
        assert!(allowed.is_ok());
    }

    /// Under allow_internal=true, public IPs over HTTP are rejected
    #[test]
    fn test_validate_url_rejects_public_http_under_allow_internal() {
        let mut cfg = HttpEndpointConfig::from_uri("http://example.com").unwrap();
        cfg.allow_internal = true;

        // Public IP over HTTP should be rejected
        let result = validate_url_for_ssrf("http://1.1.1.1/api", &cfg);
        assert!(
            result.is_err(),
            "public IP over HTTP should be rejected under allow_internal"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("HTTPS") || err.contains("public"),
            "error should mention HTTPS requirement, got: {err}"
        );

        // Private IP over HTTP should be allowed
        let result = validate_url_for_ssrf("http://127.0.0.1/api", &cfg);
        assert!(
            result.is_ok(),
            "private IP over HTTP should be allowed under allow_internal"
        );
    }

    /// Non-http(s) schemes are rejected under both policies
    #[test]
    fn test_validate_url_rejects_non_http_schemes() {
        let cfg = HttpEndpointConfig::from_uri("http://example.com").unwrap();
        let result = validate_url_for_ssrf("ftp://example.com/file", &cfg);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("scheme") || err.contains("http"), "got: {err}");
    }

    /// Unit test: validate_redirect_target_for_ssrf blocks private IPs
    #[tokio::test]
    async fn test_validate_redirect_target_blocks_private_ip() {
        let url = url::Url::parse("http://127.0.0.1:8080/internal").unwrap();
        let result = validate_redirect_target_for_ssrf(&url, false).await;
        assert!(result.is_err(), "Should block redirect to 127.0.0.1");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked IP") || err.contains("private IP"),
            "Error should mention IP blocking, got: {err}"
        );
    }

    /// Unit test: validate_redirect_target_for_ssrf allows private IPs when configured
    #[tokio::test]
    async fn test_validate_redirect_target_allows_private_ip_when_configured() {
        let url = url::Url::parse("http://127.0.0.1:8080/internal").unwrap();
        let result = validate_redirect_target_for_ssrf(&url, true).await;
        assert!(
            result.is_ok(),
            "Should allow redirect to 127.0.0.1 when allow_internal=true"
        );
    }

    /// DNS-rebinding TOCTOU prevention: resolve_initial_url_for_ssrf validates
    /// that the initial request URL's hostname does not resolve to a private IP.
    /// None = no pinning needed (IP literal).
    #[tokio::test]
    async fn test_resolve_initial_url_for_ssrf_blocks_private_ip() {
        // localhost → 127.0.0.1 → blocked
        let err = resolve_initial_url_for_ssrf("http://localhost:8080/path", false)
            .await
            .expect_err("localhost must resolve to loopback and be blocked");
        assert!(
            matches!(&err, CamelError::ProcessorError(_)),
            "DNS resolution failure at request-execution time should be ProcessorError, not Config, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("private IP") || msg.contains("blocked IP") || msg.contains("SSRF"),
            "Error should mention IP blocking/SSRF, got: {msg}"
        );
    }

    /// When allow_internal=true, resolution STILL happens for DNS pinning.
    /// localhost resolves to 127.0.0.1 (internal), so it returns Some for pinning.
    #[tokio::test]
    async fn test_resolve_initial_url_allow_internal_still_pins() {
        let result = resolve_initial_url_for_ssrf("http://localhost:8080/path", true)
            .await
            .expect("should succeed when allow_internal=true");
        assert!(
            result.is_some(),
            "should return Some for DNS pinning even when allow_internal=true"
        );
        let (host, addrs) = result.unwrap();
        assert_eq!(host, "localhost");
        assert!(
            !addrs.is_empty(),
            "should have resolved addresses for pinning"
        );
    }

    /// IP-literal URLs don't need DNS pinning (validated directly).
    #[tokio::test]
    async fn test_resolve_initial_url_ip_literal_returns_none() {
        let result = resolve_initial_url_for_ssrf("http://127.0.0.1:8080/path", false)
            .await
            .expect("IP literal should return Ok(None)");
        assert!(result.is_none(), "IP literal should return None");
    }

    /// Public hostname resolves to non-blocked IPs and returns Some for pinning.
    #[tokio::test]
    async fn test_resolve_initial_url_public_host_returns_addrs() {
        let result = resolve_initial_url_for_ssrf("http://example.com:80/", false)
            .await
            .expect("example.com should resolve and not be blocked");
        let (host, addrs) = result.expect("example.com should return Some for pinning");
        assert!(
            !addrs.is_empty(),
            "example.com should resolve to at least one addr for pinning"
        );
        assert_eq!(host, "example.com", "should return the hostname unchanged");
    }
}
