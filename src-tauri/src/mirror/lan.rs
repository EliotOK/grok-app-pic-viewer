//! LAN bind helpers for the phone-mirror host.
//!
//! Default bind stays loopback (`127.0.0.1`). Opt-in LAN (`0.0.0.0`) is token-gated
//! still, but HTTP is unencrypted — the Connect panel must confirm before enabling.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// Address ranges owned by virtual / overlay adapters (VPN, container, Clash-style
/// TUN). They are routable *on the host* but a phone on the same Wi-Fi cannot reach
/// them, so they must never be advertised as the mirror's LAN URL.
fn is_virtual_adapter_range(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    // 198.18.0.0/15 — benchmarking range; Clash / Mihomo fake-ip pools live here.
    (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        // 100.64.0.0/10 — CGNAT; Tailscale and other overlays assign from here.
        || (o[0] == 100 && (64..=127).contains(&o[1]))
}

/// IPv4s that are usable as a phone-facing LAN URL host.
pub fn is_usable_lan_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !is_virtual_adapter_range(ip)
}

/// Lower is better. Prefer typical home Wi-Fi ranges.
#[cfg(any(test, windows))]
pub fn rank_lan_ipv4(ip: Ipv4Addr) -> u8 {
    let o = ip.octets();
    if o[0] == 192 && o[1] == 168 {
        0
    } else if o[0] == 10 {
        1
    } else if o[0] == 172 && (16..=31).contains(&o[1]) {
        2
    } else {
        3
    }
}

#[cfg(test)]
pub fn pick_lan_ipv4(ips: impl IntoIterator<Item = Ipv4Addr>) -> Option<Ipv4Addr> {
    ips.into_iter()
        .filter(|ip| is_usable_lan_ipv4(*ip))
        .min_by_key(|ip| rank_lan_ipv4(*ip))
}

/// Bind address for the HTTP listener.
pub fn listen_ip(allow_lan: bool) -> Ipv4Addr {
    if allow_lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    }
}

/// Host shown in copy/QR URLs. Never advertise `0.0.0.0`.
pub fn display_host(allow_lan: bool, lan_ip: Option<Ipv4Addr>) -> String {
    if allow_lan {
        if let Some(ip) = lan_ip.filter(|ip| is_usable_lan_ipv4(*ip)) {
            return ip.to_string();
        }
    }
    "127.0.0.1".into()
}

pub fn mirror_path_url(host: &str, port: u16, token: &str) -> String {
    format!("http://{host}:{port}/t/{token}/")
}

pub fn local_access_url(
    allow_lan: bool,
    port: u16,
    token: &str,
    lan_ip: Option<Ipv4Addr>,
) -> String {
    mirror_path_url(&display_host(allow_lan, lan_ip), port, token)
}

/// Source address the OS picks for an internet destination (UDP connect trick;
/// no packets are sent). With a TUN device present this can return the tunnel
/// address, which `is_usable_lan_ipv4` then rejects.
fn default_route_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    sock.connect(SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 80)))
        .ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// Adapter aliases that never face the phone (VPN / VM / container switches).
/// Matching is a guard on top of the range filter, not the only filter.
#[cfg(windows)]
fn alias_is_virtual(alias: &str) -> bool {
    const NEEDLES: &str = "clash mihomo tailscale zerotier wireguard openvpn vethernet \
                           hyper-v wsl docker vmware virtualbox loopback bluetooth npcap";
    NEEDLES.split_whitespace().any(|needle| alias.contains(needle))
}

/// Enumerate adapters and pick the best phone-reachable IPv4.
///
/// Windows: `Get-NetIPAddress` — locale-independent fields, virtual adapters
/// skipped by alias and by range. Other platforms keep the default-route probe
/// only (a TUN address there leaves the URL at loopback, same as before).
#[cfg(windows)]
fn enumerate_lan_ipv4() -> Option<Ipv4Addr> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // Spawn without a console window; the app is a GUI process.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // One `address|interfaceAlias` line per adapter.
    const SCRIPT: &str = "Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { $_.IPAddress + '|' + $_.InterfaceAlias }";

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut best: Option<(u8, Ipv4Addr)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '|');
        let ip_text = parts.next().unwrap_or("").trim();
        let alias = parts.next().unwrap_or("").to_ascii_lowercase();
        let Ok(ip) = ip_text.parse::<Ipv4Addr>() else {
            continue;
        };
        if !is_usable_lan_ipv4(ip) || alias_is_virtual(&alias) {
            continue;
        }
        let rank = rank_lan_ipv4(ip);
        if best.is_none_or(|(best_rank, _)| rank < best_rank) {
            best = Some((rank, ip));
        }
    }
    best.map(|(_, ip)| ip)
}

#[cfg(not(windows))]
fn enumerate_lan_ipv4() -> Option<Ipv4Addr> {
    None
}

/// LAN IPv4 to advertise: the default-route source when it looks reachable from
/// the LAN, otherwise a real adapter from enumeration.
pub fn detect_lan_ipv4() -> Option<Ipv4Addr> {
    if let Some(ip) = default_route_ipv4().filter(|ip| is_usable_lan_ipv4(*ip)) {
        return Some(ip);
    }
    enumerate_lan_ipv4()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_filters_loopback_and_link_local() {
        assert!(!is_usable_lan_ipv4(Ipv4Addr::LOCALHOST));
        assert!(!is_usable_lan_ipv4(Ipv4Addr::UNSPECIFIED));
        assert!(!is_usable_lan_ipv4(Ipv4Addr::new(169, 254, 1, 1)));
        assert!(is_usable_lan_ipv4(Ipv4Addr::new(192, 168, 1, 8)));
        assert!(is_usable_lan_ipv4(Ipv4Addr::new(10, 0, 0, 4)));
        assert!(is_usable_lan_ipv4(Ipv4Addr::new(172, 16, 0, 2)));
    }

    #[test]
    fn usable_rejects_virtual_adapter_ranges() {
        // Clash / Mihomo TUN fake-ip pool.
        assert!(!is_usable_lan_ipv4(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(!is_usable_lan_ipv4(Ipv4Addr::new(198, 19, 3, 7)));
        // Tailscale / CGNAT.
        assert!(!is_usable_lan_ipv4(Ipv4Addr::new(100, 64, 0, 5)));
        assert!(!is_usable_lan_ipv4(Ipv4Addr::new(100, 127, 255, 254)));
        // Neighbours stay usable.
        assert!(is_usable_lan_ipv4(Ipv4Addr::new(198, 20, 0, 1)));
        assert!(is_usable_lan_ipv4(Ipv4Addr::new(100, 63, 0, 1)));
    }

    #[test]
    fn pick_prefers_rfc1918_home_wifi() {
        let picked = pick_lan_ipv4([
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(192, 168, 110, 188),
        ]);
        assert_eq!(picked, Some(Ipv4Addr::new(192, 168, 110, 188)));
    }

    #[test]
    fn pick_skips_tun_when_real_adapter_exists() {
        // The reported bug: the TUN address won the default-route probe and the
        // QR code pointed the phone at 198.18.0.1.
        let picked = pick_lan_ipv4([
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(192, 168, 1, 220),
        ]);
        assert_eq!(picked, Some(Ipv4Addr::new(192, 168, 1, 220)));
    }

    #[test]
    fn listen_ip_loopback_by_default() {
        assert_eq!(listen_ip(false), Ipv4Addr::LOCALHOST);
        assert_eq!(listen_ip(true), Ipv4Addr::UNSPECIFIED);
    }

    #[test]
    fn display_host_never_advertises_unspecified() {
        assert_eq!(
            display_host(false, Some(Ipv4Addr::new(192, 168, 1, 2))),
            "127.0.0.1"
        );
        assert_eq!(
            display_host(true, Some(Ipv4Addr::new(192, 168, 1, 2))),
            "192.168.1.2"
        );
        assert_eq!(display_host(true, None), "127.0.0.1");
        assert_eq!(display_host(true, Some(Ipv4Addr::LOCALHOST)), "127.0.0.1");
        // A TUN address is never advertised even when detection slipped through.
        assert_eq!(display_host(true, Some(Ipv4Addr::new(198, 18, 0, 1))), "127.0.0.1");
    }

    #[test]
    fn local_url_embeds_token_path() {
        let url = local_access_url(true, 59166, "tok", Some(Ipv4Addr::new(192, 168, 110, 188)));
        assert_eq!(url, "http://192.168.110.188:59166/t/tok/");
        let loopback =
            local_access_url(false, 59166, "tok", Some(Ipv4Addr::new(192, 168, 110, 188)));
        assert_eq!(loopback, "http://127.0.0.1:59166/t/tok/");
    }
}