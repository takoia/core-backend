//! Outbound-request safety: validate user-influenced URLs before the server
//! fetches them, to prevent SSRF (cloud metadata 169.254.169.254, localhost
//! admin ports, RFC1918 internal services, …). Resolve the host and reject any
//! private / loopback / link-local / metadata address.

use anyhow::{anyhow, bail, Result};
use std::net::IpAddr;

/// Validate that `url` is an http(s) URL whose host resolves only to public
/// addresses. Returns an error otherwise.
pub async fn validate_outbound_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("invalid URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => bail!("URL scheme '{other}' is not allowed"),
    }
    let host = parsed.host_str().ok_or_else(|| anyhow!("URL has no host"))?;
    let port = parsed.port_or_known_default().unwrap_or(443).max(1);
    let mut resolved = false;
    for addr in tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| anyhow!("cannot resolve host '{host}': {e}"))?
    {
        resolved = true;
        if is_blocked(addr.ip()) {
            bail!("destination resolves to a blocked private/internal address");
        }
    }
    if !resolved {
        bail!("host '{host}' did not resolve");
    }
    Ok(())
}

/// A `reqwest` client that does NOT follow redirects (so an allow-listed host
/// cannot 302 the request into an internal address).
pub fn safe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
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
                // carrier-grade NAT 100.64.0.0/10
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || v6.to_ipv4_mapped().map(|m| is_blocked(IpAddr::V4(m))).unwrap_or(false)
        }
    }
}
