use crate::pools::{assign_explicit, assign_roles, parse_csv_ips, AssignedIp};
use crate::warming::Role;
use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub http_bind: SocketAddr,
    pub smtp_in_bind: SocketAddr,
    pub admin_password: String,
    pub api_public_url: String,
    pub mail_hostname: String,
    pub root_domain: String,
    pub worker_concurrency: usize,
    pub dry_run: bool,
    pub ips: Vec<AssignedIp>,
    pub static_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy_load();
        let root_domain = env("ROOT_DOMAIN").unwrap_or_else(|_| "example.com".into());
        let mail_hostname =
            env("MAIL_HOSTNAME").unwrap_or_else(|_| format!("mail.{root_domain}"));
        let ips = load_ips(&root_domain, &mail_hostname)?;
        Ok(Self {
            database_url: env("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://mta:mta@127.0.0.1:5432/mta".into()),
            http_bind: env("HTTP_BIND")
                .unwrap_or_else(|_| "0.0.0.0:8787".into())
                .parse()
                .context("HTTP_BIND")?,
            smtp_in_bind: env("SMTP_IN_BIND")
                .unwrap_or_else(|_| "0.0.0.0:2525".into())
                .parse()
                .context("SMTP_IN_BIND")?,
            admin_password: env("ADMIN_PASSWORD").unwrap_or_else(|_| "changeme".into()),
            api_public_url: env("API_PUBLIC_URL")
                .unwrap_or_else(|_| format!("https://{mail_hostname}")),
            mail_hostname,
            root_domain,
            worker_concurrency: env("WORKER_CONCURRENCY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(16),
            dry_run: env("DRY_RUN")
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            ips,
            static_dir: PathBuf::from(env("STATIC_DIR").unwrap_or_else(|_| "static".into())),
        })
    }
}

fn load_ips(root: &str, mail_hostname: &str) -> Result<Vec<AssignedIp>> {
    let pool_tx = env("POOL_TX").ok().filter(|s| !s.trim().is_empty());
    let pool_mkt = env("POOL_MKT").ok().filter(|s| !s.trim().is_empty());
    let pool_canary = env("POOL_CANARY").ok().filter(|s| !s.trim().is_empty());
    if pool_tx.is_some() || pool_mkt.is_some() || pool_canary.is_some() {
        return assign_explicit(
            &parse_csv_ips(pool_tx.as_deref().unwrap_or(""))?,
            &parse_csv_ips(pool_mkt.as_deref().unwrap_or(""))?,
            &parse_csv_ips(pool_canary.as_deref().unwrap_or(""))?,
            root,
            mail_hostname,
        );
    }

    if let Ok(raw) = env("IPS") {
        if !raw.trim().is_empty() {
            let addrs = parse_csv_ips(&raw)?;
            anyhow::ensure!(!addrs.is_empty(), "IPS is empty");
            return Ok(assign_roles(&addrs, root, mail_hostname));
        }
    }

    // Legacy three-variable form still works.
    let legacy: Vec<(Role, String, String)> = [
        (
            Role::Transactional,
            env("IP_TX").unwrap_or_default(),
            env("IP_TX_EHLO").unwrap_or_else(|_| mail_hostname.to_string()),
        ),
        (
            Role::Marketing,
            env("IP_MKT").unwrap_or_default(),
            env("IP_MKT_EHLO").unwrap_or_else(|_| format!("news-out.{root}")),
        ),
        (
            Role::Canary,
            env("IP_CANARY").unwrap_or_default(),
            env("IP_CANARY_EHLO").unwrap_or_else(|_| format!("out.{root}")),
        ),
    ]
    .into_iter()
    .filter(|(_, addr, _)| !addr.trim().is_empty())
    .collect();

    if !legacy.is_empty() {
        let mut out = Vec::new();
        for (role, addr, hostname) in legacy {
            out.push(AssignedIp {
                address: addr.parse().with_context(|| format!("legacy IP {addr}"))?,
                hostname,
                role,
            });
        }
        return Ok(out);
    }

    // Local/dev fallback: a single loopback IP, shared across streams.
    Ok(assign_roles(&[IpAddr::from([127, 0, 0, 1])], root, mail_hostname))
}

fn env(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

fn dotenvy_load() {
    let _ = dotenvy::dotenv();
}
