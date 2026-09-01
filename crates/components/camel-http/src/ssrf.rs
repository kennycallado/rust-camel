//! SSRF (Server-Side Request Forgery) protection functions.
//!
//! Validates URLs, resolves hostnames, and enforces DNS pinning to prevent
//! attackers from using the HTTP producer to reach internal/private networks.

use std::time::Duration;

use camel_api::is_ssrf_blocked_ip;
use camel_component_api::CamelError;

use crate::config::HttpConfig;
use crate::{HttpEndpointConfig, build_client};

/// Whether a header carries credentials that must not be replayed to a
/// cross-origin redirect target (F2-5). Header names are case-insensitive
/// per RFC 9110; `HeaderName` comparison is already normalized, so match on
/// the lowercase string form.
fn is_sensitive_redirect_header(name: &reqwest::header::HeaderName, is_downgrade: bool) -> bool {
    let n = name.as_str();
    n == "authorization"
        || n == "cookie"
        || n == "x-api-key"
        || n == "x-apikey"
        || n == "x-auth-token"
        || n == "api-key"
        || n == "apikey"
        // Proxy credentials are additionally stripped on https→http downgrade
        // (same-origin proxy creds must never ride a cleartext hop).
        || (is_downgrade && n == "proxy-authorization")
}

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

    // Check blocked hosts (audit 2026-08-31, F2-2). Matching normalizes case
    // and a trailing root dot (`host.` == `host`, per DNS FQDN semantics), and
    // treats a blocklist entry as covering its subdomains: blocking
    // `internal.example` must also block `api.internal.example` — operators
    // read a blocklist entry as "this host and everything under it".
    if let Some(host) = parsed.host_str() {
        let norm_host = host.trim_end_matches('.').to_ascii_lowercase();
        let is_blocked = config.blocked_hosts.iter().any(|blocked| {
            let norm_blocked = blocked.trim_end_matches('.').to_ascii_lowercase();
            norm_host == norm_blocked
                || norm_host
                    .strip_suffix(norm_blocked.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        });
        if is_blocked {
            return Err(CamelError::ProcessorError(format!(
                "Host '{}' is blocked",
                host
            )));
        }
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

    // An empty resolution must fail closed on both branches: under
    // allow_internal=true it would otherwise return an empty pin set, which
    // collapses the port distinction in the pinned-client cache key and gives
    // reqwest zero addresses at connect time.
    if resolved.is_empty() {
        return Err(format!("host '{host}' did not resolve to any addresses"));
    }

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
/// 5. Builds the per-hop client with `resolve_to_addrs` from the endpoint's
///    pinned-client cache (hostname targets) or reuses the shared unpinned
///    client (IP-literal targets)
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_with_ssrf_safe_redirects(
    initial_client: &reqwest::Client,
    shared_client: &reqwest::Client,
    pinned_cache: &crate::client_cache::PinnedClientCache,
    http_config: &HttpConfig,
    endpoint_config: &HttpEndpointConfig,
    method: reqwest::Method,
    initial_url: &str,
    headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    body: Option<Vec<u8>>,
    max_redirects: usize,
    response_timeout: Option<std::time::Duration>,
) -> Result<reqwest::Response, CamelError> {
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

        // Strip sensitive headers on cross-origin redirects.
        // Audit 2026-08-31, F2-5: Authorization+Cookie alone let custom
        // credential headers (X-API-Key, X-Auth-Token, …) leak to the redirect
        // target. Use the industry-standard sensitive-header set (matches
        // curl/reqwest defaults) plus common API-key header names; scheme
        // downgrades (https→http) additionally strip Proxy-Authorization.
        let is_downgrade = current_parsed.scheme() == "https" && redirect_url.scheme() == "http";
        let new_headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> =
            if is_cross_origin {
                current_headers
                    .into_iter()
                    .filter(|(name, _)| !is_sensitive_redirect_header(name, is_downgrade))
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

        // Build the per-hop client with DNS pinning: hostname targets build
        // through the endpoint's pinned-client cache; IP-literal targets
        // reuse the shared unpinned client and never enter the cache.
        let redirect_host = redirect_url.host_str().unwrap_or("");
        if redirect_host.parse::<std::net::IpAddr>().is_ok() {
            current_client = shared_client.clone();
        } else {
            current_client = pinned_cache
                .get_or_build(redirect_host, &resolved_addrs, || {
                    build_client(http_config, Some((redirect_host, &resolved_addrs)))
                })
                .await;
        }

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
    use crate::{PINNED_CLIENT_MAX_ENTRIES, PINNED_CLIENT_TTL, PinnedClientCache};
    use camel_component_api::UriConfig;

    /// Audit 2026-08-31, F2-5: sensitive custom headers must be stripped
    /// on cross-origin redirect replay.
    #[test]
    fn test_sensitive_redirect_headers() {
        use reqwest::header::HeaderName;
        let h = |s: &str| HeaderName::from_bytes(s.as_bytes()).unwrap();

        assert!(is_sensitive_redirect_header(&h("authorization"), false));
        assert!(is_sensitive_redirect_header(&h("cookie"), false));
        assert!(is_sensitive_redirect_header(&h("x-api-key"), false));
        assert!(is_sensitive_redirect_header(
            &h("X-Auth-Token".to_lowercase().as_str()),
            false
        ));
        assert!(is_sensitive_redirect_header(
            &h("proxy-authorization"),
            true
        ));
        assert!(!is_sensitive_redirect_header(
            &h("proxy-authorization"),
            false
        ));
        assert!(!is_sensitive_redirect_header(&h("content-type"), false));
        assert!(!is_sensitive_redirect_header(&h("x-request-id"), false));
    }

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

    /// Audit 2026-08-31, F2-2: blocklist matching must not be bypassable by
    /// trailing dots, case shifts, or subdomains.
    #[test]
    fn test_blocked_hosts_normalized_and_subdomain_aware() {
        let mut cfg = HttpEndpointConfig::from_uri("http://example.com").unwrap();
        cfg.blocked_hosts = vec!["blocked.local".to_string()];
        cfg.allow_internal = false;

        for evil in [
            "http://blocked.local/api",      // exact
            "http://blocked.local./api",     // trailing root dot
            "http://BLOCKED.LOCAL/api",      // case
            "http://api.blocked.local/api",  // subdomain
            "http://a.b.blocked.local./api", // nested subdomain + dot
        ] {
            assert!(
                validate_url_for_ssrf(evil, &cfg).is_err(),
                "{evil} must be blocked"
            );
        }

        // Suffix-but-not-subdomain must NOT be blocked (evilblocked.local).
        assert!(
            validate_url_for_ssrf("http://evilblocked.local/api", &cfg).is_ok(),
            "non-subdomain suffix must stay allowed"
        );
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

    /// Local responder that accepts any number of HTTP/1.1 connections on an
    /// ephemeral 127.0.0.1 port and answers each with a minimal 302 redirect
    /// to `location`. Returns `(base_url, JoinHandle)`.
    async fn spawn_302_responder(location: String) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral 127.0.0.1 listener");
        let port = listener.local_addr().expect("local addr").port();
        let base_url = format!("http://localhost:{port}");
        let response =
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n");
        let handle = tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                let _ = conn.write_all(response.as_bytes()).await;
                let _ = conn.shutdown().await;
            }
        });
        (base_url, handle)
    }

    /// Local responder that accepts any number of HTTP/1.1 connections on an
    /// ephemeral 127.0.0.1 port and answers each with a fixed 200 OK.
    /// Returns `(base_url, JoinHandle)`.
    async fn spawn_200_responder() -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral 127.0.0.1 listener");
        let port = listener.local_addr().expect("local addr").port();
        let base_url = format!("http://localhost:{port}");
        let handle = tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                let _ = conn
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = conn.shutdown().await;
            }
        });
        (base_url, handle)
    }

    /// Port of a responder base URL, for tests that must target a different
    /// authority on the same kind of listener.
    fn responder_port(base_url: &str) -> u16 {
        url::Url::parse(base_url)
            .expect("responder base URL parses")
            .port()
            .expect("responder base URL carries an explicit port")
    }

    /// Redirect hops to a hostname target build one pinned client through
    /// the endpoint's cache and reuse it on subsequent calls; the hostname
    /// initial request itself goes through `initial_client` untouched and
    /// never enters the cache.
    #[tokio::test]
    async fn redirect_hostname_target_reuses_cached_client() {
        let (hop_base, _hop_handle) = spawn_200_responder().await;
        let hop_port = responder_port(&hop_base);
        let (entry_base, _entry_handle) =
            spawn_302_responder(format!("http://localhost:{hop_port}/hop")).await;
        let entry_port = responder_port(&entry_base);

        let cache = PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES);
        // build_client sets redirect Policy::none(); a bare reqwest::Client
        // would auto-follow the 302 before the manual loop sees it.
        let shared = build_client(&HttpConfig::default(), None);
        let endpoint_config = HttpEndpointConfig::from_uri("http://localhost/?allowInternal=true")
            .expect("endpoint config parses");

        for _ in 0..2 {
            let response = send_with_ssrf_safe_redirects(
                &shared,
                &shared,
                &cache,
                &HttpConfig::default(),
                &endpoint_config,
                reqwest::Method::GET,
                &format!("http://localhost:{entry_port}/start"),
                vec![],
                None,
                3,
                None,
            )
            .await
            .expect("redirect-following request succeeds");
            assert_eq!(response.status(), 200);
            assert_eq!(
                cache.build_count(),
                1,
                "only the localhost:{hop_port} hop enters the pinned \
                 cache — the initial request uses initial_client \
                 untouched, and the second call must reuse the hop entry"
            );
        }
    }

    /// A redirect hop to an IP-literal target bypasses the pinned cache and
    /// reuses the shared unpinned client; an IP-literal initial request
    /// likewise never enters the cache.
    #[tokio::test]
    async fn redirect_ip_literal_target_bypasses_cache() {
        let (hop_base, _hop_handle) = spawn_200_responder().await;
        let hop_port = responder_port(&hop_base);
        let (entry_base, _entry_handle) =
            spawn_302_responder(format!("http://127.0.0.1:{hop_port}/hop")).await;
        let entry_port = responder_port(&entry_base);

        let cache = PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES);
        let shared = build_client(&HttpConfig::default(), None);
        let endpoint_config = HttpEndpointConfig::from_uri("http://localhost/?allowInternal=true")
            .expect("endpoint config parses");

        let response = send_with_ssrf_safe_redirects(
            &shared,
            &shared,
            &cache,
            &HttpConfig::default(),
            &endpoint_config,
            reqwest::Method::GET,
            &format!("http://127.0.0.1:{entry_port}/start"),
            vec![],
            None,
            3,
            None,
        )
        .await
        .expect("redirect-following request succeeds");

        assert_eq!(response.status(), 200);
        assert_eq!(
            cache.build_count(),
            0,
            "neither the literal initial request nor the literal hop may \
             enter the pinned cache"
        );
    }
}
