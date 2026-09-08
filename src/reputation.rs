use crate::warming::Health;
use serde::{Deserialize, Serialize};

/// Automatic volume action from live signals (not just calendar age).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeDecision {
    /// Send nothing. IP is listed, blocked, or bleeding.
    Stop,
    /// 25% of the warmup curve. Hard trouble, not yet quarantine.
    Cut,
    /// 50% of the curve. ISP is pushing back (greylist / defer).
    Slow,
    /// Follow the curve as written.
    Hold,
    /// 120% of the curve. Clean active IP can ramp a bit faster.
    Push,
}

impl VolumeDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Cut => "cut",
            Self::Slow => "slow",
            Self::Hold => "hold",
            Self::Push => "push",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "stop" => Self::Stop,
            "cut" => Self::Cut,
            "slow" => Self::Slow,
            "push" => Self::Push,
            _ => Self::Hold,
        }
    }

    pub fn multiplier(self) -> f64 {
        match self {
            Self::Stop => 0.0,
            Self::Cut => 0.25,
            Self::Slow => 0.50,
            Self::Hold => 1.0,
            Self::Push => 1.20,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stop => "stop sending",
            Self::Cut => "cut to 25%",
            Self::Slow => "slow to 50%",
            Self::Hold => "hold curve",
            Self::Push => "increase 20%",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Signals {
    pub health: Health,
    pub sent_24h: u32,
    pub sent_7d: u32,
    pub bounce_24h: u32,
    pub bounce_7d: u32,
    pub complaint_7d: u32,
    pub deferred_24h: u32,
    pub blocked_48h: bool,
    pub dnsbl_listed: bool,
}

impl Signals {
    pub fn bounce_rate_7d(&self) -> f64 {
        rate(self.bounce_7d, self.sent_7d)
    }
    pub fn bounce_rate_24h(&self) -> f64 {
        rate(self.bounce_24h, self.sent_24h)
    }
    pub fn complaint_rate_7d(&self) -> f64 {
        rate(self.complaint_7d, self.sent_7d)
    }
    pub fn defer_rate_24h(&self) -> f64 {
        rate(self.deferred_24h, self.sent_24h)
    }
}

fn rate(part: u32, whole: u32) -> f64 {
    if whole == 0 {
        0.0
    } else {
        f64::from(part) / f64::from(whole)
    }
}

/// Decide stop / cut / slow / hold / push from live delivery, not calendar age.
pub fn decide_volume(s: &Signals) -> VolumeDecision {
    if s.health == Health::Quarantine || s.blocked_48h || s.dnsbl_listed {
        return VolumeDecision::Stop;
    }
    if s.sent_7d >= 100 && s.complaint_rate_7d() > 0.001 {
        return VolumeDecision::Stop;
    }
    if s.sent_7d >= 100 && s.bounce_rate_7d() > 0.05 {
        return VolumeDecision::Stop;
    }
    if s.sent_24h >= 30 && s.bounce_rate_24h() > 0.03 {
        return VolumeDecision::Cut;
    }
    if s.sent_7d >= 50 && s.bounce_rate_7d() > 0.02 {
        return VolumeDecision::Slow;
    }
    if s.sent_24h >= 40 && s.defer_rate_24h() > 0.20 {
        return VolumeDecision::Slow;
    }
    if s.health == Health::Active
        && s.sent_7d >= 500
        && s.bounce_rate_7d() < 0.01
        && s.complaint_7d == 0
        && s.defer_rate_24h() < 0.05
        && !s.dnsbl_listed
    {
        return VolumeDecision::Push;
    }
    VolumeDecision::Hold
}

/// 0–100 score for the console. 100 is a clean unknown; blocks and lists crush it.
pub fn reputation_score(s: &Signals) -> u8 {
    let mut score = 100.0;
    if s.dnsbl_listed {
        score -= 40.0;
    }
    if s.blocked_48h {
        score -= 35.0;
    }
    score -= s.bounce_rate_7d() * 600.0;
    score -= s.complaint_rate_7d() * 25_000.0;
    score -= s.defer_rate_24h() * 80.0;
    if s.sent_7d < 20 {
        score = score.min(70.0);
    }
    score.clamp(0.0, 100.0).round() as u8
}

pub fn apply_volume(remaining: u32, decision: VolumeDecision) -> u32 {
    (f64::from(remaining) * decision.multiplier()).floor() as u32
}

pub fn reverse_ipv4(ip: &str) -> Option<String> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    Some(format!("{}.{}.{}.{}", parts[3], parts[2], parts[1], parts[0]))
}

pub fn dnsbl_zones() -> &'static [&'static str] {
    &[
        "zen.spamhaus.org",
        "bl.spamcop.net",
        "b.barracudacentral.org",
        "dnsbl.sorbs.net",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean_active() -> Signals {
        Signals {
            health: Health::Active,
            sent_24h: 200,
            sent_7d: 800,
            bounce_24h: 1,
            bounce_7d: 4,
            complaint_7d: 0,
            deferred_24h: 5,
            blocked_48h: false,
            dnsbl_listed: false,
        }
    }

    #[test]
    fn listed_or_blocked_stops() {
        let mut s = clean_active();
        s.dnsbl_listed = true;
        assert_eq!(decide_volume(&s), VolumeDecision::Stop);
        s.dnsbl_listed = false;
        s.blocked_48h = true;
        assert_eq!(decide_volume(&s), VolumeDecision::Stop);
    }

    #[test]
    fn complaint_spike_stops() {
        let mut s = clean_active();
        s.complaint_7d = 2;
        assert_eq!(decide_volume(&s), VolumeDecision::Stop);
    }

    #[test]
    fn day_bounce_cuts() {
        let s = Signals {
            health: Health::Warmup,
            sent_24h: 100,
            sent_7d: 100,
            bounce_24h: 4,
            bounce_7d: 4,
            complaint_7d: 0,
            deferred_24h: 0,
            blocked_48h: false,
            dnsbl_listed: false,
        };
        assert_eq!(decide_volume(&s), VolumeDecision::Cut);
        assert_eq!(apply_volume(100, VolumeDecision::Cut), 25);
    }

    #[test]
    fn greylist_slows() {
        let s = Signals {
            health: Health::Warmup,
            sent_24h: 50,
            sent_7d: 50,
            bounce_24h: 0,
            bounce_7d: 0,
            complaint_7d: 0,
            deferred_24h: 12,
            blocked_48h: false,
            dnsbl_listed: false,
        };
        assert_eq!(decide_volume(&s), VolumeDecision::Slow);
    }

    #[test]
    fn clean_active_pushes() {
        assert_eq!(decide_volume(&clean_active()), VolumeDecision::Push);
        assert_eq!(apply_volume(100, VolumeDecision::Push), 120);
    }

    #[test]
    fn new_ip_holds_curve() {
        let s = Signals {
            health: Health::Warmup,
            sent_24h: 10,
            sent_7d: 10,
            bounce_24h: 0,
            bounce_7d: 0,
            complaint_7d: 0,
            deferred_24h: 1,
            blocked_48h: false,
            dnsbl_listed: false,
        };
        assert_eq!(decide_volume(&s), VolumeDecision::Hold);
    }

    #[test]
    fn score_drops_on_blocks_and_lists() {
        let clean = reputation_score(&clean_active());
        let mut listed = clean_active();
        listed.dnsbl_listed = true;
        assert!(reputation_score(&listed) < clean);
        assert!(reputation_score(&listed) < 70);
    }

    #[test]
    fn reverse_octets() {
        assert_eq!(reverse_ipv4("203.0.113.10").as_deref(), Some("10.113.0.203"));
        assert_eq!(reverse_ipv4("2001:db8::1"), None);
    }
}
