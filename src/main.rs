use anyhow::Result;
use mta::config::Config;
use mta::db;
use mta::http::{self, App};
use mta::queue;
use mta::smtp_in;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mta=info".parse()?))
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(
        domain = %cfg.root_domain,
        http = %cfg.http_bind,
        smtp = %cfg.smtp_in_bind,
        dry_run = cfg.dry_run,
        "starting mta engine"
    );

    let pool = db::connect(&cfg).await?;
    let app = App {
        cfg: cfg.clone(),
        pool: pool.clone(),
    };

    let http_listener = tokio::net::TcpListener::bind(cfg.http_bind).await?;
    let http = axum::serve(http_listener, http::router(app).into_make_service());

    let workers = {
        let cfg = cfg.clone();
        let pool = pool.clone();
        tokio::spawn(async move { queue::worker_loop(cfg, pool).await })
    };
    let smtp = {
        let cfg = cfg.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = smtp_in::serve(cfg, pool).await {
                tracing::error!(error = %e, "smtp intake died");
            }
        })
    };

    tracing::info!("console on /console  (basic auth admin / ADMIN_PASSWORD)");
    tracing::info!("API on POST /v1/messages");

    tokio::select! {
        r = http => r?,
        _ = workers => {},
        _ = smtp => {},
    }
    Ok(())
}
