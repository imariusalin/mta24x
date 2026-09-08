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

/// Pick an IP for a stream. Marketing never rides a transactional IP.
/// If the dedicated pool is quarantined, fall back to canary only.
pub fn pick_ip(stream: Stream, ips: &[SendingIp]) -> Result<&SendingIp, RouteError> {
    let role = stream.preferred_role();
    if let Some(ip) = best(ips, role) {
        return Ok(ip);
    }
    if role != Role::Canary {
        if let Some(ip) = best(ips, Role::Canary) {
            return Ok(ip);
        }
    }
    Err(RouteError::NoHealthyIp)
}

fn best(ips: &[SendingIp], role: Role) -> Option<&SendingIp> {
    ips.iter()
        .filter(|ip| ip.role == role && ip.health != Health::Quarantine)
        .min_by_key(|ip| match ip.health {
            Health::Active => 0,
            Health::Warmup => 1,
            Health::Quarantine => 2,
        })
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
