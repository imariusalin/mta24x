use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isp {
    Gmail,
    Microsoft,
    Yahoo,
    Apple,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Transactional,
    Marketing,
    Canary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Warmup,
    Active,
    Quarantine,
}

#[derive(Debug, Clone)]
pub struct Reputation {
    pub age_days: u32,
    pub sent_7d: u32,
    pub hard_bounce_7d: u32,
    pub complaint_7d: u32,
    pub blocked_48h: bool,
    pub current: Health,
}

impl Reputation {
    pub fn hard_bounce_rate(&self) -> f64 {
        if self.sent_7d == 0 {
            0.0
        } else {
            f64::from(self.hard_bounce_7d) / f64::from(self.sent_7d)
        }
    }

    pub fn complaint_rate(&self) -> f64 {
        if self.sent_7d == 0 {
            0.0
        } else {
            f64::from(self.complaint_7d) / f64::from(self.sent_7d)
        }
    }
}

/// Per-ISP independent daily cap during warmup.
/// Gmail is the strictest curve; Apple is slower still.
pub fn warmup_daily_cap(age_days: u32, isp: Isp) -> u32 {
    let base: u32 = match age_days {
        0 => 20,
        1 => 50,
        2 => 100,
        3 => 180,
        4 => 300,
        5 => 500,
        6 => 800,
        7 => 1_200,
        8..=10 => 2_000,
        11..=13 => 3_500,
        14..=20 => 6_000,
        21..=29 => 12_000,
        _ => 50_000,
    };
    let factor: u32 = match isp {
        Isp::Gmail => 100,
        Isp::Microsoft => 80,
        Isp::Yahoo => 70,
        Isp::Apple => 50,
        Isp::Other => 110,
    };
    base.saturating_mul(factor) / 100
}

pub fn isp_hourly_cap(isp: Isp, health: Health) -> u32 {
    let active = match isp {
        Isp::Gmail => 4_000,
        Isp::Microsoft => 2_500,
        Isp::Yahoo => 1_500,
        Isp::Apple => 800,
        Isp::Other => 3_000,
    };
    match health {
        Health::Active => active,
        Health::Warmup => active / 8,
        Health::Quarantine => 0,
    }
}

pub fn isp_concurrency(isp: Isp) -> u32 {
    match isp {
        Isp::Gmail => 8,
        Isp::Microsoft => 4,
        Isp::Yahoo => 3,
        Isp::Apple => 2,
        Isp::Other => 6,
    }
}

/// Remaining sends allowed right now for this IP+ISP+domain combo.
pub fn remaining_quota(
    ip_age_days: u32,
    domain_age_days: u32,
    ip_health: Health,
    isp: Isp,
    sent_today_ip_isp: u32,
    sent_today_domain_isp: u32,
    sent_this_hour_ip_isp: u32,
) -> u32 {
    if ip_health == Health::Quarantine {
        return 0;
    }
    let ip_daily = warmup_daily_cap(ip_age_days, isp);
    let domain_daily = warmup_daily_cap(domain_age_days, isp);
    let hourly = isp_hourly_cap(isp, ip_health);
    let left_ip = ip_daily.saturating_sub(sent_today_ip_isp);
    let left_domain = domain_daily.saturating_sub(sent_today_domain_isp);
    let left_hour = hourly.saturating_sub(sent_this_hour_ip_isp);
    left_ip.min(left_domain).min(left_hour)
}

pub fn evaluate_health(s: &Reputation) -> Health {
    if s.current == Health::Quarantine {
        return Health::Quarantine;
    }
    if s.blocked_48h {
        return Health::Quarantine;
    }
    if s.sent_7d >= 100 && s.complaint_rate() > 0.001 {
        return Health::Quarantine;
    }
    if s.sent_7d >= 100 && s.hard_bounce_rate() > 0.05 {
        return Health::Quarantine;
    }
    if s.age_days >= 14
        && s.sent_7d >= 500
        && s.hard_bounce_rate() < 0.02
        && s.complaint_rate() < 0.0008
        && !s.blocked_48h
    {
        return Health::Active;
    }
    Health::Warmup
}

pub fn classify_isp(domain: &str) -> Isp {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let last_two = last_labels(&d, 2);
    let last_three = last_labels(&d, 3);
    match last_two.as_str() {
        "gmail.com" | "googlemail.com" | "google.com" => Isp::Gmail,
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" | "outlook.fr"
        | "hotmail.co.uk" | "live.co.uk" => Isp::Microsoft,
        "yahoo.com" | "yahoo.co.uk" | "yahoo.fr" | "ymail.com" | "aol.com" | "rocketmail.com" => {
            Isp::Yahoo
        }
        "icloud.com" | "me.com" | "mac.com" => Isp::Apple,
        _ if last_three == "mail.protection.outlook.com" || last_two == "office365.com" => {
            Isp::Microsoft
        }
        _ => Isp::Other,
    }
}

fn last_labels(domain: &str, n: usize) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() <= n {
        domain.to_string()
    } else {
        parts[parts.len() - n..].join(".")
    }
}

pub fn retry_delay_secs(attempt: u32) -> Option<u64> {
    match attempt {
        0 => Some(0),
        1 => Some(2 * 60),
        2 => Some(10 * 60),
        3 => Some(30 * 60),
        4 => Some(2 * 3600),
        5 => Some(8 * 3600),
        6 => Some(24 * 3600),
        7 => Some(24 * 3600),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_zero_gmail_is_tiny() {
        assert_eq!(warmup_daily_cap(0, Isp::Gmail), 20);
        assert!(warmup_daily_cap(0, Isp::Apple) < warmup_daily_cap(0, Isp::Gmail));
    }

    #[test]
    fn ramp_is_monotonic() {
        let mut prev = 0;
        for day in 0..40 {
            let cap = warmup_daily_cap(day, Isp::Gmail);
            assert!(cap >= prev, "day {day} dropped {prev} -> {cap}");
            prev = cap;
        }
    }

    #[test]
    fn gmail_is_stricter_than_other() {
        assert!(warmup_daily_cap(5, Isp::Gmail) < warmup_daily_cap(5, Isp::Other));
    }

    #[test]
    fn quarantine_has_zero_quota() {
        let left = remaining_quota(30, 30, Health::Quarantine, Isp::Gmail, 0, 0, 0);
        assert_eq!(left, 0);
    }

    #[test]
    fn domain_and_ip_caps_both_apply() {
        let left = remaining_quota(0, 0, Health::Warmup, Isp::Gmail, 19, 0, 0);
        assert_eq!(left, 1);
        let left = remaining_quota(0, 0, Health::Warmup, Isp::Gmail, 0, 20, 0);
        assert_eq!(left, 0);
    }

    #[test]
    fn complaint_spike_quarantines() {
        let r = Reputation {
            age_days: 20,
            sent_7d: 1000,
            hard_bounce_7d: 5,
            complaint_7d: 3,
            blocked_48h: false,
            current: Health::Active,
        };
        assert_eq!(evaluate_health(&r), Health::Quarantine);
    }

    #[test]
    fn clean_two_weeks_graduates() {
        let r = Reputation {
            age_days: 14,
            sent_7d: 800,
            hard_bounce_7d: 4,
            complaint_7d: 0,
            blocked_48h: false,
            current: Health::Warmup,
        };
        assert_eq!(evaluate_health(&r), Health::Active);
    }

    #[test]
    fn quarantine_is_sticky() {
        let r = Reputation {
            age_days: 40,
            sent_7d: 800,
            hard_bounce_7d: 0,
            complaint_7d: 0,
            blocked_48h: false,
            current: Health::Quarantine,
        };
        assert_eq!(evaluate_health(&r), Health::Quarantine);
    }

    #[test]
    fn block_quarantines_immediately() {
        let r = Reputation {
            age_days: 2,
            sent_7d: 10,
            hard_bounce_7d: 0,
            complaint_7d: 0,
            blocked_48h: true,
            current: Health::Warmup,
        };
        assert_eq!(evaluate_health(&r), Health::Quarantine);
    }

    #[test]
    fn isp_from_gmail_and_googlemail() {
        assert_eq!(classify_isp("gmail.com"), Isp::Gmail);
        assert_eq!(classify_isp("mail.google.com"), Isp::Gmail);
        assert_eq!(classify_isp("user.googlemail.com"), Isp::Gmail);
    }

    #[test]
    fn isp_from_microsoft() {
        assert_eq!(classify_isp("outlook.com"), Isp::Microsoft);
        assert_eq!(classify_isp("hotmail.com"), Isp::Microsoft);
        assert_eq!(classify_isp("contoso.office365.com"), Isp::Microsoft);
    }

    #[test]
    fn retry_gives_up_after_week() {
        assert!(retry_delay_secs(7).is_some());
        assert_eq!(retry_delay_secs(8), None);
    }
}
