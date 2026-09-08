use crate::bounce::BounceClass;
use crate::config::Config;
use crate::db::{health_str, load_ips};
use crate::dnsbl;
use crate::reputation::{apply_volume, decide_volume, reputation_score, Signals};
use crate::routing::{pick_ip, SendingIp, Stream};
use crate::smtp_out::{deliver, sign_dkim};
use crate::warming::{
    classify_isp, evaluate_health, remaining_quota, retry_delay_secs, Health, Isp, Reputation, Role,
};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

pub struct Queued {
    pub id: Uuid,
    pub stream: String,
    pub ip_id: String,
    pub domain_id: String,
    pub envelope_from: String,
    pub recipient: String,
    pub raw: Vec<u8>,
    pub attempts: i32,
}

pub async fn enqueue_raw(
    pool: &PgPool,
    cfg: &Config,
    stream: Stream,
    header_from: &str,
    recipient: &str,
    raw: &[u8],
) -> Result<Uuid> {
    let recipient = recipient.trim().to_ascii_lowercase();
    if suppressed(pool, &recipient).await? {
        return Err(anyhow!("recipient is suppressed"));
    }
    let from_domain = domain_of(header_from).ok_or_else(|| anyhow!("From missing domain"))?;
    let (domain_id, pem, selector, _domain_health, domain_age) =
        load_domain(pool, &from_domain).await?;
    let ips = load_ips(pool).await?;
    let ip = pick_ip(stream, &ips).map_err(|e| anyhow!("{e}"))?;
    let isp = classify_isp(domain_of(&recipient).as_deref().unwrap_or(""));
    let ip_age = ip_age_days(pool, &ip.id).await?;
    let (sent_today_ip, sent_hour_ip) = counters(pool, &ip.id, "", isp).await?;
    let (sent_today_dom, _) = counters(pool, &ip.id, &domain_id, isp).await?;
    let listed = listed_flag(pool, &ip.id).await?;
    let signals = load_signals(pool, &ip.id, Some(isp), ip.health, listed).await?;
    let decision = decide_volume(&signals);
    let left = apply_volume(
        remaining_quota(
            ip_age,
            domain_age,
            ip.health,
            isp,
            sent_today_ip,
            sent_today_dom,
            sent_hour_ip,
        ),
        decision,
    );
    let delay = if left == 0 {
        Duration::minutes(15)
    } else {
        Duration::zero()
    };

    let signed = sign_dkim(raw, &from_domain, &selector, &pem).unwrap_or_else(|e| {
        warn!(error = %e, "dkim sign failed, queueing unsigned");
        raw.to_vec()
    });

    let id = Uuid::new_v4();
    let envelope = format!("bounce+{id}@bounces.{}", cfg.root_domain);
    sqlx::query(
        "INSERT INTO messages (id, stream, ip_id, domain_id, envelope_from, recipient, raw, priority, status, next_attempt_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'queued',$9)",
    )
    .bind(id)
    .bind(stream.as_str())
    .bind(&ip.id)
    .bind(&domain_id)
    .bind(&envelope)
    .bind(&recipient)
    .bind(&signed)
    .bind(stream.priority())
    .bind(Utc::now() + delay)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn worker_loop(cfg: Config, pool: PgPool) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tick.tick().await;
        if let Err(e) = drain_once(&cfg, &pool).await {
            warn!(error = %e, "worker cycle");
        }
        if let Err(e) = refresh_health(&pool).await {
            warn!(error = %e, "health cycle");
        }
    }
}

async fn drain_once(cfg: &Config, pool: &PgPool) -> Result<()> {
    let jobs: Vec<Queued> = sqlx::query_as::<_, (Uuid, String, String, String, String, String, Vec<u8>, i32)>(
        "UPDATE messages SET status = 'sending'
         WHERE id IN (
            SELECT id FROM messages
            WHERE status = 'queued' AND next_attempt_at <= now()
            ORDER BY priority, created_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
         )
         RETURNING id, stream, COALESCE(ip_id,''), COALESCE(domain_id,''), envelope_from, recipient, raw, attempts",
    )
    .bind(cfg.worker_concurrency as i32)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(id, stream, ip_id, domain_id, envelope_from, recipient, raw, attempts)| Queued {
                id,
                stream,
                ip_id,
                domain_id,
                envelope_from,
                recipient,
                raw,
                attempts,
            })
            .collect()
    })?;

    for job in jobs {
        let pool = pool.clone();
        let dry = cfg.dry_run;
        tokio::spawn(async move {
            if let Err(e) = process_job(&pool, job, dry).await {
                warn!(error = %e, "job failed");
            }
        });
    }
    Ok(())
}

async fn process_job(pool: &PgPool, job: Queued, dry_run: bool) -> Result<()> {
    let ips = load_ips(pool).await?;
    let ip = ips
        .iter()
        .find(|i| i.id == job.ip_id)
        .cloned()
        .or_else(|| {
            Stream::parse(&job.stream)
                .and_then(|s| pick_ip(s, &ips).ok())
                .cloned()
        });
    let Some(ip) = ip else {
        return finish(pool, &job, BounceClass::Defer, 450, "no ip", false).await;
    };
    let result = deliver(&ip, &job.envelope_from, &job.recipient, &job.raw, dry_run).await;
    bump_counter(
        pool,
        &ip.id,
        &job.domain_id,
        classify_isp(domain_of(&job.recipient).as_deref().unwrap_or("")),
        result.class,
    )
    .await?;
    finish(pool, &job, result.class, result.code, &result.detail, true).await
}

async fn finish(
    pool: &PgPool,
    job: &Queued,
    class: BounceClass,
    code: u16,
    detail: &str,
    _count: bool,
) -> Result<()> {
    sqlx::query("INSERT INTO events (message_id, kind, detail, smtp_code) VALUES ($1,$2,$3,$4)")
        .bind(job.id)
        .bind(class.as_str())
        .bind(detail)
        .bind(code as i32)
        .execute(pool)
        .await?;

    match class {
        BounceClass::Success => {
            sqlx::query("UPDATE messages SET status='sent', sent_at=now(), smtp_code=$2, last_error=NULL WHERE id=$1")
                .bind(job.id)
                .bind(code as i32)
                .execute(pool)
                .await?;
        }
        BounceClass::Hard | BounceClass::Block => {
            sqlx::query("UPDATE messages SET status='bounced', smtp_code=$2, last_error=$3, attempts=attempts+1 WHERE id=$1")
                .bind(job.id)
                .bind(code as i32)
                .bind(detail)
                .execute(pool)
                .await?;
            sqlx::query(
                "INSERT INTO suppressions (email, reason) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(&job.recipient)
            .bind(class.as_str())
            .execute(pool)
            .await?;
        }
        BounceClass::Defer | BounceClass::Soft => {
            let next_attempt = job.attempts as u32 + 1;
            if let Some(secs) = retry_delay_secs(next_attempt) {
                sqlx::query(
                    "UPDATE messages SET status='queued', attempts=attempts+1, smtp_code=$2, last_error=$3, next_attempt_at=$4 WHERE id=$1",
                )
                .bind(job.id)
                .bind(code as i32)
                .bind(detail)
                .bind(Utc::now() + Duration::seconds(secs as i64))
                .execute(pool)
                .await?;
            } else {
                sqlx::query("UPDATE messages SET status='bounced', attempts=attempts+1, smtp_code=$2, last_error=$3 WHERE id=$1")
                    .bind(job.id)
                    .bind(code as i32)
                    .bind("retries exhausted")
                    .execute(pool)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn bump_counter(
    pool: &PgPool,
    ip_id: &str,
    domain_id: &str,
    isp: Isp,
    class: BounceClass,
) -> Result<()> {
    let hour = Utc::now().format("%Y-%m-%d %H:00:00").to_string();
    sqlx::query(
        "INSERT INTO send_counters (ip_id, domain_id, isp, bucket, sent, bounced, complained, deferred, blocked)
         VALUES ($1,$2,$3,$4::timestamptz,1,0,0,0,0)
         ON CONFLICT (ip_id, domain_id, isp, bucket) DO UPDATE SET
            sent = send_counters.sent + 1,
            bounced = send_counters.bounced + CASE WHEN $5 THEN 1 ELSE 0 END,
            deferred = send_counters.deferred + CASE WHEN $6 THEN 1 ELSE 0 END,
            blocked = send_counters.blocked + CASE WHEN $7 THEN 1 ELSE 0 END",
    )
    .bind(ip_id)
    .bind(domain_id)
    .bind(format!("{isp:?}").to_lowercase())
    .bind(&hour)
    .bind(class == BounceClass::Hard)
    .bind(class == BounceClass::Defer || class == BounceClass::Soft)
    .bind(class == BounceClass::Block)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn refresh_health(pool: &PgPool) -> Result<()> {
    let ips = load_ips(pool).await?;
    for ip in ips {
        let age = ip_age_days(pool, &ip.id).await?;
        let due = probe_due(pool, &ip.id).await?;
        let mut listed = listed_flag(pool, &ip.id).await?;
        let mut listed_on = String::new();
        if due {
            match dnsbl::listed_on(&ip.address).await {
                Ok(hits) => {
                    listed = !hits.is_empty();
                    listed_on = hits.join(",");
                }
                Err(e) => warn!(ip = %ip.id, error = %e, "dnsbl probe failed"),
            }
        }
        let signals = load_signals(pool, &ip.id, None, ip.health, listed).await?;
        let decision = decide_volume(&signals);
        let score = reputation_score(&signals);
        let next = evaluate_health(&Reputation {
            age_days: age,
            sent_7d: signals.sent_7d,
            hard_bounce_7d: signals.bounce_7d,
            complaint_7d: signals.complaint_7d,
            blocked_48h: signals.blocked_48h,
            current: ip.health,
        });
        if next != ip.health {
            info!(ip = %ip.id, from = ?ip.health, to = ?next, "health change");
        }
        if due {
            sqlx::query(
                "UPDATE ips SET health=$2, reputation_score=$3, volume_decision=$4, listed_on=$5, last_probe_at=now(),
                 graduated_at = CASE WHEN $2='active' THEN COALESCE(graduated_at, now()) ELSE graduated_at END
                 WHERE id=$1",
            )
            .bind(&ip.id)
            .bind(health_str(next))
            .bind(score as i32)
            .bind(decision.as_str())
            .bind(&listed_on)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE ips SET health=$2, reputation_score=$3, volume_decision=$4,
                 graduated_at = CASE WHEN $2='active' THEN COALESCE(graduated_at, now()) ELSE graduated_at END
                 WHERE id=$1",
            )
            .bind(&ip.id)
            .bind(health_str(next))
            .bind(score as i32)
            .bind(decision.as_str())
            .execute(pool)
            .await?;
        }
        let _ = Role::Canary;
    }
    Ok(())
}

async fn listed_flag(pool: &PgPool, ip_id: &str) -> Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT listed_on FROM ips WHERE id=$1")
        .bind(ip_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(s,)| !s.is_empty()).unwrap_or(false))
}

async fn probe_due(pool: &PgPool, ip_id: &str) -> Result<bool> {
    let row: Option<(Option<chrono::DateTime<Utc>>,)> =
        sqlx::query_as("SELECT last_probe_at FROM ips WHERE id=$1")
            .bind(ip_id)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((Some(at),)) => Utc::now() - at > Duration::hours(6),
        _ => true,
    })
}

pub async fn load_signals(
    pool: &PgPool,
    ip_id: &str,
    isp: Option<Isp>,
    health: Health,
    dnsbl_listed: bool,
) -> Result<Signals> {
    let isp_s = isp.map(|i| format!("{i:?}").to_lowercase());
    let week: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(sent),0), COALESCE(SUM(bounced),0), COALESCE(SUM(complained),0), COALESCE(SUM(blocked),0)
         FROM send_counters
         WHERE ip_id=$1 AND bucket > now() - interval '7 days'
           AND ($2::text IS NULL OR isp=$2)",
    )
    .bind(ip_id)
    .bind(&isp_s)
    .fetch_one(pool)
    .await?;
    let day: (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(sent),0), COALESCE(SUM(bounced),0), COALESCE(SUM(deferred),0)
         FROM send_counters
         WHERE ip_id=$1 AND bucket > now() - interval '24 hours'
           AND ($2::text IS NULL OR isp=$2)",
    )
    .bind(ip_id)
    .bind(&isp_s)
    .fetch_one(pool)
    .await?;
    let blocked_48: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(blocked),0) FROM send_counters
         WHERE ip_id=$1 AND bucket > now() - interval '48 hours'",
    )
    .bind(ip_id)
    .fetch_one(pool)
    .await?;
    Ok(Signals {
        health,
        sent_24h: day.0 as u32,
        sent_7d: week.0 as u32,
        bounce_24h: day.1 as u32,
        bounce_7d: week.1 as u32,
        complaint_7d: week.2 as u32,
        deferred_24h: day.2 as u32,
        blocked_48h: blocked_48.0 > 0,
        dnsbl_listed,
    })
}

pub async fn record_verp_bounce(pool: &PgPool, id: Uuid, raw: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(raw);
    let class = if text.to_ascii_lowercase().contains("5.7") {
        BounceClass::Block
    } else {
        BounceClass::Hard
    };
    let rec: Option<(String,)> = sqlx::query_as("SELECT recipient FROM messages WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    sqlx::query("UPDATE messages SET status='bounced', last_error='verp dsn' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    if let Some((email,)) = rec {
        sqlx::query(
            "INSERT INTO suppressions (email, reason) VALUES ($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(&email)
        .bind(class.as_str())
        .execute(pool)
        .await?;
    }
    sqlx::query("INSERT INTO events (message_id, kind, detail) VALUES ($1,$2,$3)")
        .bind(id)
        .bind(class.as_str())
        .bind("inbound DSN")
        .execute(pool)
        .await?;
    Ok(())
}

async fn suppressed(pool: &PgPool, email: &str) -> Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT email FROM suppressions WHERE email=$1")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

async fn load_domain(
    pool: &PgPool,
    name: &str,
) -> Result<(String, String, String, Health, u32)> {
    let row: Option<(String, String, String, String, Option<chrono::DateTime<Utc>>)> =
        sqlx::query_as(
            "SELECT id, dkim_private_pem, selector, health, first_sent_at FROM domains WHERE name=$1",
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;
    let Some((id, pem, selector, health, first)) = row else {
        return Err(anyhow!("unknown sending domain {name} — add it in the console"));
    };
    let age = first
        .map(|t| (Utc::now() - t).num_days().max(0) as u32)
        .unwrap_or(0);
    if first.is_none() {
        sqlx::query("UPDATE domains SET first_sent_at=now() WHERE id=$1")
            .bind(&id)
            .execute(pool)
            .await?;
    }
    Ok((
        id,
        pem,
        selector,
        crate::db::parse_health(&health).unwrap_or(Health::Warmup),
        age,
    ))
}

async fn ip_age_days(pool: &PgPool, id: &str) -> Result<u32> {
    let created: (chrono::DateTime<Utc>,) =
        sqlx::query_as("SELECT created_at FROM ips WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok((Utc::now() - created.0).num_days().max(0) as u32)
}

async fn counters(pool: &PgPool, ip_id: &str, domain_id: &str, isp: Isp) -> Result<(u32, u32)> {
    let isp_s = format!("{isp:?}").to_lowercase();
    let today: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(sent),0) FROM send_counters
         WHERE ip_id=$1 AND ($2 = '' OR domain_id=$2) AND isp=$3 AND bucket::date = CURRENT_DATE",
    )
    .bind(ip_id)
    .bind(domain_id)
    .bind(&isp_s)
    .fetch_one(pool)
    .await?;
    let hour: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(sent),0) FROM send_counters
         WHERE ip_id=$1 AND isp=$3 AND bucket = date_trunc('hour', now())
           AND ($2 = '' OR domain_id=$2)",
    )
    .bind(ip_id)
    .bind(domain_id)
    .bind(&isp_s)
    .fetch_one(pool)
    .await?;
    Ok((today.0 as u32, hour.0 as u32))
}

fn domain_of(addr: &str) -> Option<String> {
    let addr = addr
        .rsplit('<')
        .next()?
        .trim_end_matches('>')
        .trim();
    addr.rsplit('@').next().map(|d| d.to_ascii_lowercase())
}

pub async fn create_api_key(pool: &PgPool, name: &str) -> Result<(String, String)> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    let mut raw = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut raw);
    let secret = format!("sk_live_{}", hex::encode(raw));
    let hash = hex::encode(Sha256::digest(secret.as_bytes()));
    let prefix: String = secret.chars().take(12).collect();
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO api_keys (id, name, hash, prefix) VALUES ($1,$2,$3,$4)")
        .bind(&id)
        .bind(name)
        .bind(&hash)
        .bind(&prefix)
        .execute(pool)
        .await?;
    Ok((id, secret))
}

pub async fn api_key_ok(pool: &PgPool, presented: &str) -> Result<bool> {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(presented.as_bytes()));
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM api_keys WHERE hash=$1")
        .bind(&hash)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

// silence unused import in some builds
#[allow(dead_code)]
fn _ip_type(_: SendingIp) {}
