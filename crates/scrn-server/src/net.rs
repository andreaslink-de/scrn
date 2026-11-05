//! IP address ranges for the list of trusted reverse proxies.
//!
//! Only the configuration lives here for now. Extracting the client IP from
//! `X-Forwarded-For` follows together with rate limiting.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

/// An IP network in CIDR notation, e.g. `172.18.0.0/16` or `::1/128`.
/// A plain address is treated as a single-host network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

/// Error for a string that is neither an IP address nor a CIDR range.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid IP address or CIDR range")]
pub struct ParseIpNetError;

impl IpNet {
    /// Returns true if `ip` lies inside this network. IPv4-mapped IPv6
    /// addresses (`::ffff:a.b.c.d`) are compared as IPv4.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, normalize(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = prefix_mask_u32(self.prefix);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = prefix_mask_u128(self.prefix);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for IpNet {
    type Err = ParseIpNetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr_part, prefix_part) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr = normalize(addr_part.parse::<IpAddr>().map_err(|_| ParseIpNetError)?);
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_part {
            None => max,
            Some(p) => {
                // Only plain decimal digits, so "+8" or " 8" are rejected.
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(ParseIpNetError);
                }
                let prefix: u8 = p.parse().map_err(|_| ParseIpNetError)?;
                if prefix > max {
                    return Err(ParseIpNetError);
                }
                prefix
            }
        };
        Ok(Self { addr, prefix })
    }
}

impl fmt::Display for IpNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// The set of reverse proxies whose forwarding headers are trusted.
/// An empty set trusts nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    pub fn new(nets: Vec<IpNet>) -> Self {
        Self { nets }
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(ip))
    }

    pub fn nets(&self) -> &[IpNet] {
        &self.nets
    }
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

fn prefix_mask_u32(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

fn prefix_mask_u128(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn single_address_matches_only_itself() {
        let net: IpNet = "10.0.0.5".parse().unwrap();
        assert!(net.contains(ip("10.0.0.5")));
        assert!(!net.contains(ip("10.0.0.4")));
        assert_eq!(net.to_string(), "10.0.0.5/32");
    }

    #[test]
    fn ipv4_ranges() {
        let net: IpNet = "172.18.0.0/16".parse().unwrap();
        assert!(net.contains(ip("172.18.255.1")));
        assert!(!net.contains(ip("172.19.0.1")));
        let all: IpNet = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("8.8.8.8")));
        assert!(!all.contains(ip("2001:db8::1")));
    }

    #[test]
    fn ipv6_ranges() {
        let net: IpNet = "2001:db8::/32".parse().unwrap();
        assert!(net.contains(ip("2001:db8:1::1")));
        assert!(!net.contains(ip("2001:db9::1")));
        assert!(!net.contains(ip("10.0.0.1")));
    }

    #[test]
    fn ipv4_mapped_addresses_match_ipv4_ranges() {
        let net: IpNet = "127.0.0.0/8".parse().unwrap();
        assert!(net.contains(ip("::ffff:127.0.0.1")));
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "abc",
            "10.0.0.0/",
            "10.0.0.0/33",
            "::/129",
            "10.0.0.0/+8",
            "10.0.0.0/8/8",
        ] {
            assert_eq!(s.parse::<IpNet>(), Err(ParseIpNetError), "{s:?}");
        }
    }

    #[test]
    fn empty_set_trusts_nobody() {
        let proxies = TrustedProxies::default();
        assert!(proxies.is_empty());
        assert!(!proxies.contains(ip("127.0.0.1")));
    }
}
