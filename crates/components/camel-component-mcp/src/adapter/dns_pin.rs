//! DNS pinning for remote MCP servers (audit 2026-08-31 R4 / rc-juqrd).
//!
//! Parity with the camel-http component's SSRF posture: a hostname-based
//! remote is resolved ONCE, every resolved IP is validated against
//! [`camel_api::is_ssrf_blocked_ip`], and the outbound reqwest client is
//! pinned to the validated addresses with
//! [`reqwest::ClientBuilder::resolve_to_addrs`]. rmcp then speaks through
//! that client ([`rmcp::transport::StreamableHttpClientTransport::with_client`]),
//! closing the DNS-rebinding TOCTOU window between validation and
//! connection: a rebinding attack cannot steer the connection to an
//! internal target after validation passes, because the connection never
//! consults DNS again.
//!
//! Mirrors the camel-http rules:
//!
//! - default (`allow_internal = false`): any resolved blocked IP rejects
//!   the whole resolution (fail closed, not filter-and-continue);
//! - cleartext `http://` with any resolved public IP rejects unless
//!   `allow_cleartext=true` (uniform rule, ADR-0081; independent of
//!   `allow_internal`);
//! - IP-literal URLs are NOT resolved here — they were already validated
//!   as literals at config load ([`crate::config::McpRemoteConfig::validate_url`]).

use std::net::SocketAddr;

use camel_api::is_ssrf_blocked_ip;
use camel_api::redact::redact_url_fail_closed;

use crate::error::McpError;

/// Echo a remote URL into an error message through the canonical redactor
/// (audit 2026-08-31 F5-4): raw URL bytes never enter an error unmasked.
fn masked(url: &str) -> String {
    redact_url_fail_closed(url)
}

/// Split a remote URL into `(scheme, host, port)`.
///
/// String-based (this crate has no `url` dependency): the authority is
/// between `://` and the next `/`; userinfo is stripped; a bracketed IPv6
/// literal is handled. Port defaults to 80 (`http`) / 443 (`https`).
pub(super) fn split_authority(url: &str) -> Result<(String, String, u16), McpError> {
    let lower = url.to_ascii_lowercase();
    let scheme = if lower.starts_with("https://") {
        "https"
    } else if lower.starts_with("http://") {
        "http"
    } else {
        return Err(McpError::Endpoint(format!(
            "remote url must use http:// or https:// (got '{}')",
            masked(url)
        )));
    };

    let after_scheme = &url[scheme.len() + 3..];
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let authority = authority.rsplit('@').next().unwrap_or(authority);

    // Bracketed IPv6: `[::1]:8000` or bare `[::1]`.
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (literal, tail) = rest.split_once(']').ok_or_else(|| {
            McpError::Endpoint(format!("unterminated IPv6 literal in '{}'", masked(url)))
        })?;
        let port = match tail.strip_prefix(':') {
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| McpError::Endpoint(format!("invalid port in '{}'", masked(url))))?,
            None if tail.is_empty() => default_port(scheme),
            None => {
                return Err(McpError::Endpoint(format!(
                    "invalid authority in '{}'",
                    masked(url)
                )));
            }
        };
        (literal.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => {
                let port = p.parse::<u16>().map_err(|_| {
                    McpError::Endpoint(format!("invalid port in '{}'", masked(url)))
                })?;
                (h.to_string(), port)
            }
            None => (authority.to_string(), default_port(scheme)),
        }
    };

    if host.is_empty() {
        return Err(McpError::Endpoint(format!(
            "empty host in '{}'",
            masked(url)
        )));
    }
    // `resolve_to_addrs` is an exact-string key match against the host the
    // `url` crate normalizes inside reqwest/rmcp. A trailing dot or a
    // non-ASCII (IDN) host would make the override key silently miss and
    // fall back to the system resolver — fail closed with a clear error
    // instead (e_glm stage-4 finding 4).
    if host.ends_with('.') || !host.is_ascii() {
        return Err(McpError::Endpoint(format!(
            "remote host '{host}' must be a plain ASCII name without a trailing dot \
             (DNS-pinning key normalization)"
        )));
    }
    Ok((scheme.to_string(), host, port))
}

fn default_port(scheme: &str) -> u16 {
    if scheme == "https" { 443 } else { 80 }
}

/// Validate a DNS resolution against the SSRF policy (pure; TDD surface).
///
/// - `!allow_internal`: any blocked IP rejects the whole resolution.
/// - `http` + `!allow_cleartext`: any public IP rejects (uniform cleartext
///   rule, ADR-0081; independent of `allow_internal`).
/// - Empty resolution rejects (fail closed — an unresolvable host is a
///   configuration error, not a pass).
pub(super) fn validate_resolution(
    addrs: &[SocketAddr],
    scheme: &str,
    allow_internal: bool,
    allow_cleartext: bool,
) -> Result<(), McpError> {
    if addrs.is_empty() {
        return Err(McpError::Endpoint(
            "remote host did not resolve to any addresses".to_string(),
        ));
    }
    if !allow_internal && let Some(blocked) = addrs.iter().find(|sa| is_ssrf_blocked_ip(&sa.ip())) {
        return Err(McpError::Endpoint(format!(
            "remote host resolves to a blocked/internal address ({}); \
             set allow_internal=true to override",
            blocked.ip()
        )));
    }
    if scheme == "http"
        && !allow_cleartext
        && let Some(public) = addrs.iter().find(|sa| !is_ssrf_blocked_ip(&sa.ip()))
    {
        return Err(McpError::Endpoint(format!(
            "remote host resolves to public address {} — not allowed over \
             cleartext http:// (set allow_cleartext=true or use https://)",
            public.ip()
        )));
    }
    Ok(())
}

/// Build the DNS-pinned reqwest client for one remote.
///
/// Hardening mirrors rmcp's `default_http_client` (no idle pooling, no
/// redirects) plus the camel-http rule that environment proxies bypass
/// `resolve_to_addrs` — hence `.no_proxy()`. IP-literal hosts skip
/// resolution (already validated as literals at config load).
pub(super) async fn build_pinned_http_client(
    url: &str,
    allow_internal: bool,
    allow_cleartext: bool,
) -> Result<reqwest::Client, McpError> {
    let (scheme, host, port) = split_authority(url)?;

    // IP literals: validated at config load; no pinning needed (there is
    // no name to rebind).
    if host.parse::<std::net::IpAddr>().is_ok() {
        return hardened_client_builder()
            .build()
            .map_err(|e| McpError::Endpoint(format!("failed to build MCP http client: {e}")));
    }

    // Bounded resolution — parity with the camel-http resolver's DNS
    // timeout; a hanging resolver must not hang route start (e_glm
    // stage-4 finding 3).
    let lookup = tokio::net::lookup_host((host.as_str(), port));
    let addrs: Vec<SocketAddr> = tokio::time::timeout(RESOLVE_TIMEOUT, lookup)
        .await
        .map_err(|_| McpError::Endpoint(format!("timed out resolving remote host '{host}'")))?
        .map_err(|e| McpError::Endpoint(format!("failed to resolve remote host '{host}': {e}")))?
        .collect();
    validate_resolution(&addrs, &scheme, allow_internal, allow_cleartext)?;

    // Key the DNS override on the lowercased host: the `url` crate (used
    // inside reqwest/rmcp to parse the remote URL) normalizes
    // special-scheme hosts to lowercase, and `resolve_to_addrs` is an
    // exact-string key match — an original-case key would silently miss
    // and fall back to the system resolver, reopening the rebinding
    // window (r_glm holistic finding 3).
    let host_key = host.to_ascii_lowercase();
    hardened_client_builder()
        .resolve_to_addrs(host_key.as_str(), &addrs)
        .build()
        .map_err(|e| McpError::Endpoint(format!("failed to build MCP http client: {e}")))
}

/// DNS resolution timeout — mirrors the camel-http resolver's 5s budget.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn hardened_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy() // CRITICAL: env proxies bypass resolve_to_addrs
        .pool_max_idle_per_host(0) // mirror rmcp default_http_client
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(RESOLVE_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse().expect("test IP literal"), port)
    }

    // ── split_authority ────────────────────────────────────────────────

    #[test]
    fn split_authority_plain_https() {
        let (scheme, host, port) =
            split_authority("https://mcp.example.com/sse").expect("valid URL");
        assert_eq!(
            (scheme.as_str(), host.as_str(), port),
            ("https", "mcp.example.com", 443)
        );
    }

    #[test]
    fn split_authority_http_with_port() {
        let (scheme, host, port) =
            split_authority("http://internal.corp:8080/mcp").expect("valid URL");
        assert_eq!(
            (scheme.as_str(), host.as_str(), port),
            ("http", "internal.corp", 8080)
        );
    }

    #[test]
    fn split_authority_strips_userinfo() {
        let (_, host, _) =
            split_authority("https://user:pass@mcp.example.com/mcp").expect("valid URL");
        assert_eq!(host, "mcp.example.com");
    }

    #[test]
    fn split_authority_ipv6_bracketed_with_port() {
        let (_, host, port) = split_authority("http://[2001:db8::1]:9000/mcp").expect("valid URL");
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, 9000);
    }

    #[test]
    fn split_authority_rejects_trailing_dot_host() {
        // resolve_to_addrs key normalization: a trailing dot would silently
        // defeat pinning (e_glm stage-4 finding 4).
        let err = split_authority("https://mcp.corp./mcp").unwrap_err();
        assert!(err.to_string().contains("trailing dot"), "msg: {err}");
    }

    #[test]
    fn split_authority_rejects_non_ascii_host() {
        let err = split_authority("https://mcp.ñ.example.com/mcp").unwrap_err();
        assert!(err.to_string().contains("ASCII"), "msg: {err}");
    }

    #[test]
    fn split_authority_rejects_non_http_scheme() {
        let err = split_authority("gopher://host/x").unwrap_err();
        assert!(err.to_string().contains("http"), "msg: {err}");
    }

    // ── validate_resolution ────────────────────────────────────────────

    #[test]
    fn resolution_with_blocked_ip_rejected_by_default() {
        let addrs = [addr("93.184.216.34", 443), addr("10.0.0.5", 443)];
        let err = validate_resolution(&addrs, "https", false, false).unwrap_err();
        assert!(err.to_string().contains("blocked"), "msg: {err}");
    }

    #[test]
    fn resolution_https_public_ok_by_default_both_flags() {
        let addrs = [addr("93.184.216.34", 443)];
        assert!(validate_resolution(&addrs, "https", false, false).is_ok());
    }

    #[test]
    fn resolution_blocked_ok_when_allow_internal_https() {
        let addrs = [addr("10.0.0.5", 443), addr("127.0.0.1", 443)];
        assert!(validate_resolution(&addrs, "https", true, false).is_ok());
    }

    #[test]
    fn resolution_public_over_http_rejected_by_default() {
        let addrs = [addr("93.184.216.34", 80)];
        let err = validate_resolution(&addrs, "http", false, false).unwrap_err();
        assert!(err.to_string().contains("cleartext"), "msg: {err}");
        assert!(
            err.to_string().contains("allow_cleartext"),
            "remedy missing: {err}"
        );
    }

    #[test]
    fn resolution_public_over_http_allowed_with_allow_cleartext() {
        let addrs = [addr("93.184.216.34", 80)];
        assert!(validate_resolution(&addrs, "http", false, true).is_ok());
        assert!(validate_resolution(&addrs, "http", true, true).is_ok());
    }

    // Regression pin (ADR-0079 cell): allow_internal=true + public cleartext
    // http still rejects.
    #[test]
    fn resolution_public_over_http_rejected_when_allow_internal() {
        let addrs = [addr("93.184.216.34", 80)];
        let err = validate_resolution(&addrs, "http", true, false).unwrap_err();
        assert!(err.to_string().contains("cleartext"), "msg: {err}");
        assert!(
            err.to_string().contains("allow_cleartext"),
            "remedy missing: {err}"
        );
    }

    // Internal cleartext is NOT governed by the new rule: with
    // allow_internal=true, a resolution of only internal addresses over
    // http:// stays allowed when allow_cleartext=false.
    #[test]
    fn resolution_internal_http_ok_when_allow_internal() {
        let addrs = [addr("10.0.0.5", 80), addr("127.0.0.1", 80)];
        assert!(validate_resolution(&addrs, "http", true, false).is_ok());
    }

    #[test]
    fn empty_resolution_rejected() {
        let err = validate_resolution(&[], "https", true, false).unwrap_err();
        assert!(err.to_string().contains("did not resolve"), "msg: {err}");
    }

    // ── build_pinned_http_client (no-network paths) ────────────────────

    #[tokio::test]
    async fn bad_scheme_url_rejected_before_any_resolution() {
        let err = build_pinned_http_client("ftp://host/x", false, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("http"), "msg: {err}");
    }

    // Audit 2026-08-31 F5-4 (rc-a67at): split_authority error echoes route
    // through the canonical redactor — a query secret never leaks.
    #[test]
    fn split_authority_error_masks_query_secrets() {
        // Port '99x' fails to parse; the query carries a token.
        let err = split_authority("https://host:99x/mcp?token=abc123").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("abc123"), "secret leaked: {msg}");
        assert!(msg.contains("?[redacted]"), "sentinel missing: {msg}");
    }

    #[tokio::test]
    async fn ip_literal_url_builds_client_without_dns() {
        // IP literals skip resolution entirely — no DNS, no pinning.
        let client = build_pinned_http_client("http://127.0.0.1:8000/mcp", true, false)
            .await
            .expect("IP-literal URL must not need DNS");
        drop(client);
    }
}
