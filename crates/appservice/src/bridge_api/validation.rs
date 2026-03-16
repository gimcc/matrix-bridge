/// Validate that a webhook URL uses an allowed scheme.
/// When `ssrf_protection` is enabled, also blocks localhost, cloud metadata
/// endpoints, and private/reserved IP ranges (RFC1918, link-local, CGNAT, etc.).
/// DNS names are resolved to catch rebinding attacks (e.g., `127.0.0.1.nip.io`).
pub(crate) async fn validate_webhook_url(url: &str, ssrf_protection: bool) -> Result<(), String> {
    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("unsupported scheme: {other}")),
    }

    if !ssrf_protection {
        return Ok(());
    }

    let host = parsed.host_str().ok_or("missing host")?;

    // Block well-known dangerous hostnames.
    let blocked_hosts = ["localhost", "metadata.google.internal"];
    if blocked_hosts.contains(&host) {
        return Err(format!("blocked host: {host}"));
    }

    // Parse as IP and block private/reserved ranges.
    if let Ok(ip) = host.parse::<std::net::IpAddr>()
        && is_private_ip(ip)
    {
        return Err(format!("blocked private/reserved IP: {ip}"));
    }
    // Also try stripping brackets for IPv6 (e.g., "[::1]").
    let stripped = host.trim_start_matches('[').trim_end_matches(']');
    if stripped != host
        && let Ok(ip) = stripped.parse::<std::net::IpAddr>()
        && is_private_ip(ip)
    {
        return Err(format!("blocked private/reserved IP: {ip}"));
    }

    // Resolve DNS names to catch rebinding attacks (e.g., 127.0.0.1.nip.io).
    // Only check if the host is not already a raw IP address.
    if host.parse::<std::net::IpAddr>().is_err() && stripped.parse::<std::net::IpAddr>().is_err() {
        let port = parsed
            .port()
            .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
        let authority = format!("{host}:{port}");
        if let Ok(addrs) = tokio::net::lookup_host(&authority).await {
            for addr in addrs {
                if is_private_ip(addr.ip()) {
                    return Err(format!(
                        "host {host} resolves to blocked private/reserved IP: {}",
                        addr.ip()
                    ));
                }
            }
        }
        // If DNS resolution fails, the webhook will fail at delivery time anyway.
    }

    Ok(())
}

/// Check if an IP address belongs to a private, loopback, link-local,
/// or otherwise reserved range that should not be reachable via webhooks.
fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.0.0.0/8
            || v4.is_private()        // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()     // 169.254.0.0/16
            || v4.is_unspecified()    // 0.0.0.0
            || v4.is_broadcast()      // 255.255.255.255
            || v4.is_documentation()  // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
            || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()          // ::1
            || v6.is_unspecified()    // ::
            || (seg[0] & 0xfe00) == 0xfc00  // fc00::/7 (unique local address)
            || (seg[0] & 0xffc0) == 0xfe80  // fe80::/10 (link-local)
            // Check for IPv4-mapped IPv6 (::ffff:x.x.x.x).
            || match v6.to_ipv4_mapped() {
                Some(v4) => is_private_ip(std::net::IpAddr::V4(v4)),
                None => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[tokio::test]
    async fn validate_webhook_url_accepts_valid_http() {
        assert!(validate_webhook_url("http://example.com/webhook", true).await.is_ok());
        assert!(validate_webhook_url("https://example.com/webhook", true).await.is_ok());
        assert!(validate_webhook_url("https://hooks.slack.com/services/abc", false).await.is_ok());
    }

    #[tokio::test]
    async fn validate_webhook_url_rejects_non_http_schemes() {
        let err = validate_webhook_url("ftp://example.com/file", true).await.unwrap_err();
        assert!(err.contains("unsupported scheme"), "got: {err}");
        let err = validate_webhook_url("file:///etc/passwd", true).await.unwrap_err();
        assert!(err.contains("unsupported scheme"), "got: {err}");
        let err = validate_webhook_url("javascript:alert(1)", false).await.unwrap_err();
        assert!(err.contains("unsupported scheme"), "got: {err}");
    }

    #[tokio::test]
    async fn validate_webhook_url_rejects_invalid_url() {
        assert!(validate_webhook_url("not a url", true).await.is_err());
        assert!(validate_webhook_url("", true).await.is_err());
    }

    #[tokio::test]
    async fn validate_webhook_url_rejects_private_ips_with_ssrf_on() {
        let err = validate_webhook_url("http://127.0.0.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://10.0.0.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://172.16.0.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://192.168.1.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://169.254.1.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://0.0.0.0/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://100.64.0.1/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err = validate_webhook_url("http://localhost/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
        let err =
            validate_webhook_url("http://metadata.google.internal/v1/metadata", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
    }

    #[tokio::test]
    async fn validate_webhook_url_rejects_ipv6_loopback_with_ssrf_on() {
        let err = validate_webhook_url("http://[::1]/hook", true).await.unwrap_err();
        assert!(err.contains("blocked"), "got: {err}");
    }

    #[tokio::test]
    async fn validate_webhook_url_accepts_private_ips_with_ssrf_off() {
        assert!(validate_webhook_url("http://127.0.0.1/hook", false).await.is_ok());
        assert!(validate_webhook_url("http://10.0.0.1/hook", false).await.is_ok());
        assert!(validate_webhook_url("http://192.168.1.1/hook", false).await.is_ok());
        assert!(validate_webhook_url("http://172.16.0.1/hook", false).await.is_ok());
        assert!(validate_webhook_url("http://localhost/hook", false).await.is_ok());
        assert!(validate_webhook_url("http://[::1]/hook", false).await.is_ok());
    }

    #[tokio::test]
    async fn validate_webhook_url_rejects_url_without_host() {
        let err = validate_webhook_url("http://", true).await.unwrap_err();
        assert!(
            err.contains("invalid URL") || err.contains("missing host"),
            "got: {err}"
        );
        let err = validate_webhook_url("http", true).await.unwrap_err();
        assert!(err.contains("invalid URL"), "got: {err}");
    }

    #[test]
    fn is_private_ip_covers_ipv4_ranges() {
        assert!(is_private_ip("127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("127.255.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("10.0.0.0".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("172.16.0.0".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("172.31.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("192.168.0.0".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("169.254.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("255.255.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("100.64.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("100.127.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("192.0.2.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("198.51.100.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("203.0.113.1".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_private_ip_covers_ipv6_ranges() {
        assert!(is_private_ip("::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("::".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("fc00::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("fd12::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("fe80::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("::ffff:127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("::ffff:10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip(
            "2607:f8b0:4004:800::200e".parse::<IpAddr>().unwrap()
        ));
    }
}
