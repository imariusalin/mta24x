use crate::warming::{Health, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Transactional,
    Marketing,
    Mailbox,
}

impl Stream {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "transactional" | "tx" => Some(Self::Transactional),
            "marketing" | "bulk" | "news" => Some(Self::Marketing),
            "mailbox" | "people" | "human" => Some(Self::Mailbox),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transactional => "transactional",
            Self::Marketing => "marketing",
            Self::Mailbox => "mailbox",
        }
    }

    pub fn preferred_role(self) -> Role {
        match self {
            Self::Transactional | Self::Mailbox => Role::Transactional,
            Self::Marketing => Role::Marketing,
        }
    }

    pub fn priority(self) -> i32 {
        match self {
            Self::Transactional => 10,
            Self::Mailbox => 20,
            Self::Marketing => 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendingIp {
    pub id: String,
    pub address: String,
    pub hostname: String,
    pub role: Role,
    pub health: Health,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    NoHealthyIp,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHealthyIp => write!(
                f,
                "no healthy IP available for this stream (pool quarantined and canary down)"
            ),
        }
    }
}

/// Candidates for a stream, preferred pool first.
/// Marketing never uses a transactional IP when any marketing/canary IP exists.
/// A single-IP host (only transactional) shares that IP with every stream.
pub fn candidates(stream: Stream, ips: &[SendingIp]) -> Vec<&SendingIp> {
    let role = stream.preferred_role();
    let mut out = by_role(ips, role);
    if out.is_empty() && role != Role::Canary {
        out = by_role(ips, Role::Canary);
    }
    if out.is_empty() && role == Role::Marketing {
        let isolated = ips
            .iter()
            .any(|i| i.role == Role::Marketing || i.role == Role::Canary);
        if !isolated {
            out = by_role(ips, Role::Transactional);
        }
    }
    if out.is_empty() && role == Role::Transactional {
        out = ips
            .iter()
            .filter(|ip| ip.health != Health::Quarantine)
            .collect();
    }
    out.sort_by_key(|ip| match ip.health {
        Health::Active => 0,
        Health::Warmup => 1,
        Health::Quarantine => 2,
    });
    out
}

pub fn pick_ip(stream: Stream, ips: &[SendingIp]) -> Result<&SendingIp, RouteError> {
    candidates(stream, ips)
        .into_iter()
        .next()
        .ok_or(RouteError::NoHealthyIp)
}

fn by_role(ips: &[SendingIp], role: Role) -> Vec<&SendingIp> {
    ips.iter()
        .filter(|ip| ip.role == role && ip.health != Health::Quarantine)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(id: &str, role: Role, health: Health) -> SendingIp {
        SendingIp {
            id: id.into(),
            address: format!("203.0.113.{id}"),
            hostname: format!("{id}.example.com"),
            role,
            health,
        }
    }

    #[test]
    fn transactional_uses_tx_ip() {
        let ips = vec![
            ip("1", Role::Transactional, Health::Warmup),
            ip("2", Role::Marketing, Health::Warmup),
            ip("3", Role::Canary, Health::Warmup),
        ];
        let chosen = pick_ip(Stream::Transactional, &ips).unwrap();
        assert_eq!(chosen.id, "1");
        let chosen = pick_ip(Stream::Mailbox, &ips).unwrap();
        assert_eq!(chosen.id, "1");
    }

    #[test]
    fn marketing_never_uses_transactional_ip() {
        let ips = vec![
            ip("1", Role::Transactional, Health::Active),
            ip("2", Role::Marketing, Health::Warmup),
            ip("3", Role::Canary, Health::Active),
        ];
        let chosen = pick_ip(Stream::Marketing, &ips).unwrap();
        assert_eq!(chosen.id, "2");
    }

    #[test]
    fn prefers_active_over_warmup_same_role() {
        let ips = vec![
            ip("cold", Role::Transactional, Health::Warmup),
            ip("hot", Role::Transactional, Health::Active),
        ];
        assert_eq!(pick_ip(Stream::Transactional, &ips).unwrap().id, "hot");
    }

    #[test]
    fn quarantined_tx_falls_back_to_canary() {
        let ips = vec![
            ip("1", Role::Transactional, Health::Quarantine),
            ip("2", Role::Marketing, Health::Active),
            ip("3", Role::Canary, Health::Warmup),
        ];
        let chosen = pick_ip(Stream::Transactional, &ips).unwrap();
        assert_eq!(chosen.id, "3");
    }

    #[test]
    fn single_ip_is_shared() {
        let ips = vec![ip("only", Role::Transactional, Health::Warmup)];
        assert_eq!(pick_ip(Stream::Transactional, &ips).unwrap().id, "only");
        assert_eq!(pick_ip(Stream::Marketing, &ips).unwrap().id, "only");
        assert_eq!(pick_ip(Stream::Mailbox, &ips).unwrap().id, "only");
    }

    #[test]
    fn all_quarantined_rejects() {
        let ips = vec![
            ip("1", Role::Transactional, Health::Quarantine),
            ip("2", Role::Marketing, Health::Quarantine),
            ip("3", Role::Canary, Health::Quarantine),
        ];
        assert_eq!(
            pick_ip(Stream::Marketing, &ips),
            Err(RouteError::NoHealthyIp)
        );
    }
}
