//! Reachable-endpoint enumeration for the web-remote server.
//!
//! Produces the list of URLs a browser on another device could use to
//! reach this desktop once the server is bound on `0.0.0.0:<port>`:
//!
//! - **loopback** — `127.0.0.1` (a browser secure-context origin, so
//!   clipboard etc. keep working here).
//! - **lan** — every non-loopback *private* IPv4 on a *real* local interface
//!   that is not inside the tailnet CGNAT range (plus marginal addresses such
//!   as unique-local IPv6).
//! - **public** — a globally routable IPv4/IPv6 on a real interface. The only
//!   way a server-class host (a VPS whose one address is a public IPv4) is
//!   reachable at all, so it is surfaced and — absent anything better —
//!   recommended.
//! - **tailnet** — interface IPs inside `100.64.0.0/10` plus whatever
//!   `tailscale status --json` reports for this node.
//! - **magicdns** — the node's MagicDNS name (`Self.DNSName`) when
//!   Tailscale is present.
//!
//! Docker bridges, `veth` pairs, libvirt/VM host-only nets, Kubernetes CNI
//! plumbing, ZeroTier, and the Tailscale tunnel are skipped by interface
//! name — their `172.x`/`169.254.x`/etc. addresses are noise, not a place a
//! human wants to point a browser.
//!
//! Each endpoint also carries a coarse `group` (`this_device` |
//! `local_network` | `tailscale` | `public_internet` | `other`) that the
//! settings UI renders as labelled sections, and a single `recommended` hint
//! pointing at the best "from anywhere" option.
//!
//! Everything degrades gracefully: no interfaces, no `tailscale` binary,
//! or a malformed status blob just yields fewer entries, never an error.

use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A single place a browser could reach this desktop.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Endpoint {
    /// `loopback` | `lan` | `public` | `tailnet` | `magicdns`.
    pub kind: String,
    /// Coarse UI grouping: `this_device` | `local_network` | `tailscale` |
    /// `public_internet` | `other`. Drives the labelled sections in the
    /// settings panel.
    pub group: String,
    /// IP literal or DNS hostname (no scheme, no port).
    pub host: String,
    pub port: u16,
    /// Ready-to-copy `http://host:port` URL.
    pub url: String,
    /// Whether a browser treats this origin as a secure context. Only
    /// loopback qualifies over plain HTTP; LAN/tailnet need Tailscale's
    /// HTTPS serve (or a user proxy) to become secure, which we cannot
    /// detect here — so they report `false` and the UI surfaces the note.
    pub secure: bool,
    /// The single best "reach from anywhere" endpoint, surfaced with a
    /// "Recommended" chip. At most one endpoint carries this.
    pub recommended: bool,
    /// Short human hint for the settings UI.
    pub label: String,
}

const GROUP_THIS_DEVICE: &str = "this_device";
const GROUP_LOCAL_NETWORK: &str = "local_network";
const GROUP_TAILSCALE: &str = "tailscale";
/// Globally routable addresses — a VPS's public IPv4/IPv6. Kept out of
/// `other` (which is a catch-all disclosure for marginal addresses) because on
/// a server-class host it is *the* way in, and because the internet-exposure
/// note deserves to be visible rather than tucked away.
const GROUP_PUBLIC: &str = "public_internet";
const GROUP_OTHER: &str = "other";
/// The from-anywhere iroh transport's group. New to the UI's fixed set; the
/// frontend grouping helper degrades an unrecognised group to `other`, so this
/// never renders blank while the browser side is still catching up.
const GROUP_RELAY: &str = "relay";

/// Build the iroh transport's endpoint entry, surfaced only when relay mode is
/// on. The `host` is the device's `node_id` — the address a browser dials over
/// iroh — and the URL uses an `iroh://` scheme rather than `http://`. Unlike the
/// HTTP endpoints this is an E2E-encrypted, mutually-authenticated QUIC
/// transport (not a browser origin), so it is always `secure`. Appended after
/// [`mark_recommended`] so it never displaces the HTTP "from anywhere" hint.
pub fn iroh_endpoint(node_id: &str) -> Endpoint {
    Endpoint {
        kind: "iroh".to_string(),
        group: GROUP_RELAY.to_string(),
        host: node_id.to_string(),
        // Not a TCP port — the browser dials the node_id, not host:port.
        port: 0,
        url: format!("iroh://{node_id}"),
        secure: true,
        recommended: false,
        label: "Reach from anywhere (end-to-end encrypted, by node id)".to_string(),
    }
}

/// `true` if `name` looks like a virtual / container / VM / mesh interface
/// whose addresses are not useful "reach this machine" endpoints: Docker
/// bridges (`docker0`, `br-<hash>`), `veth` pairs, libvirt (`virbr`),
/// Kubernetes CNI plumbing (`cni`, `flannel`, `cali`, `kube`), VirtualBox /
/// VMware host-only nets, ZeroTier (`zt`), and the Tailscale tunnel
/// (`tailscale0` — tailnet addresses come from the CLI instead).
///
/// The `br-` prefix is intentionally hyphenated so a legitimate bridge like
/// `br0` is *not* swept up with Docker's `br-<hash>` networks.
fn is_virtual_iface(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "docker", "br-", "veth", "virbr", "cni", "flannel", "cali", "kube", "vboxnet", "vmnet",
        "zt", "tailscale",
    ];
    let n = name.to_ascii_lowercase();
    PREFIXES.iter().any(|p| n.starts_with(p))
}

/// `true` if `addr` is inside the tailnet CGNAT range `100.64.0.0/10`.
fn is_tailnet_v4(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    // /10 → first octet 100, second octet in 64..=127.
    o[0] == 100 && (0x40..=0x7f).contains(&o[1])
}

/// RFC 1918 private IPv4 ranges (plus 169.254/16 link-local, which is still
/// LAN-reachable on the local segment). Excludes the tailnet range (checked
/// earlier). Used to decide *whether* a v4 address is surfaced at all; its
/// display group is refined by `v4.is_private()` vs link-local.
fn is_lan_v4(addr: Ipv4Addr) -> bool {
    addr.is_private() || addr.is_link_local()
}

/// `true` if `addr` is a globally routable IPv4 — the public address of a
/// server-class host. Everything with a special-use meaning is excluded:
/// private/loopback/link-local/broadcast/multicast, the CGNAT range used by
/// the tailnet, documentation ranges, `0.0.0.0/8`, `192.0.0.0/24` (IETF
/// protocol assignments), `198.18.0.0/15` (benchmarking), and the reserved
/// `240.0.0.0/4`. `Ipv4Addr::is_global` would say this for us, but it is
/// still unstable, so the checks live here.
fn is_global_v4(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    !(addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_documentation()
        || addr.is_unspecified()
        || addr.is_multicast()
        || is_tailnet_v4(addr)
        || o[0] == 0
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240)
}

/// `true` if `addr` is a globally routable IPv6. Excludes loopback, the
/// unspecified address, multicast, link-local (`fe80::/10`), unique-local
/// (`fc00::/7`, which covers the tailnet's `fd7a:115c:a1e0::/48`), the
/// documentation prefix (`2001:db8::/32`), and IPv4-mapped addresses (judged
/// by their embedded v4 instead). `Ipv6Addr::is_global` is unstable, hence the
/// hand-rolled version.
fn is_global_v6(addr: Ipv6Addr) -> bool {
    let s = addr.segments();
    let is_v4_mapped = s[0..5].iter().all(|x| *x == 0) && s[5] == 0xffff;
    !(addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || is_v4_mapped)
}

/// The human hint attached to a globally routable address. Deliberately blunt:
/// unlike a LAN or tailnet address this port is reachable by anyone on the
/// internet who finds it, and it is plain HTTP (`secure: false`) either way.
const PUBLIC_LABEL: &str = "Public address (plain HTTP — reachable from the internet)";

/// Build the URL host portion, bracketing IPv6 literals.
fn url_host(host: &str, is_v6: bool) -> String {
    if is_v6 {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn make_endpoint(
    kind: &str,
    group: &str,
    host: String,
    port: u16,
    secure: bool,
    label: &str,
    is_v6: bool,
) -> Endpoint {
    let url = format!("http://{}:{}", url_host(&host, is_v6), port);
    Endpoint {
        kind: kind.to_string(),
        group: group.to_string(),
        host,
        port,
        url,
        secure,
        recommended: false,
        label: label.to_string(),
    }
}

/// Enumerate every endpoint at which a bound server on `port` is reachable.
pub fn list(port: u16) -> Vec<Endpoint> {
    let mut out: Vec<Endpoint> = Vec::new();

    // Loopback is always first — the one secure-context origin.
    out.push(make_endpoint(
        "loopback",
        GROUP_THIS_DEVICE,
        "127.0.0.1".to_string(),
        port,
        true,
        "This device only (secure context)",
        false,
    ));

    // Interface IPs — skip virtual bridges by name so Docker's 172.x et al.
    // never masquerade as a LAN endpoint.
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        let pairs: Vec<(String, IpAddr)> = ifaces
            .into_iter()
            .map(|i| {
                let ip = i.ip();
                (i.name, ip)
            })
            .collect();
        push_interface_endpoints(&pairs, port, &mut out);
    }

    // Tailnet IPs + MagicDNS from the Tailscale CLI, if present.
    if let Some(status) = tailscale_status() {
        for ip in status.ips {
            if let Ok(IpAddr::V4(v4)) = ip.parse::<IpAddr>() {
                push_unique(
                    &mut out,
                    make_endpoint(
                        "tailnet",
                        GROUP_TAILSCALE,
                        v4.to_string(),
                        port,
                        false,
                        "Over your tailnet",
                        false,
                    ),
                );
            } else if let Ok(IpAddr::V6(v6)) = ip.parse::<IpAddr>() {
                push_unique(
                    &mut out,
                    make_endpoint(
                        "tailnet",
                        GROUP_TAILSCALE,
                        v6.to_string(),
                        port,
                        false,
                        "Over your tailnet",
                        true,
                    ),
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
                        GROUP_TAILSCALE,
                        host,
                        port,
                        false,
                        "MagicDNS name (enable Tailscale's HTTPS serve for a trusted certificate)",
                        false,
                    ),
                );
            }
        }
    }

    mark_recommended(&mut out);
    out
}

/// The tailnet IP addresses this node is reachable at: CGNAT-range
/// (`100.64.0.0/10`) interface IPs plus every address `tailscale status
/// --json` reports for this node. Reuses the exact discovery [`list`] uses,
/// then keeps only `tailnet` entries (dropping loopback/LAN/MagicDNS) and
/// parses their host back to an [`IpAddr`], so the addresses the server
/// binds for the "Tailscale only" scope are precisely the "Tailscale"
/// endpoints the Settings pane advertises. Empty when Tailscale is absent —
/// the caller must NOT silently fall back to all interfaces.
pub fn tailnet_ips() -> Vec<IpAddr> {
    // The port is irrelevant here — we only read each endpoint's host — so
    // pass a placeholder.
    list(0)
        .into_iter()
        .filter(|e| e.kind == "tailnet")
        .filter_map(|e| e.host.parse::<IpAddr>().ok())
        .collect()
}

/// Classify each `(interface-name, ip)` pair into an endpoint, appending to
/// `out`. Loopback, IPv6 link-local, and virtual/bridge interfaces are
/// skipped. Pulled out of `list` so the name-skipping and group-assignment
/// logic is unit-testable without touching the host's real interfaces.
fn push_interface_endpoints(ifaces: &[(String, IpAddr)], port: u16, out: &mut Vec<Endpoint>) {
    for (name, ip) in ifaces {
        if is_virtual_iface(name) {
            continue;
        }
        if ip.is_loopback() {
            continue;
        }
        match ip {
            IpAddr::V4(v4) => {
                if is_tailnet_v4(*v4) {
                    push_unique(
                        out,
                        make_endpoint(
                            "tailnet",
                            GROUP_TAILSCALE,
                            v4.to_string(),
                            port,
                            false,
                            "Over your tailnet",
                            false,
                        ),
                    );
                } else if is_lan_v4(*v4) {
                    // RFC1918 is a genuine local-network endpoint; 169.254
                    // link-local is marginal, so it lands in "other".
                    let group = if v4.is_private() {
                        GROUP_LOCAL_NETWORK
                    } else {
                        GROUP_OTHER
                    };
                    push_unique(
                        out,
                        make_endpoint(
                            "lan",
                            group,
                            v4.to_string(),
                            port,
                            false,
                            "Local network (plain HTTP)",
                            false,
                        ),
                    );
                } else if is_global_v4(*v4) {
                    // A public IPv4 on a real interface — on a hosted box this
                    // is the only address anyone can reach, so surface it
                    // rather than dropping it and leaving only loopback.
                    push_unique(
                        out,
                        make_endpoint(
                            "public",
                            GROUP_PUBLIC,
                            v4.to_string(),
                            port,
                            false,
                            PUBLIC_LABEL,
                            false,
                        ),
                    );
                }
            }
            IpAddr::V6(v6) => {
                // Skip link-local (fe80::/10) — not routable for a peer
                // without a scope id, which we cannot express in a URL.
                if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                    continue;
                }
                // A globally routable v6 is internet-reachable, exactly like a
                // public v4; unique-local (fc00::/7) and anything else has no
                // better class than "other". Plain HTTP either way.
                let (kind, group, label) = if is_global_v6(*v6) {
                    ("public", GROUP_PUBLIC, PUBLIC_LABEL)
                } else {
                    ("lan", GROUP_OTHER, "Other network address (plain HTTP)")
                };
                push_unique(
                    out,
                    make_endpoint(kind, group, v6.to_string(), port, false, label, true),
                );
            }
        }
    }
}

/// Mark the single best "from anywhere" endpoint as recommended: the MagicDNS
/// name if present, else any tailnet IP, else the first local-network address,
/// else a globally routable address. Public comes last because it is the least
/// private option, but it must be considered — a hosted box whose only address
/// is a public IPv4 would otherwise recommend nothing and fall back to a
/// useless `127.0.0.1` pairing URL. Loopback-only (nowhere to reach from)
/// still leaves nothing marked.
fn mark_recommended(out: &mut [Endpoint]) {
    let idx = out
        .iter()
        .position(|e| e.kind == "magicdns")
        .or_else(|| out.iter().position(|e| e.group == GROUP_TAILSCALE))
        .or_else(|| out.iter().position(|e| e.group == GROUP_LOCAL_NETWORK))
        .or_else(|| out.iter().position(|e| e.kind == "public"));
    if let Some(i) = idx {
        out[i].recommended = true;
    }
}

fn push_unique(out: &mut Vec<Endpoint>, ep: Endpoint) {
    if !out.iter().any(|e| e.host == ep.host && e.kind == ep.kind) {
        out.push(ep);
    }
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
    fn global_v4_detection() {
        assert!(is_global_v4("203.0.114.9".parse().unwrap()));
        assert!(is_global_v4("8.8.8.8".parse().unwrap()));
        assert!(is_global_v4("45.79.12.34".parse().unwrap()));
        // Special-use ranges are never "public".
        for special in [
            "192.168.1.5",
            "10.0.0.7",
            "172.16.4.4",
            "127.0.0.1",
            "169.254.5.5",
            "100.100.100.100",
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.7",
            "198.51.100.7",
            "203.0.113.7",
            "198.19.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(
                !is_global_v4(special.parse().unwrap()),
                "{special} should not count as a public address"
            );
        }
    }

    #[test]
    fn global_v6_detection() {
        assert!(is_global_v6("2a01:4f8:1c1c:1::1".parse().unwrap()));
        for special in [
            "::1",
            "::",
            "fe80::1",
            "fd00::5",
            "fd7a:115c:a1e0::1",
            "2001:db8::1",
            "ff02::1",
            "::ffff:192.168.1.5",
        ] {
            assert!(
                !is_global_v6(special.parse().unwrap()),
                "{special} should not count as a public address"
            );
        }
    }

    #[test]
    fn public_v4_is_surfaced_and_grouped() {
        let ifaces = vec![("eth0".to_string(), "203.0.114.9".parse().unwrap())];
        let mut out = Vec::new();
        push_interface_endpoints(&ifaces, 4377, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "public");
        assert_eq!(out[0].group, GROUP_PUBLIC);
        assert_eq!(out[0].url, "http://203.0.114.9:4377");
        assert!(!out[0].secure, "a public address is still plain HTTP");
        assert!(
            out[0].label.contains("internet"),
            "the label must warn that the address is internet-reachable: {}",
            out[0].label
        );
    }

    #[test]
    fn public_v6_is_grouped_with_public_v4_and_bracketed() {
        let ifaces = vec![
            ("eth0".to_string(), "2a01:4f8:1c1c:1::1".parse().unwrap()),
            ("eth0".to_string(), "fd00::5".parse().unwrap()),
        ];
        let mut out = Vec::new();
        push_interface_endpoints(&ifaces, 4377, &mut out);
        let public = out.iter().find(|e| e.kind == "public").unwrap();
        assert_eq!(public.group, GROUP_PUBLIC);
        assert_eq!(public.url, "http://[2a01:4f8:1c1c:1::1]:4377");
        // Unique-local v6 keeps its old "other" classification.
        let ula = out.iter().find(|e| e.host == "fd00::5").unwrap();
        assert_eq!(ula.kind, "lan");
        assert_eq!(ula.group, GROUP_OTHER);
    }

    #[test]
    fn public_addresses_on_virtual_ifaces_are_still_skipped() {
        let ifaces = vec![
            ("docker0".to_string(), "203.0.114.9".parse().unwrap()),
            ("veth1234".to_string(), "8.8.8.8".parse().unwrap()),
            ("tailscale0".to_string(), "2a01:4f8:1c1c:1::1".parse().unwrap()),
            ("eth0".to_string(), "203.0.114.10".parse().unwrap()),
        ];
        let mut out = Vec::new();
        push_interface_endpoints(&ifaces, 4377, &mut out);
        assert_eq!(out.len(), 1, "only the real interface's address survives");
        assert_eq!(out[0].host, "203.0.114.10");
    }

    #[test]
    fn virtual_iface_names_are_detected() {
        for n in [
            "docker0",
            "br-1a2b3c4d",
            "veth9f8e21",
            "virbr0",
            "cni0",
            "flannel.1",
            "cali7c1d2e",
            "kube-ipvs0",
            "vboxnet0",
            "vmnet8",
            "zt5u4d2i8n",
            "tailscale0",
        ] {
            assert!(is_virtual_iface(n), "{n} should be treated as virtual");
        }
        for n in ["eth0", "wlan0", "en0", "enp3s0", "lo", "br0", "bond0"] {
            assert!(!is_virtual_iface(n), "{n} should be treated as real");
        }
    }

    #[test]
    fn docker_and_virtual_ifaces_are_skipped() {
        let ifaces = vec![
            ("docker0".to_string(), "172.17.0.1".parse().unwrap()),
            ("br-1a2b3c4d".to_string(), "172.18.0.1".parse().unwrap()),
            ("veth1234".to_string(), "169.254.10.1".parse().unwrap()),
            ("eth0".to_string(), "192.168.1.50".parse().unwrap()),
        ];
        let mut out = Vec::new();
        push_interface_endpoints(&ifaces, 4377, &mut out);
        // Only the real LAN address survives; the 172.x Docker bridges are gone.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].host, "192.168.1.50");
        assert_eq!(out[0].group, GROUP_LOCAL_NETWORK);
        assert!(
            !out.iter().any(|e| e.host.starts_with("172.")),
            "no Docker 172.x address should be surfaced as a LAN endpoint"
        );
    }

    #[test]
    fn group_assignment_by_address_class() {
        let ifaces = vec![
            ("eth0".to_string(), "192.168.68.58".parse().unwrap()),
            ("eth0".to_string(), "100.119.27.64".parse().unwrap()),
            ("eth0".to_string(), "169.254.5.5".parse().unwrap()),
            ("eth0".to_string(), "fd00::5".parse().unwrap()),
        ];
        let mut out = Vec::new();
        push_interface_endpoints(&ifaces, 4377, &mut out);
        let group_of = |host: &str| {
            out.iter()
                .find(|e| e.host == host)
                .map(|e| e.group.as_str())
        };
        assert_eq!(group_of("192.168.68.58"), Some(GROUP_LOCAL_NETWORK));
        assert_eq!(group_of("100.119.27.64"), Some(GROUP_TAILSCALE));
        // Link-local v4 is surfaced but demoted to "other".
        assert_eq!(group_of("169.254.5.5"), Some(GROUP_OTHER));
        // Unique-local v6 lands in "other" too, with a bracketed URL.
        assert_eq!(group_of("fd00::5"), Some(GROUP_OTHER));
        assert_eq!(
            out.iter().find(|e| e.host == "fd00::5").unwrap().url,
            "http://[fd00::5]:4377"
        );
    }

    #[test]
    fn recommended_prefers_magicdns_and_is_unique() {
        let mut out = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("lan", GROUP_LOCAL_NETWORK, "192.168.1.5".into(), 4377, false, "", false),
            make_endpoint("tailnet", GROUP_TAILSCALE, "100.64.0.1".into(), 4377, false, "", false),
            make_endpoint("magicdns", GROUP_TAILSCALE, "box.ts.net".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut out);
        let recommended: Vec<&Endpoint> = out.iter().filter(|e| e.recommended).collect();
        assert_eq!(recommended.len(), 1, "exactly one endpoint is recommended");
        assert_eq!(recommended[0].kind, "magicdns");
    }

    #[test]
    fn recommended_falls_back_to_tailnet_then_local_then_none() {
        // No MagicDNS → first tailnet IP wins.
        let mut a = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("lan", GROUP_LOCAL_NETWORK, "192.168.1.5".into(), 4377, false, "", false),
            make_endpoint("tailnet", GROUP_TAILSCALE, "100.64.0.1".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut a);
        assert_eq!(a.iter().filter(|e| e.recommended).count(), 1);
        assert_eq!(a.iter().find(|e| e.recommended).unwrap().kind, "tailnet");

        // No Tailscale at all → first local-network address wins.
        let mut b = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("lan", GROUP_LOCAL_NETWORK, "192.168.1.5".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut b);
        assert_eq!(b.iter().filter(|e| e.recommended).count(), 1);
        assert_eq!(
            b.iter().find(|e| e.recommended).unwrap().group,
            GROUP_LOCAL_NETWORK
        );

        // Loopback only → nothing to recommend.
        let mut c = vec![make_endpoint(
            "loopback",
            GROUP_THIS_DEVICE,
            "127.0.0.1".into(),
            4377,
            true,
            "",
            false,
        )];
        mark_recommended(&mut c);
        assert_eq!(c.iter().filter(|e| e.recommended).count(), 0);
    }

    #[test]
    fn recommended_falls_back_to_public_when_it_is_the_only_way_in() {
        // A hosted box: loopback plus one public IPv4. Without the public
        // fallback this recommended nothing and the pairing URL was 127.0.0.1.
        let mut out = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("public", GROUP_PUBLIC, "203.0.114.9".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut out);
        let rec = out.iter().find(|e| e.recommended).unwrap();
        assert_eq!(rec.kind, "public");
        assert_eq!(rec.host, "203.0.114.9");
        assert_eq!(out.iter().filter(|e| e.recommended).count(), 1);
    }

    #[test]
    fn tailnet_and_local_network_still_outrank_public() {
        // Tailnet beats a public address.
        let mut a = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("public", GROUP_PUBLIC, "203.0.114.9".into(), 4377, false, "", false),
            make_endpoint("tailnet", GROUP_TAILSCALE, "100.64.0.1".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut a);
        assert_eq!(a.iter().find(|e| e.recommended).unwrap().kind, "tailnet");

        // So does a local-network address.
        let mut b = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("public", GROUP_PUBLIC, "203.0.114.9".into(), 4377, false, "", false),
            make_endpoint("lan", GROUP_LOCAL_NETWORK, "192.168.1.5".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut b);
        assert_eq!(b.iter().find(|e| e.recommended).unwrap().host, "192.168.1.5");

        // And a marginal "other" address (link-local, ULA) never wins — it is
        // not a public address, so loopback-plus-other still recommends nothing.
        let mut c = vec![
            make_endpoint("loopback", GROUP_THIS_DEVICE, "127.0.0.1".into(), 4377, true, "", false),
            make_endpoint("lan", GROUP_OTHER, "169.254.5.5".into(), 4377, false, "", false),
        ];
        mark_recommended(&mut c);
        assert_eq!(c.iter().filter(|e| e.recommended).count(), 0);
    }

    #[test]
    fn public_only_host_yields_a_public_pairing_endpoint_end_to_end() {
        // The full `list`-shaped path for a public-IPv4-only host: loopback
        // first (so `.first()` is loopback), the public address recommended —
        // which is what `control_pair_from` turns into the pairing URL.
        let mut out = vec![make_endpoint(
            "loopback",
            GROUP_THIS_DEVICE,
            "127.0.0.1".into(),
            4377,
            true,
            "This device only (secure context)",
            false,
        )];
        push_interface_endpoints(
            &[("eth0".to_string(), "203.0.114.9".parse().unwrap())],
            4377,
            &mut out,
        );
        mark_recommended(&mut out);
        let chosen = out
            .iter()
            .find(|e| e.recommended)
            .or_else(|| out.first())
            .unwrap();
        assert_eq!(chosen.url, "http://203.0.114.9:4377");
    }

    #[test]
    fn list_always_includes_loopback_first_and_secure() {
        let eps = list(4377);
        assert_eq!(eps[0].kind, "loopback");
        assert_eq!(eps[0].group, GROUP_THIS_DEVICE);
        assert!(eps[0].secure, "loopback must be a secure context");
        assert_eq!(eps[0].url, "http://127.0.0.1:4377");
        // Non-loopback endpoints are never marked secure (plain HTTP).
        for ep in eps.iter().filter(|e| e.kind != "loopback") {
            assert!(!ep.secure, "{} should not be secure over HTTP", ep.host);
        }
        // At most one recommendation, whatever the host's real interfaces are.
        assert!(eps.iter().filter(|e| e.recommended).count() <= 1);
    }

    #[test]
    fn ipv6_url_host_is_bracketed() {
        let ep = make_endpoint("lan", GROUP_OTHER, "fd00::1".to_string(), 4377, false, "x", true);
        assert_eq!(ep.url, "http://[fd00::1]:4377");
    }
}
