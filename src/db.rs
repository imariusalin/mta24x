use crate::config::Config;
use crate::dkim_keys::generate_rsa_2048;
use crate::pools::{ip_id, role_str as pool_role};
use crate::routing::SendingIp;
use crate::warming::{Health, Role};
use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::info;

const MIGRATIONS: &[&str] = &[
    include_str!("../sql/001_init.sql"),
    include_str!("../sql/002_reputation.sql"),
];

pub async fn connect(cfg: &Config) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&cfg.database_url)
        .await
        .context("postgres")?;
    migrate(&pool).await?;
    seed(cfg, &pool).await?;
    Ok(pool)
}

async fn migrate(pool: &PgPool) -> Result<()> {
    for schema in MIGRATIONS {
        for stmt in schema.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt)
                .execute(pool)
                .await
                .with_context(|| stmt.to_string())?;
        }
    }
    Ok(())
}

async fn seed(cfg: &Config, pool: &PgPool) -> Result<()> {
    anyhow::ensure!(!cfg.ips.is_empty(), "no sending IPs configured");
    for ip in &cfg.ips {
        seed_ip(
            pool,
            &ip_id(&ip.address),
            &ip.address.to_string(),
            &ip.hostname,
            pool_role(ip.role),
        )
        .await?;
    }
    info!(count = cfg.ips.len(), "seeded sending IPs");

    seed_domain(pool, &format!("notify.{}", cfg.root_domain), "transactional").await?;
    seed_domain(pool, &format!("news.{}", cfg.root_domain), "marketing").await?;
    seed_domain(pool, &cfg.root_domain, "mailbox").await?;
    Ok(())
}

async fn seed_ip(pool: &PgPool, id: &str, address: &str, hostname: &str, role: &str) -> Result<()> {
    let updated = sqlx::query("UPDATE ips SET hostname = $2, role = $3 WHERE address = $1")
        .bind(address)
        .bind(hostname)
        .bind(role)
        .execute(pool)
        .await?
        .rows_affected();
    if updated == 0 {
        sqlx::query(
            "INSERT INTO ips (id, address, hostname, role, health)
             VALUES ($1,$2,$3,$4,'warmup')
             ON CONFLICT (id) DO UPDATE SET address = EXCLUDED.address, hostname = EXCLUDED.hostname, role = EXCLUDED.role",
        )
        .bind(id)
        .bind(address)
        .bind(hostname)
        .bind(role)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn seed_domain(pool: &PgPool, name: &str, stream: &str) -> Result<()> {
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM domains WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    if exists.is_some() {
        return Ok(());
    }
    let keys = generate_rsa_2048()?;
    info!(domain = name, "generated DKIM key");
    sqlx::query(
        "INSERT INTO domains (id, name, stream, selector, dkim_private_pem, dkim_public, health)
         VALUES ($1,$2,$3,'mail',$4,$5,'warmup')",
    )
    .bind(name)
    .bind(name)
    .bind(stream)
    .bind(&keys.private_pem)
    .bind(&keys.public_b64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_ips(pool: &PgPool) -> Result<Vec<SendingIp>> {
    let rows: Vec<(String, String, String, String, String)> =
        sqlx::query_as("SELECT id, address, hostname, role, health FROM ips")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, address, hostname, role, health)| {
            Some(SendingIp {
                id,
                address,
                hostname,
                role: parse_role(&role)?,
                health: parse_health(&health)?,
            })
        })
        .collect())
}

pub fn parse_role(s: &str) -> Option<Role> {
    match s {
        "transactional" => Some(Role::Transactional),
        "marketing" => Some(Role::Marketing),
        "canary" => Some(Role::Canary),
        _ => None,
    }
}

pub fn parse_health(s: &str) -> Option<Health> {
    match s {
        "warmup" => Some(Health::Warmup),
        "active" => Some(Health::Active),
        "quarantine" => Some(Health::Quarantine),
        _ => None,
    }
}

pub fn role_str(r: Role) -> &'static str {
    match r {
        Role::Transactional => "transactional",
        Role::Marketing => "marketing",
        Role::Canary => "canary",
    }
}

pub fn health_str(h: Health) -> &'static str {
    match h {
        Health::Warmup => "warmup",
        Health::Active => "active",
        Health::Quarantine => "quarantine",
    }
}
