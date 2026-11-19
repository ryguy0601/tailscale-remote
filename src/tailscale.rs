use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::process::Command;

/// Summary metadata for a Tailscale peer node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSummary {
    pub hostname: String,
    pub os: String,
}

/// Detected Tailscale identity and known tailnet peers for this host.
#[derive(Clone, Debug)]
pub struct TailscaleInfo {
    pub ipv4: String,
    pub ipv6: Option<String>,
    pub hostname: String,
    pub dns_name: Option<String>,
    pub peers: HashMap<String, PeerSummary>,
}

impl TailscaleInfo {
    /// Returns a human-readable peer label like `"iphone (iOS)"` or falls back to the IP string.
    pub fn lookup_peer_label(&self, ip: IpAddr) -> String {
        let normalized = normalize_ip(ip);
        let key = normalized.to_string();
        if let Some(peer) = self.peers.get(&key) {
            if peer.os.is_empty() {
                return peer.hostname.clone();
            }
            return format!("{} ({})", peer.hostname, peer.os);
        }
        if normalized.is_loopback() {
            return format!("Localhost ({key})");
        }
        key
    }
}

fn new_tailscale_cmd(args: &[&str]) -> Command {
    let mut cmd = Command::new("tailscale");
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Queries the local Tailscale daemon (`tailscale status --json`, with `tailscale ip -4` fallback)
/// to determine the host's Tailscale IPv4/IPv6 addresses, MagicDNS name, and peer directory.
pub fn detect_tailscale() -> Result<TailscaleInfo, String> {
    if let Ok(output) = new_tailscale_cmd(&["status", "--json"]).output() {
        if output.status.success() {
            if let Ok(val) = serde_json::from_slice::<Value>(&output.stdout) {
                if let Some(info) = parse_status_json(&val) {
                    return Ok(info);
                }
                if let Some(state) = val.get("BackendState").and_then(|v| v.as_str()) {
                    if state != "Running" {
                        return Err(format!(
                            "Tailscale backend state is '{state}'. Start or authenticate it with `tailscale up`."
                        ));
                    }
                }
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if !stderr.is_empty() {
                // Still try `tailscale ip -4` below before returning the error.
                if let Ok(info) = fallback_tailscale_ip() {
                    return Ok(info);
                }
                return Err(format!(
                    "Tailscale CLI returned an error ({stderr}). Ensure Tailscale is installed and running (`tailscale up`)."
                ));
            }
        }
    }

    fallback_tailscale_ip().map_err(|err| {
        format!(
            "Could not detect an active Tailscale connection ({err}). Ensure `tailscale` is installed, running, and logged in (`tailscale up`), or run with `--allow-lan --bind 0.0.0.0:9880` for local testing."
        )
    })
}

fn parse_status_json(val: &Value) -> Option<TailscaleInfo> {
    let self_obj = val.get("Self")?.as_object()?;
    let mut ipv4: Option<String> = None;
    let mut ipv6: Option<String> = None;

    if let Some(ips) = self_obj.get("TailscaleIPs").and_then(|v| v.as_array()) {
        for raw in ips.iter().filter_map(|v| v.as_str()) {
            if let Ok(parsed) = raw.parse::<IpAddr>() {
                match parsed {
                    IpAddr::V4(_) if ipv4.is_none() => ipv4 = Some(raw.to_string()),
                    IpAddr::V6(_) if ipv6.is_none() => ipv6 = Some(raw.to_string()),
                    _ => {}
                }
            }
        }
    }

    let ipv4 = ipv4?;
    let hostname = self_obj
        .get("HostName")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("media-pc")
        .to_string();

    let dns_name = self_obj
        .get("DNSName")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty());

    let mut peers = HashMap::new();
    if let Some(peer_map) = val.get("Peer").and_then(|v| v.as_object()) {
        for (_, peer) in peer_map {
            let p_host = peer
                .get("HostName")
                .and_then(|v| v.as_str())
                .unwrap_or("Tailnet Peer")
                .trim()
                .to_string();
            let p_os = peer
                .get("OS")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let summary = PeerSummary {
                hostname: p_host,
                os: p_os,
            };
            if let Some(ips) = peer.get("TailscaleIPs").and_then(|v| v.as_array()) {
                for ip_str in ips.iter().filter_map(|v| v.as_str()) {
                    peers.insert(ip_str.to_string(), summary.clone());
                }
            }
        }
    }

    Some(TailscaleInfo {
        ipv4,
        ipv6,
        hostname,
        dns_name,
        peers,
    })
}

fn fallback_tailscale_ip() -> Result<TailscaleInfo, String> {
    let out = new_tailscale_cmd(&["ip", "-4"])
        .output()
        .map_err(|e| format!("failed to run `tailscale ip -4`: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "tailscale ip -4 exited non-zero".to_string()
        } else {
            stderr
        });
    }
    let ipv4 = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.parse::<std::net::Ipv4Addr>().is_ok())
        .ok_or_else(|| "no IPv4 address returned by `tailscale ip -4`".to_string())?
        .to_string();

    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "media-pc".to_string());

    Ok(TailscaleInfo {
        ipv4,
        ipv6: None,
        hostname,
        dns_name: None,
        peers: HashMap::new(),
    })
}

/// Unwraps IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) into `IpAddr::V4`.
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

/// Returns `true` if `ip` belongs to Tailscale's CGNAT IPv4 range (`100.64.0.0/10`)
/// or Tailscale's IPv6 ULA prefix (`fd7a:115c:a1e0::/48`).
pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            seg[0] == 0xfd7a && seg[1] == 0x115c && seg[2] == 0xa1e0
        }
    }
}

/// Returns `true` if `ip` is either loopback (`127.0.0.0/8`, `::1`) or a Tailscale address.
pub fn is_tailscale_or_loopback(ip: IpAddr) -> bool {
    let norm = normalize_ip(ip);
    norm.is_loopback() || is_tailscale_ip(norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_tailscale_ip_ranges() {
        let allowed = [
            "100.64.0.1",
            "100.64.0.10",
            "100.127.255.254",
            "127.0.0.1",
            "::1",
            "fd7a:115c:a1e0::1",
            "fd7a:115c:a1e0::1234:5678",
            "::ffff:100.64.0.10",
            "::ffff:127.0.0.1",
        ];
        for addr in allowed {
            let ip = IpAddr::from_str(addr).unwrap();
            assert!(
                is_tailscale_or_loopback(ip),
                "expected {addr} to be allowed as Tailscale or loopback"
            );
        }

        let blocked = [
            "100.63.255.255",
            "100.128.0.0",
            "192.168.1.10",
            "10.0.0.5",
            "172.16.0.2",
            "8.8.8.8",
            "fd00::1",
            "fd7a:115c:a1e1::1",
        ];
        for addr in blocked {
            let ip = IpAddr::from_str(addr).unwrap();
            assert!(
                !is_tailscale_or_loopback(ip),
                "expected {addr} to be blocked outside Tailscale/loopback"
            );
        }
    }

    #[test]
    fn test_parse_status_json_and_peer_lookup() {
        let sample = serde_json::json!({
            "Self": {
                "HostName": "media-pc",
                "DNSName": "media-pc.tailnet-demo.ts.net.",
                "TailscaleIPs": ["100.64.0.10", "fd7a:115c:a1e0::1"]
            },
            "Peer": {
                "node1": {
                    "HostName": "demo-phone",
                    "OS": "iOS",
                    "TailscaleIPs": ["100.80.10.5"]
                }
            }
        });
        let info = parse_status_json(&sample).expect("valid sample status JSON");
        assert_eq!(info.ipv4, "100.64.0.10");
        assert_eq!(info.ipv6.as_deref(), Some("fd7a:115c:a1e0::1"));
        assert_eq!(info.hostname, "media-pc");
        assert_eq!(
            info.dns_name.as_deref(),
            Some("media-pc.tailnet-demo.ts.net")
        );
        assert_eq!(
            info.lookup_peer_label("100.80.10.5".parse().unwrap()),
            "demo-phone (iOS)"
        );
        assert_eq!(
            info.lookup_peer_label("100.80.10.99".parse().unwrap()),
            "100.80.10.99"
        );
    }
}
