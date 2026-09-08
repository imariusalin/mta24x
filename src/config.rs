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
    pub ip_tx: PoolIp,
    pub ip_mkt: PoolIp,
    pub ip_canary: PoolIp,
    pub static_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PoolIp {
    pub address: IpAddr,
    pub hostname: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy_load();
        let root_domain = env("ROOT_DOMAIN").unwrap_or_else(|_| "example.com".into());
        let mail_hostname =
            env("MAIL_HOSTNAME").unwrap_or_else(|_| format!("mail.{root_domain}"));
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
            root_domain: root_domain.clone(),
            worker_concurrency: env("WORKER_CONCURRENCY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(16),
            dry_run: env("DRY_RUN")
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            ip_tx: PoolIp {
                address: env("IP_TX")
                    .unwrap_or_else(|_| "127.0.0.1".into())
                    .parse()
                    .context("IP_TX")?,
                hostname: env("IP_TX_EHLO")
                    .unwrap_or_else(|_| format!("mail.{root_domain}")),
            },
            ip_mkt: PoolIp {
                address: env("IP_MKT")
                    .unwrap_or_else(|_| "127.0.0.2".into())
                    .parse()
                    .context("IP_MKT")?,
                hostname: env("IP_MKT_EHLO")
                    .unwrap_or_else(|_| format!("news-out.{root_domain}")),
            },
            ip_canary: PoolIp {
                address: env("IP_CANARY")
                    .unwrap_or_else(|_| "127.0.0.3".into())
                    .parse()
                    .context("IP_CANARY")?,
                hostname: env("IP_CANARY_EHLO")
                    .unwrap_or_else(|_| format!("out.{root_domain}")),
            },
            static_dir: PathBuf::from(env("STATIC_DIR").unwrap_or_else(|_| "static".into())),
        })
    }
}

fn env(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

fn dotenvy_load() {
    let _ = dotenvy::dotenv();
}
