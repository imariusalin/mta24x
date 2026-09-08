use crate::warming::Role;
use anyhow::{anyhow, Context, Result};
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedIp {
    pub address: IpAddr,
    pub hostname: String,
    pub role: Role,
}

/// How many IPs go to tx / marketing / canary for a host with `n` addresses.
pub fn split_counts(n: usize) -> (usize, usize, usize) {
    match n {
        0 => (0, 0, 0),
        1 => (1, 0, 0),
        2 => (1, 1, 0),
        3 => (1, 1, 1),
        n => {
            let n_tx = (n * 20 / 100).max(1);
            let n_canary = (n * 10 / 100).max(1);
            let n_mkt = n.saturating_sub(n_tx + n_canary).max(1);
            // If rounding overflowed, steal from canary then tx.
            let used = n_tx + n_mkt + n_canary;
            if used > n {
                let extra = used - n;
                let canary = n_canary.saturating_sub(extra);
                let leftover = extra.saturating_sub(n_canary);
                let tx = n_tx.saturating_sub(leftover);
                (tx.max(1), n - tx.max(1) - canary, canary)
            } else if used < n {
                (n_tx, n_mkt + (n - used), n_canary)
            } else {
                (n_tx, n_mkt, n_canary)
            }
        }
    }
}

pub fn ehlo_hostname(role: Role, index: usize, root: &str, mail_hostname: &str) -> String {
    match (role, index) {
        (Role::Transactional, 0) => mail_hostname.to_string(),
        (Role::Transactional, i) => format!("tx-{}.{root}", i + 1),
        (Role::Marketing, 0) => format!("news-out.{root}"),
        (Role::Marketing, i) => format!("mkt-{}.{root}", i + 1),
        (Role::Canary, 0) => format!("out.{root}"),
        (Role::Canary, i) => format!("out-{}.{root}", i + 1),
    }
}

pub fn parse_csv_ips(raw: &str) -> Result<Vec<IpAddr>> {
    let mut out = Vec::new();
    for part in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(part.parse::<IpAddr>().with_context(|| format!("bad IP {part}"))?);
    }
    Ok(out)
}

pub fn assign_roles(addresses: &[IpAddr], root: &str, mail_hostname: &str) -> Vec<AssignedIp> {
    let (n_tx, n_mkt, _n_canary) = split_counts(addresses.len());
    let mut out = Vec::with_capacity(addresses.len());
    let mut tx_i = 0usize;
    let mut mkt_i = 0usize;
    let mut can_i = 0usize;
    for (i, addr) in addresses.iter().enumerate() {
        let role = if i < n_tx {
            Role::Transactional
        } else if i < n_tx + n_mkt {
            Role::Marketing
        } else {
            Role::Canary
        };
        let index = match role {
            Role::Transactional => {
                let n = tx_i;
                tx_i += 1;
                n
            }
            Role::Marketing => {
                let n = mkt_i;
                mkt_i += 1;
                n
            }
            Role::Canary => {
                let n = can_i;
                can_i += 1;
                n
            }
        };
        out.push(AssignedIp {
            address: *addr,
            hostname: ehlo_hostname(role, index, root, mail_hostname),
            role,
        });
    }
    out
}

pub fn assign_explicit(
    tx: &[IpAddr],
    mkt: &[IpAddr],
    canary: &[IpAddr],
    root: &str,
    mail_hostname: &str,
) -> Result<Vec<AssignedIp>> {
    if tx.is_empty() && mkt.is_empty() && canary.is_empty() {
        return Err(anyhow!("no IPs in POOL_TX / POOL_MKT / POOL_CANARY"));
    }
    let mut out = Vec::new();
    for (i, addr) in tx.iter().enumerate() {
        out.push(AssignedIp {
            address: *addr,
            hostname: ehlo_hostname(Role::Transactional, i, root, mail_hostname),
            role: Role::Transactional,
        });
    }
    for (i, addr) in mkt.iter().enumerate() {
        out.push(AssignedIp {
            address: *addr,
            hostname: ehlo_hostname(Role::Marketing, i, root, mail_hostname),
            role: Role::Marketing,
        });
    }
    for (i, addr) in canary.iter().enumerate() {
        out.push(AssignedIp {
            address: *addr,
            hostname: ehlo_hostname(Role::Canary, i, root, mail_hostname),
            role: Role::Canary,
        });
    }
    Ok(out)
}

pub fn ip_id(addr: &IpAddr) -> String {
    format!("ip-{}", addr.to_string().replace(['.', ':'], "-"))
}

pub fn role_str(role: Role) -> &'static str {
    match role {
        Role::Transactional => "transactional",
        Role::Marketing => "marketing",
        Role::Canary => "canary",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn split_one_two_three() {
        assert_eq!(split_counts(1), (1, 0, 0));
        assert_eq!(split_counts(2), (1, 1, 0));
        assert_eq!(split_counts(3), (1, 1, 1));
    }

    #[test]
    fn split_four_and_ten() {
        assert_eq!(split_counts(4), (1, 2, 1));
        let (tx, mkt, can) = split_counts(10);
        assert_eq!(tx + mkt + can, 10);
        assert!(tx >= 1 && mkt >= 1 && can >= 1);
    }

    #[test]
    fn split_two_hundred_adds_up() {
        let (tx, mkt, can) = split_counts(200);
        assert_eq!(tx + mkt + can, 200);
        assert_eq!(tx, 40);
        assert_eq!(can, 20);
        assert_eq!(mkt, 140);
    }

    #[test]
    fn assign_three_classic() {
        let addrs = [v4(203, 0, 113, 10), v4(203, 0, 113, 11), v4(203, 0, 113, 12)];
        let assigned = assign_roles(&addrs, "example.com", "mail.example.com");
        assert_eq!(assigned[0].role, Role::Transactional);
        assert_eq!(assigned[0].hostname, "mail.example.com");
        assert_eq!(assigned[1].role, Role::Marketing);
        assert_eq!(assigned[1].hostname, "news-out.example.com");
        assert_eq!(assigned[2].role, Role::Canary);
        assert_eq!(assigned[2].hostname, "out.example.com");
    }

    #[test]
    fn assign_one_is_transactional() {
        let addrs = [v4(203, 0, 113, 10)];
        let assigned = assign_roles(&addrs, "example.com", "mail.example.com");
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].role, Role::Transactional);
    }

    #[test]
    fn parse_csv() {
        let ips = parse_csv_ips("203.0.113.10, 203.0.113.11").unwrap();
        assert_eq!(ips.len(), 2);
    }
}
