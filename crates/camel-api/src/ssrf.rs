//! SSRF (Server-Side Request Forgery) defense helpers.
//!
//! Canonical IP-classification logic shared by every outbound HTTP client
//! in the workspace. Centralising this prevents drift between crates
//! (e.g. one allowing ULA, another not) and makes the rule set auditable
//! in one place.
//!
//! Blocking policy:
//! - IPv4: private, loopback, link-local, broadcast, multicast, unspecified, 0.0.0.0/8,
//!   CGN (100.64.0.0/10), benchmark (198.18.0.0/15), reserved future-use (240.0.0.0/4)
//! - IPv6: loopback, multicast, unspecified, ULA (fc00::/7), link-local (fe80::/10),
//!   deprecated site-local (fec0::/10)
//!
//! Public, routable addresses always return `false`. Domain-name validation
//! is the caller's responsibility — this helper operates on `IpAddr`.

use std::net::IpAddr;

/// Returns `true` if `ip` belongs to a network that must NOT be reached
/// by an outbound HTTP client in this workspace.
///
/// Blocks:
///
/// - **IPv4**: private (RFC 1918), loopback (127.0.0.0/8), link-local
///   (169.254.0.0/16 — cloud metadata), broadcast (255.255.255.255),
///   multicast (224.0.0.0/4), unspecified (0.0.0.0), the entire
///   `0.0.0.0/8` block (first octet 0), carrier-grade NAT
///   (100.64.0.0/10 — RFC 6598), network interconnect benchmark
///   (198.18.0.0/15 — RFC 2544), and the reserved future-use
///   `240.0.0.0/4` block (RFC 1112).
/// - **IPv6**: loopback (::1), multicast (ff00::/8), unspecified (::),
///   unique-local (fc00::/7), link-local (fe80::/10), and deprecated
///   site-local (fec0::/10 — RFC 3879).
///   IPv4-mapped IPv6 (`::ffff:a.b.c.d`) inherits the classification of
///   the embedded IPv4 — required to close the DNS-rebinding bypass where
///   a public AAAA record maps to a private IPv4 in v4-mapped form.
///
/// Public, routable addresses (e.g. 8.8.8.8, 2001:4860:4860::8888) return `false`.
pub fn is_ssrf_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
                // CGN 100.64.0.0/10 (RFC 6598) — carrier-grade NAT range,
                // commonly used as shared address space inside ISPs and
                // occasionally leaked to internal networks.
                || (v4.octets()[0] == 100
                    && (v4.octets()[1] >= 64 && v4.octets()[1] <= 127))
                // Network interconnect benchmark 198.18.0.0/15 (RFC 2544) —
                // reserved for benchmarking; never appears on the public
                // internet, only on lab equipment that an attacker could
                // pivot through.
                || (v4.octets()[0] == 198
                    && (v4.octets()[1] == 18 || v4.octets()[1] == 19))
                // Reserved future-use 240.0.0.0/4 (RFC 1112) — covers
                // 240.0.0.0..255.255.255.254. Currently unallocated;
                // blocking prevents any surprise assignment from becoming
                // an SSRF target.
                || v4.octets()[0] >= 240
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                // ULA fc00::/7 — covers both fc00::/8 and fd00::/8
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // Deprecated site-local fec0::/10 (RFC 3879). Replaced by
                // ULA but still routable in some legacy networks.
                || (v6.segments()[0] & 0xffc0) == 0xfec0
                // IPv4-mapped IPv6: recurse into the embedded IPv4 to
                // close the rebinding bypass.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| {
                        v4.is_private()
                            || v4.is_loopback()
                            || v4.is_link_local()
                            || v4.is_broadcast()
                            || v4.is_multicast()
                            || v4.is_unspecified()
                            || v4.octets()[0] == 0
                            || (v4.octets()[0] == 100
                                && (v4.octets()[1] >= 64 && v4.octets()[1] <= 127))
                            || (v4.octets()[0] == 198
                                && (v4.octets()[1] == 18 || v4.octets()[1] == 19))
                            || v4.octets()[0] >= 240
                    })
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().expect("valid ipv4")) // allow-unwrap
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().expect("valid ipv6")) // allow-unwrap
    }

    // ---- IPv4: blocked ranges ----

    #[test]
    fn blocks_rfc1918_10() {
        assert!(is_ssrf_blocked_ip(&v4("10.0.0.1")));
        assert!(is_ssrf_blocked_ip(&v4("10.255.255.255")));
    }

    #[test]
    fn blocks_rfc1918_172_16() {
        assert!(is_ssrf_blocked_ip(&v4("172.16.1.10")));
        assert!(is_ssrf_blocked_ip(&v4("172.31.255.254")));
    }

    #[test]
    fn blocks_rfc1918_192_168() {
        assert!(is_ssrf_blocked_ip(&v4("192.168.1.1")));
        assert!(is_ssrf_blocked_ip(&v4("192.168.0.0")));
    }

    #[test]
    fn blocks_loopback_v4() {
        assert!(is_ssrf_blocked_ip(&v4("127.0.0.1")));
        assert!(is_ssrf_blocked_ip(&v4("127.255.255.254")));
    }

    #[test]
    fn blocks_link_local_v4() {
        // 169.254/16 — cloud metadata endpoints
        assert!(is_ssrf_blocked_ip(&v4("169.254.169.254")));
        assert!(is_ssrf_blocked_ip(&v4("169.254.1.1")));
    }

    #[test]
    fn blocks_broadcast_v4() {
        assert!(is_ssrf_blocked_ip(&v4("255.255.255.255")));
    }

    #[test]
    fn blocks_multicast_v4() {
        assert!(is_ssrf_blocked_ip(&v4("224.0.0.1")));
        assert!(is_ssrf_blocked_ip(&v4("239.255.255.255")));
    }

    #[test]
    fn blocks_unspecified_v4() {
        assert!(is_ssrf_blocked_ip(&v4("0.0.0.0")));
    }

    #[test]
    fn blocks_zero_octet_v4() {
        // 0.0.0.0/8 — first octet 0, but not the unspecified address
        assert!(is_ssrf_blocked_ip(&v4("0.1.2.3")));
        assert!(is_ssrf_blocked_ip(&v4("0.255.255.255")));
    }

    #[test]
    fn blocks_cgn_v4() {
        // CGN 100.64.0.0/10 (RFC 6598) — first octet 100, second 64..=127
        assert!(is_ssrf_blocked_ip(&v4("100.64.0.0")));
        assert!(is_ssrf_blocked_ip(&v4("100.100.100.100")));
        assert!(is_ssrf_blocked_ip(&v4("100.127.255.255")));
        // 100.63 and 100.128 are NOT CGN
        assert!(!is_ssrf_blocked_ip(&v4("100.63.255.255")));
        assert!(!is_ssrf_blocked_ip(&v4("100.128.0.0")));
    }

    #[test]
    fn blocks_benchmark_v4() {
        // Benchmark 198.18.0.0/15 (RFC 2544) — second octet 18 or 19
        assert!(is_ssrf_blocked_ip(&v4("198.18.0.0")));
        assert!(is_ssrf_blocked_ip(&v4("198.18.255.255")));
        assert!(is_ssrf_blocked_ip(&v4("198.19.255.255")));
        // 198.17 and 198.20 are NOT benchmark
        assert!(!is_ssrf_blocked_ip(&v4("198.17.255.255")));
        assert!(!is_ssrf_blocked_ip(&v4("198.20.0.0")));
    }

    #[test]
    fn blocks_reserved_v4() {
        // Reserved 240.0.0.0/4 — first octet >= 240
        assert!(is_ssrf_blocked_ip(&v4("240.0.0.0")));
        assert!(is_ssrf_blocked_ip(&v4("241.1.2.3")));
        assert!(is_ssrf_blocked_ip(&v4("250.100.200.50")));
        // broadcast 255.255.255.255 already covered by is_broadcast
        assert!(is_ssrf_blocked_ip(&v4("255.255.255.255")));
        // 239.x is the top of multicast (224.0.0.0/4), NOT reserved —
        // multicast is still blocked, but via a different rule.
        assert!(is_ssrf_blocked_ip(&v4("239.255.255.255")));
        // 100.x is CGN, blocked, not reserved
        assert!(is_ssrf_blocked_ip(&v4("100.100.100.100")));
    }

    // ---- IPv4: allowed ranges ----

    #[test]
    fn allows_public_dns_v4() {
        assert!(!is_ssrf_blocked_ip(&v4("8.8.8.8")));
        assert!(!is_ssrf_blocked_ip(&v4("1.1.1.1")));
    }

    #[test]
    fn allows_public_edge_v4() {
        // 172.15 and 172.32 are NOT RFC-1918 (only 172.16/12 is)
        assert!(!is_ssrf_blocked_ip(&v4("172.15.255.255")));
        assert!(!is_ssrf_blocked_ip(&v4("172.32.0.0")));
    }

    // ---- IPv6: blocked ranges ----

    #[test]
    fn blocks_loopback_v6() {
        assert!(is_ssrf_blocked_ip(&v6("::1")));
    }

    #[test]
    fn blocks_unspecified_v6() {
        assert!(is_ssrf_blocked_ip(&v6("::")));
    }

    #[test]
    fn blocks_multicast_v6() {
        assert!(is_ssrf_blocked_ip(&v6("ff02::1")));
        assert!(is_ssrf_blocked_ip(&v6("ff00::1")));
    }

    #[test]
    fn blocks_ula_fc_v6() {
        assert!(is_ssrf_blocked_ip(&v6("fc00::1")));
        assert!(is_ssrf_blocked_ip(&v6("fc00:1234:abcd::1")));
    }

    #[test]
    fn blocks_ula_fd_v6() {
        assert!(is_ssrf_blocked_ip(&v6("fd00::1")));
        assert!(is_ssrf_blocked_ip(&v6("fd12:3456:789a::1")));
    }

    #[test]
    fn blocks_link_local_v6() {
        assert!(is_ssrf_blocked_ip(&v6("fe80::1")));
        // fe80::/10 covers fe80..febf
        assert!(is_ssrf_blocked_ip(&v6("febf:ffff::1")));
        // febf + 1 is site-local, which is also blocked (separate test below)
    }

    #[test]
    fn blocks_site_local_v6() {
        // fec0::/10 (RFC 3879) — deprecated site-local, but still
        // routable in some legacy networks.
        assert!(is_ssrf_blocked_ip(&v6("fec0::1")));
        assert!(is_ssrf_blocked_ip(&v6("feff:ffff::1")));
        // febf is link-local, blocked by the fe80::/10 rule
        assert!(is_ssrf_blocked_ip(&v6("febf::1")));
        // ff00::/8 is multicast, already blocked
        assert!(is_ssrf_blocked_ip(&v6("ff00::1")));
    }

    // ---- IPv6: allowed ranges ----

    #[test]
    fn allows_public_dns_v6() {
        assert!(!is_ssrf_blocked_ip(&v6("2001:4860:4860::8888")));
    }

    #[test]
    fn allows_public_documentation_v6() {
        assert!(!is_ssrf_blocked_ip(&v6("2001:db8::1")));
    }
}
