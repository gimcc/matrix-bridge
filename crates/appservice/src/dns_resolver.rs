//! Custom DNS resolver that blocks private/reserved IP addresses at connect time.
//!
//! This mitigates SSRF DNS rebinding attacks: even if a hostname resolves to a
//! public IP during webhook URL validation, the actual HTTP connection is still
//! protected because this resolver filters out private IPs when `reqwest`
//! performs its own resolution at connect time.

use std::net::{IpAddr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// A DNS resolver that wraps the system resolver and filters out
/// private/reserved IP addresses from the results.
///
/// If **all** resolved addresses are private, the resolution returns an error,
/// preventing the HTTP client from connecting to internal services.
pub struct SafeDnsResolver;

impl SafeDnsResolver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SafeDnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolve for SafeDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Use port 0 as a placeholder; tokio::net::lookup_host requires "host:port".
            let authority = format!("{host}:0");

            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&authority)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();

            if addrs.is_empty() {
                return Err(format!("DNS resolution returned no addresses for {host}").into());
            }

            let safe_addrs: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|a| !is_private_ip(a.ip()))
                .collect();

            if safe_addrs.is_empty() {
                return Err(format!(
                    "all resolved addresses for {host} are private/reserved (blocked by SSRF protection)"
                )
                .into());
            }

            let addrs: Addrs = Box::new(safe_addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// Check if an IP address belongs to a private, loopback, link-local,
/// or otherwise reserved range that should not be reachable via webhooks.
pub(crate) fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.0.0.0/8
            || v4.is_private()        // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()     // 169.254.0.0/16
            || v4.is_unspecified()    // 0.0.0.0
            || v4.is_broadcast()      // 255.255.255.255
            || v4.is_documentation()  // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
            || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()          // ::1
            || v6.is_unspecified()    // ::
            || (seg[0] & 0xfe00) == 0xfc00  // fc00::/7 (unique local address)
            || (seg[0] & 0xffc0) == 0xfe80  // fe80::/10 (link-local)
            // Check for IPv4-mapped IPv6 (::ffff:x.x.x.x).
            || match v6.to_ipv4_mapped() {
                Some(v4) => is_private_ip(IpAddr::V4(v4)),
                None => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
    }

    #[test]
    fn test_public_ipv4() {
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn test_private_ipv6() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        // fc00::/7 (unique local)
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fe80::/10 (link-local)
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn test_public_ipv6() {
        // 2001:4860:4860::8888 (Google DNS)
        assert!(!is_private_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
        ))));
    }

    #[test]
    fn test_ipv4_mapped_ipv6() {
        // ::ffff:127.0.0.1
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_private_ip(IpAddr::V6(mapped)));

        // ::ffff:8.8.8.8
        let mapped_public = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0808, 0x0808);
        assert!(!is_private_ip(IpAddr::V6(mapped_public)));
    }
}
