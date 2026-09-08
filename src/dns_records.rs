use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpDns {
    pub address: String,
    pub ehlo: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkimDns {
    pub domain: String,
    pub selector: String,
    pub public_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsRecord {
    pub host: String,
    pub r#type: &'static str,
    pub value: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtrInstruction {
    pub ip: String,
    pub hostname: String,
}

pub struct DnsBundle {
    pub records: Vec<DnsRecord>,
    pub ptr: Vec<PtrInstruction>,
    pub notes: Vec<String>,
}

pub fn generate_dns(
    root: &str,
    mail_hostname: &str,
    ips: &[IpDns],
    dkim: &[DkimDns],
    dmarc_policy: &str,
    report_mbox: &str,
) -> DnsBundle {
    let root = root.trim_end_matches('.');
    let mut records = Vec::new();
    let mut ptr = Vec::new();

    let inbound = ips
        .iter()
        .find(|i| i.role == "transactional")
        .or_else(|| ips.first());

    if let Some(ip) = inbound {
        records.push(DnsRecord {
            host: mail_hostname.to_string(),
            r#type: "A",
            value: ip.address.clone(),
            purpose: "MX target / IMAP / webmail".into(),
        });
    }

    records.push(DnsRecord {
        host: root.to_string(),
        r#type: "MX",
        value: format!("10 {mail_hostname}."),
        purpose: "Inbound mail for people".into(),
    });

    for sub in ["notify", "news", "bounces"] {
        records.push(DnsRecord {
            host: format!("{sub}.{root}"),
            r#type: "MX",
            value: format!("10 {mail_hostname}."),
            purpose: format!("Inbound for {sub}.{root}"),
        });
    }

    let spf_ips: String = ips
        .iter()
        .map(|i| format!("ip4:{}", i.address))
        .collect::<Vec<_>>()
        .join(" ");

    records.push(DnsRecord {
        host: root.to_string(),
        r#type: "TXT",
        value: format!("v=spf1 mx {spf_ips} -all"),
        purpose: "SPF for the mailbox domain".into(),
    });
    records.push(DnsRecord {
        host: format!("notify.{root}"),
        r#type: "TXT",
        value: format!("v=spf1 {spf_ips} -all"),
        purpose: "SPF for transactional From".into(),
    });
    records.push(DnsRecord {
        host: format!("news.{root}"),
        r#type: "TXT",
        value: format!("v=spf1 {spf_ips} -all"),
        purpose: "SPF for marketing From".into(),
    });
    records.push(DnsRecord {
        host: format!("bounces.{root}"),
        r#type: "TXT",
        value: format!("v=spf1 {spf_ips} -all"),
        purpose: "SPF for VERP Return-Path".into(),
    });

    for key in dkim {
        records.push(DnsRecord {
            host: format!("{}._domainkey.{}", key.selector, key.domain),
            r#type: "TXT",
            value: format!(
                "v=DKIM1; k=rsa; p={}",
                crate::dkim_keys::to_spki_dns_p(&key.public_b64)
            ),
            purpose: format!("DKIM for {}", key.domain),
        });
    }

    let dmarc = format!(
        "v=DMARC1; p={dmarc_policy}; rua=mailto:{report_mbox}; ruf=mailto:{report_mbox}; fo=1; adkim=s; aspf=s"
    );
    records.push(DnsRecord {
        host: format!("_dmarc.{root}"),
        r#type: "TXT",
        value: dmarc.clone(),
        purpose: "DMARC (start p=none, move to quarantine then reject)".into(),
    });
    for sub in ["notify", "news", "bounces"] {
        records.push(DnsRecord {
            host: format!("_dmarc.{sub}.{root}"),
            r#type: "TXT",
            value: dmarc.clone(),
            purpose: format!("DMARC for {sub}.{root}"),
        });
    }

    records.push(DnsRecord {
        host: format!("_mta-sts.{root}"),
        r#type: "TXT",
        value: "v=STSv1; id=20260908".into(),
        purpose: "MTA-STS discovery".into(),
    });
    if let Some(ip) = inbound {
        records.push(DnsRecord {
            host: format!("mta-sts.{root}"),
            r#type: "A",
            value: ip.address.clone(),
            purpose: "MTA-STS policy host (served by the engine)".into(),
        });
    }
    records.push(DnsRecord {
        host: format!("_smtp._tls.{root}"),
        r#type: "TXT",
        value: format!("v=TLSRPTv1; rua=mailto:{report_mbox}"),
        purpose: "TLS reporting".into(),
    });

    for ip in ips {
        ptr.push(PtrInstruction {
            ip: ip.address.clone(),
            hostname: ip.ehlo.clone(),
        });
        records.push(DnsRecord {
            host: ip.ehlo.clone(),
            r#type: "A",
            value: ip.address.clone(),
            purpose: format!("EHLO/PTR pair for {} pool", ip.role),
        });
    }

    let notes = vec![
        "Set PTR/rDNS at the VPS provider. DNS hosting cannot do this.".into(),
        "Each PTR must equal that IP's EHLO hostname exactly.".into(),
        "Publish SPF/DKIM first, DMARC p=none for a week, then quarantine, then reject.".into(),
        "Gmail Postmaster Tools + Microsoft SNDS must be enrolled on these IPs.".into(),
        "Do not send marketing from notify. or from the mailbox domain.".into(),
    ];

    DnsBundle {
        records,
        ptr,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DnsBundle {
        generate_dns(
            "acme.test",
            "mail.acme.test",
            &[
                IpDns {
                    address: "203.0.113.10".into(),
                    ehlo: "mail.acme.test".into(),
                    role: "transactional".into(),
                },
                IpDns {
                    address: "203.0.113.11".into(),
                    ehlo: "news-out.acme.test".into(),
                    role: "marketing".into(),
                },
                IpDns {
                    address: "203.0.113.12".into(),
                    ehlo: "out.acme.test".into(),
                    role: "canary".into(),
                },
            ],
            &[DkimDns {
                domain: "notify.acme.test".into(),
                selector: "mail".into(),
                public_b64: "MIIBIjAN".into(),
            }],
            "none",
            "dmarc@acme.test",
        )
    }

    #[test]
    fn spf_includes_all_three_ips() {
        let bundle = sample();
        let spf = bundle
            .records
            .iter()
            .find(|r| r.host == "notify.acme.test" && r.r#type == "TXT")
            .unwrap();
        assert!(spf.value.contains("ip4:203.0.113.10"));
        assert!(spf.value.contains("ip4:203.0.113.11"));
        assert!(spf.value.contains("ip4:203.0.113.12"));
        assert!(spf.value.contains("-all"));
    }

    #[test]
    fn dkim_and_dmarc_present() {
        let bundle = sample();
        assert!(bundle
            .records
            .iter()
            .any(|r| r.host == "mail._domainkey.notify.acme.test"));
        assert!(bundle
            .records
            .iter()
            .any(|r| r.host == "_dmarc.acme.test" && r.value.contains("p=none")));
    }

    #[test]
    fn ptr_instructions_for_each_ip() {
        let bundle = sample();
        assert_eq!(bundle.ptr.len(), 3);
        assert_eq!(bundle.ptr[0].hostname, "mail.acme.test");
    }
}
