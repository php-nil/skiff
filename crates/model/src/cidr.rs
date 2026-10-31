//! CIDR 匹配（可信代理解析用，§6.2）。
//!
//! 放在 model：`WebConfig.trusted_proxies`、配置校验、代理解析三处共用同一类型，
//! 且零依赖——配置解析不该为了一个网段判断去依赖 web 或 docker。

use std::net::IpAddr;

/// 极简 CIDR 匹配（可信代理解析用，§6.2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Cidr> {
        let s = s.trim();
        let (ip, prefix) = match s.split_once('/') {
            Some((i, p)) => (i, Some(p.parse::<u8>().ok()?)),
            None => (s, None),
        };
        let addr: IpAddr = ip.parse().ok()?;
        let max = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return None;
        }
        Some(Cidr { addr, prefix })
    }

    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                if self.prefix == 0 {
                    return true;
                }
                let mask = u32::MAX << (32 - self.prefix as u32);
                (u32::from(a) & mask) == (u32::from(*b) & mask)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                if self.prefix == 0 {
                    return true;
                }
                let mask = u128::MAX << (128 - self.prefix as u32);
                (u128::from(a) & mask) == (u128::from(*b) & mask)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ipv4_ranges() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains(&"10.1.2.3".parse().unwrap()));
        assert!(!c.contains(&"11.1.2.3".parse().unwrap()));
        let c = Cidr::parse("127.0.0.1").unwrap();
        assert!(c.contains(&"127.0.0.1".parse().unwrap()));
        assert!(!c.contains(&"127.0.0.2".parse().unwrap()));
    }

    #[test]
    fn rejects_bad_cidr() {
        assert!(Cidr::parse("10.0.0.0/33").is_none());
        assert!(Cidr::parse("not-an-ip").is_none());
        let c = Cidr::parse("0.0.0.0/0").unwrap();
        assert!(c.contains(&"8.8.8.8".parse().unwrap()));
    }
}
