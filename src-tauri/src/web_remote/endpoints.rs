//! Reachable-endpoint enumeration for the web-remote server.
//!
//! Produces the list of URLs a browser on another device could use to
//! reach this desktop once the server is bound on `0.0.0.0:<port>`:
//!
//! - **loopback** — `127.0.0.1` (a browser secure-context origin, so
//!   clipboard etc. keep working here).
//! - **lan** — every non-loopback IPv4 on a local interface that is not
//!   inside the tailnet CGNAT range.
//! - **tailnet** — interface IPs inside `100.64.0.0/10` plus whatever
//!   `tailscale status --json` reports for this node.
//! - **magicdns** — the node's MagicDNS name (`Self.DNSName`) when a
//!   mesh VPN is present.
//!
//! Everything degrades gracefully: no interfaces, no `tailscale` binary,
//! or a malformed status blob just yields fewer entries, never an error.

use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};

/// A single place a browser could reach this desktop.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Endpoint {
    /// `loopback` | `lan` | `tailnet` | `magicdns`.
    pub kind: String,
    /// IP literal or DNS hostname (no scheme, no port).
    pub host: String,
    pub port: u16,
    /// Ready-to-copy `http://host:port` URL.
    pub url: String,
    /// Whether a browser treats this origin as a secure context. Only
    /// loopback qualifies over plain HTTP; LAN/tailnet need the mesh's
    /// HTTPS serve (or a user proxy) to become secure, which we cannot
    /// detect here — so they report `false` and the UI surfaces the note.
    pub secure: bool,
    /// Short human hint for the settings UI.
    pub label: String,
}

/// `true` if `addr` is inside the tailnet CGNAT range `100.64.0.0/10`.
fn is_tailnet_v4(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    // /10 → first octet 100, second octet in 64..=127.
    o[0] == 100 && (0x40..=0x7f).contains(&o[1])
}

/// Build the URL host portion, bracketing IPv6 literals.
fn url_host(host: &str, is_v6: bool) -> String {
    if is_v6 {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn make_endpoint(kind: &str, host: String, port: u16, secure: bool, label: &str, is_v6: bool) -> Endpoint {
    let url = format!("http://{}:{}", url_host(&host, is_v6), port);
    Endpoint {
        kind: kind.to_string(),
        host,
        port,
        url,
        secure,
        label: label.to_string(),
    }
}

/// Enumerate every endpoint at which a bound server on `port` is reachable.
pub fn list(port: u16) -> Vec<Endpoint> {
    let mut out: Vec<Endpoint> = Vec::new();

    // Loopback is always first — the one secure-context origin.
    out.push(make_endpoint(
        "loopback",
        "127.0.0.1".to_string(),
        port,
        true,
        "This device only (secure context)",
        false,
    ));

    // Interface IPs.
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            let ip = iface.ip();
            match ip {
                IpAddr::V4(v4) => {
                    if is_tailnet_v4(v4) {
                        push_unique(
                            &mut out,
                            make_endpoint(
                                "tailnet",
                                v4.to_string(),
                                port,
                                false,
                                "Over your mesh VPN",
                                false,
                            ),
                        );
                    } else if is_lan_v4(v4) {
                        push_unique(
                            &mut out,
                            make_endpoint(
                                "lan",
                                v4.to_string(),
                                port,
                                false,
                                "Local network (plain HTTP)",
                                false,
                            ),
                        );
                    }
                }
                IpAddr::V6(v6) => {
                    // Skip link-local (fe80::/10) — not routable for a peer
                    // without a scope id, which we cannot express in a URL.
                    if v6.is_loopback() || (v6.segments()[0] & 0xffc0) == 0xfe80 {
                        continue;
                    }
                    // Unique-local (fc00::/7) is LAN-ish; global is also LAN
                    // from the server's point of view. Either way, plain HTTP.
                    push_unique(
                        &mut out,
                        make_endpoint(
                            "lan",
                            v6.to_string(),
                            port,
                            false,
                            "Local network (plain HTTP)",
                            true,
                        ),
                    );
                }
            }
        }
    }

    // Tailnet IPs + MagicDNS from the mesh CLI, if present.
    if let Some(status) = tailscale_status() {
        for ip in status.ips {
            if let Ok(IpAddr::V4(v4)) = ip.parse::<IpAddr>() {
                push_unique(
                    &mut out,
                    make_endpoint("tailnet", v4.to_string(), port, false, "Over your mesh VPN", false),
                );
            } else if let Ok(IpAddr::V6(v6)) = ip.parse::<IpAddr>() {
                push_unique(
                    &mut out,
                    make_endpoint("tailnet", v6.to_string(), port, false, "Over your mesh VPN", true),
                );
            }
        }
        if let Some(dns) = status.dns_name {
            let host = dns.trim_end_matches('.').to_string();
            if !host.is_empty() {
                push_unique(
                    &mut out,
                    make_endpoint(
                        "magicdns",
                        host,
                        port,
                        false,
                        "MagicDNS name (enable your mesh's HTTPS serve for a trusted certificate)",
                        false,
                    ),
                );
            }
        }
    }

    out
}

fn push_unique(out: &mut Vec<Endpoint>, ep: Endpoint) {
    if !out.iter().any(|e| e.host == ep.host && e.kind == ep.kind) {
        out.push(ep);
    }
}

/// RFC 1918 private IPv4 ranges (plus 169.254/16 link-local, which is
/// still LAN-reachable). Excludes the tailnet range (checked earlier).
fn is_lan_v4(addr: Ipv4Addr) -> bool {
    addr.is_private() || addr.is_link_local()
}

struct TailscaleStatus {
    ips: Vec<String>,
    dns_name: Option<String>,
}

/// Shell out to `tailscale status --json` and pull this node's addresses.
/// Returns `None` when the binary is absent, errors, or the output is not
/// the expected shape — the caller then simply omits tailnet entries.
fn tailscale_status() -> Option<TailscaleStatus> {
    // `which` avoids spawning a shell error when tailscale isn't installed.
    if which::which("tailscale").is_err() {
        return None;
    }
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let self_node = value.get("Self")?;
    let ips = self_node
        .get("TailscaleIPs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let dns_name = self_node
        .get("DNSName")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    Some(TailscaleStatus { ips, dns_name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailnet_range_detection() {
        assert!(is_tailnet_v4("100.64.0.1".parse().unwrap()));
        assert!(is_tailnet_v4("100.100.100.100".parse().unwrap()));
        assert!(is_tailnet_v4("100.127.255.255".parse().unwrap()));
        assert!(!is_tailnet_v4("100.63.255.255".parse().unwrap()));
        assert!(!is_tailnet_v4("100.128.0.0".parse().unwrap()));
        assert!(!is_tailnet_v4("192.168.1.5".parse().unwrap()));
        assert!(!is_tailnet_v4("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn lan_range_detection() {
        assert!(is_lan_v4("192.168.1.5".parse().unwrap()));
        assert!(is_lan_v4("10.0.0.7".parse().unwrap()));
        assert!(is_lan_v4("172.16.4.4".parse().unwrap()));
        assert!(!is_lan_v4("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn list_always_includes_loopback_first_and_secure() {
        let eps = list(4377);
        assert_eq!(eps[0].kind, "loopback");
        assert!(eps[0].secure, "loopback must be a secure context");
        assert_eq!(eps[0].url, "http://127.0.0.1:4377");
        // Non-loopback endpoints are never marked secure (plain HTTP).
        for ep in eps.iter().filter(|e| e.kind != "loopback") {
            assert!(!ep.secure, "{} should not be secure over HTTP", ep.host);
        }
    }

    #[test]
    fn ipv6_url_host_is_bracketed() {
        let ep = make_endpoint("lan", "fd00::1".to_string(), 4377, false, "x", true);
        assert_eq!(ep.url, "http://[fd00::1]:4377");
    }
}
