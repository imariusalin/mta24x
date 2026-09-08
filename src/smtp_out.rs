use crate::bounce::{classify_smtp, BounceClass};
use crate::routing::SendingIp;
use anyhow::{anyhow, Context, Result};
use hickory_resolver::TokioResolver;
use mail_auth::common::crypto::{RsaKey, Sha256};
use mail_auth::common::headers::HeaderWriter;
use mail_auth::dkim::DkimSigner;
use mail_send::smtp::message::Message;
use mail_send::SmtpClientBuilder;
use std::net::IpAddr;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug)]
pub struct DeliveryResult {
    pub class: BounceClass,
    pub code: u16,
    pub detail: String,
}

pub fn sign_dkim(raw: &[u8], domain: &str, selector: &str, private_pem: &str) -> Result<Vec<u8>> {
    #[allow(deprecated)]
    let key = RsaKey::<Sha256>::from_pkcs8_pem(private_pem).context("dkim pem")?;
    let sig = DkimSigner::from_key(key)
        .domain(domain)
        .selector(selector)
        .headers(["From", "To", "Subject", "Date", "Message-ID", "MIME-Version"])
        .sign(raw)
        .context("dkim sign")?;
    let mut out = Vec::with_capacity(raw.len() + 512);
    sig.write_header(&mut out);
    out.extend_from_slice(raw);
    Ok(out)
}

pub async fn deliver(
    ip: &SendingIp,
    envelope_from: &str,
    recipient: &str,
    raw: &[u8],
    dry_run: bool,
) -> DeliveryResult {
    if dry_run {
        info!(ip = %ip.address, to = recipient, "dry-run skip MX");
        return DeliveryResult {
            class: BounceClass::Success,
            code: 250,
            detail: "dry-run".into(),
        };
    }
    match deliver_inner(ip, envelope_from, recipient, raw).await {
        Ok(r) => r,
        Err(e) => classify_error(&e),
    }
}

async fn deliver_inner(
    ip: &SendingIp,
    envelope_from: &str,
    recipient: &str,
    raw: &[u8],
) -> Result<DeliveryResult> {
    let domain = recipient
        .rsplit('@')
        .next()
        .ok_or_else(|| anyhow!("bad recipient"))?;
    let resolver = TokioResolver::builder_tokio()
        .context("resolver")?
        .build();
    let mx = resolver.mx_lookup(domain).await.context("mx lookup")?;
    let mut hosts: Vec<(u16, String)> = mx
        .iter()
        .map(|r| (r.preference(), r.exchange().to_ascii().trim_end_matches('.').to_string()))
        .collect();
    hosts.sort_by_key(|(p, _)| *p);
    if hosts.is_empty() {
        hosts.push((0, domain.to_string()));
    }

    let local: IpAddr = ip.address.parse().context("ip parse")?;
    let mut last = anyhow!("no mx hosts");
    for (_, host) in hosts.into_iter().take(5) {
        match try_host(&host, local, &ip.hostname, envelope_from, recipient, raw).await {
            Ok(r) => return Ok(r),
            Err(e) => {
                warn!(mx = %host, error = %e, "mx attempt failed");
                last = e;
            }
        }
    }
    Err(last)
}

async fn try_host(
    host: &str,
    local: IpAddr,
    ehlo: &str,
    envelope_from: &str,
    recipient: &str,
    raw: &[u8],
) -> Result<DeliveryResult> {
    let timeout = Duration::from_secs(60);

    match SmtpClientBuilder::new(host, 25)
        .implicit_tls(false)
        .helo_host(ehlo)
        .timeout(timeout)
        .local_ip(local)
        .connect()
        .await
    {
        Ok(mut client) => {
            let msg = Message::new(envelope_from, [recipient], raw);
            match client.send(msg).await {
                Ok(()) => {
                    return Ok(DeliveryResult {
                        class: BounceClass::Success,
                        code: 250,
                        detail: format!("delivered via {host} starttls"),
                    })
                }
                Err(e) => warn!(mx = host, error = %e, "starttls send failed"),
            }
        }
        Err(e) => warn!(mx = host, error = %e, "starttls connect failed"),
    }

    let mut client = SmtpClientBuilder::new(host, 25)
        .helo_host(ehlo)
        .timeout(timeout)
        .local_ip(local)
        .connect_plain()
        .await
        .with_context(|| format!("plain connect {host}"))?;
    let msg = Message::new(envelope_from, [recipient], raw);
    match client.send(msg).await {
        Ok(()) => Ok(DeliveryResult {
            class: BounceClass::Success,
            code: 250,
            detail: format!("delivered via {host} plain"),
        }),
        Err(e) => Ok(classify_error(&anyhow!("{e}"))),
    }
}

fn classify_error(err: &anyhow::Error) -> DeliveryResult {
    let text = format!("{err:#}");
    let code = extract_code(&text).unwrap_or(450);
    let class = classify_smtp(code, None, &text);
    DeliveryResult {
        class,
        code,
        detail: text,
    }
}

fn extract_code(text: &str) -> Option<u16> {
    for token in text.split(|c: char| !c.is_ascii_digit()) {
        if token.len() == 3 {
            if let Ok(n) = token.parse::<u16>() {
                if (200..600).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}
