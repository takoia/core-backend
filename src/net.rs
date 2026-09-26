//! Outbound HTTP guard rails: SSRF validation with DNS pinning, and HTTP
//! clients that always carry timeouts.

use anyhow::{anyhow, bail, Result};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// How long a connection attempt may take before giving up, for every client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Validate that `url` is an http(s) URL whose host resolves only to public
/// addresses, and return those addresses so the request can be pinned to them
/// (see [`pinned_client`]). Validating and then letting the HTTP client resolve
/// the name again would let a rebinding DNS answer point the real request at
/// an internal address.
pub async fn validate_outbound_url(url: &str) -> Result<Vec<SocketAddr>> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("invalid URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => bail!("URL scheme '{other}' is not allowed"),
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host"))?;
    let port = parsed.port_or_known_default().unwrap_or(443).max(1);
    let mut addrs = Vec::new();
    for addr in tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| anyhow!("cannot resolve host '{host}': {e}"))?
    {
        if is_blocked(addr.ip()) {
            bail!("destination resolves to a blocked private/internal address");
        }
        addrs.push(addr);
    }
    if addrs.is_empty() {
        bail!("host '{host}' did not resolve");
    }
    Ok(addrs)
}

/// A client for one validated URL: the host name is pinned to the addresses
/// [`validate_outbound_url`] just checked, redirects are not followed (an
/// allow-listed host cannot 302 the request into an internal address), and
/// the whole request is bounded by `timeout` — seconds for a webhook, minutes
/// for a remote agent run, so the caller decides.
pub fn pinned_client(
    url: &str,
    addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<reqwest::Client> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("invalid URL"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host"))?;
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .map_err(|e| anyhow!("failed to build HTTP client: {e}"))
}

/// The shared general-purpose client: connect timeout built in, TLS roots and
/// connection pool initialised once per process. Callers set the overall bound
/// per request with `.timeout(..)` on the builder. Every outbound call goes
/// through this or [`pinned_client`]; a bare `reqwest::Client::new()` has no
/// timeout and a hung peer would pin a worker for good.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || o[0] == 0
                // Carrier-grade NAT 100.64.0.0/10.
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || v6
                    .to_ipv4_mapped()
                    .map(|m| is_blocked(IpAddr::V4(m)))
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_every_internal_range() {
        for a in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "255.255.255.255",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_blocked(ip(a)), "{a} must be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for a in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2001:4860:4860::8888",
        ] {
            assert!(!is_blocked(ip(a)), "{a} must be allowed");
        }
    }

    #[tokio::test]
    async fn rejects_non_http_schemes_and_literal_internal_hosts() {
        assert!(validate_outbound_url("ftp://example.com/x").await.is_err());
        assert!(validate_outbound_url("file:///etc/passwd").await.is_err());
        assert!(validate_outbound_url("http://127.0.0.1:8080/api")
            .await
            .is_err());
        assert!(validate_outbound_url("http://169.254.169.254/latest")
            .await
            .is_err());
        assert!(validate_outbound_url("not a url").await.is_err());
    }

    #[test]
    fn pinned_client_needs_a_host() {
        let addrs = vec!["93.184.216.34:443".parse().unwrap()];
        let t = Duration::from_secs(5);
        assert!(pinned_client("https://example.com/hook", &addrs, t).is_ok());
        assert!(pinned_client("mailto:x@y", &addrs, t).is_err());
    }
}
