use crate::config::Config;
use crate::db::{health_str, load_ips, parse_health};
use crate::dns_records::{generate_dns, DkimDns, IpDns};
use crate::queue::{api_key_ok, create_api_key, enqueue_raw};
use crate::routing::Stream;
use crate::warming::{evaluate_health, Reputation};
use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use hmac::{Hmac, Mac};
use mail_send::mail_builder::headers::raw::Raw;
use mail_send::mail_builder::MessageBuilder;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::services::ServeDir;
use uuid::Uuid;

#[derive(Clone)]
pub struct App {
    pub cfg: Config,
    pub pool: PgPool,
}

pub fn router(app: App) -> Router {
    let static_dir = app.cfg.static_dir.clone();
    Router::new()
        .route("/health", get(health))
        .route("/v1/messages", post(send_message))
        .route("/v1/messages/{id}", get(get_message))
        .route("/v1/suppressions", post(add_suppression).get(list_suppressions))
        .route("/console", get(console_index))
        .route("/console/api/overview", get(admin_overview))
        .route("/console/api/dns", get(admin_dns))
        .route("/console/api/keys", post(admin_create_key).get(admin_list_keys))
        .route("/console/api/ips/{id}/health", post(admin_set_health))
        .route("/console/api/messages", get(admin_messages))
        .route("/u/{token}", get(unsub_get).post(unsub_post))
        .route("/.well-known/mta-sts.txt", get(mta_sts))
        .nest_service("/console/static", ServeDir::new(static_dir))
        .with_state(Arc::new(app))
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct SendReq {
    from: String,
    to: Vec<String>,
    subject: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Serialize)]
struct SendResp {
    ids: Vec<Uuid>,
    status: &'static str,
}

async fn send_message(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<SendReq>,
) -> Response {
    if let Err(resp) = require_api(&app, &headers).await {
        return resp;
    }
    let stream = body
        .stream
        .as_deref()
        .and_then(Stream::parse)
        .unwrap_or(Stream::Transactional);
    if body.to.is_empty() {
        return err(StatusCode::BAD_REQUEST, "to is required");
    }
    let from_addr = extract_email(&body.from);
    let mut ids = Vec::new();
    for to in &body.to {
        let unsub = unsub_url(&app.cfg, to);
        let mut builder = MessageBuilder::new()
            .from(body.from.as_str())
            .to(to.as_str())
            .subject(body.subject.as_str())
            .message_id(format!("<{}@{}>", Uuid::new_v4(), app.cfg.root_domain));
        if let Some(t) = &body.text {
            builder = builder.text_body(t.as_str());
        }
        if let Some(h) = &body.html {
            builder = builder.html_body(h.as_str());
        }
        if stream == Stream::Marketing {
            builder = builder
                .header("List-Unsubscribe", Raw::new(format!("<{unsub}>")))
                .header("List-Unsubscribe-Post", Raw::new("List-Unsubscribe=One-Click"));
        }
        for (k, v) in &body.headers {
            builder = builder.header(k.as_str(), Raw::new(v.clone()));
        }
        let raw = match builder.write_to_vec() {
            Ok(b) => b,
            Err(e) => return err(StatusCode::BAD_REQUEST, &e.to_string()),
        };
        match enqueue_raw(&app.pool, &app.cfg, stream, &from_addr, to, &raw).await {
            Ok(id) => ids.push(id),
            Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()),
        }
    }
    Json(SendResp {
        ids,
        status: "queued",
    })
    .into_response()
}

async fn get_message(State(app): State<Arc<App>>, Path(id): Path<Uuid>) -> Response {
    let row: Option<(Uuid, String, String, String, Option<i32>, Option<String>, chrono::DateTime<Utc>)> =
        match sqlx::query_as(
            "SELECT id, status, recipient, stream, smtp_code, last_error, created_at FROM messages WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&app.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    match row {
        Some((id, status, recipient, stream, code, last_error, created_at)) => Json(serde_json::json!({
            "id": id,
            "status": status,
            "recipient": recipient,
            "stream": stream,
            "smtp_code": code,
            "last_error": last_error,
            "created_at": created_at,
        }))
        .into_response(),
        None => err(StatusCode::NOT_FOUND, "not found"),
    }
}

#[derive(Deserialize)]
struct SuppressionReq {
    email: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn add_suppression(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<SuppressionReq>,
) -> Response {
    if let Err(resp) = require_api(&app, &headers).await {
        return resp;
    }
    let email = body.email.to_ascii_lowercase();
    let reason = body.reason.unwrap_or_else(|| "manual".into());
    if let Err(e) = sqlx::query(
        "INSERT INTO suppressions (email, reason) VALUES ($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(&email)
    .bind(&reason)
    .execute(&app.pool)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(serde_json::json!({"email": email, "reason": reason})).into_response()
}

async fn list_suppressions(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_api(&app, &headers).await {
        return resp;
    }
    let rows: Vec<(String, String, chrono::DateTime<Utc>)> =
        match sqlx::query_as("SELECT email, reason, created_at FROM suppressions ORDER BY created_at DESC LIMIT 500")
            .fetch_all(&app.pool)
            .await
        {
            Ok(r) => r,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    Json(rows
        .into_iter()
        .map(|(email, reason, created_at)| serde_json::json!({ "email": email, "reason": reason, "created_at": created_at }))
        .collect::<Vec<_>>())
    .into_response()
}

async fn console_index(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let path = app.cfg.static_dir.join("index.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => Html(include_str!("../static/index.html")).into_response(),
    }
}

async fn admin_overview(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let ips = match load_ips(&app.pool).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let mut ip_json = Vec::new();
    for ip in ips {
        let stats: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(sent),0), COALESCE(SUM(bounced),0), COALESCE(SUM(complained),0), COALESCE(SUM(blocked),0)
             FROM send_counters WHERE ip_id=$1 AND bucket > now() - interval '7 days'",
        )
        .bind(&ip.id)
        .fetch_one(&app.pool)
        .await
        .unwrap_or((0, 0, 0, 0));
        ip_json.push(serde_json::json!({
            "id": ip.id,
            "address": ip.address,
            "hostname": ip.hostname,
            "role": format!("{:?}", ip.role).to_lowercase(),
            "health": health_str(ip.health),
            "sent_7d": stats.0,
            "bounced_7d": stats.1,
            "complained_7d": stats.2,
            "blocked_7d": stats.3,
        }));
    }
    let queued: (i64,) = sqlx::query_as("SELECT count(*) FROM messages WHERE status IN ('queued','sending')")
        .fetch_one(&app.pool)
        .await
        .unwrap_or((0,));
    let sent_today: (i64,) = sqlx::query_as("SELECT count(*) FROM messages WHERE sent_at::date = CURRENT_DATE")
        .fetch_one(&app.pool)
        .await
        .unwrap_or((0,));
    Json(serde_json::json!({
        "root_domain": app.cfg.root_domain,
        "mail_hostname": app.cfg.mail_hostname,
        "dry_run": app.cfg.dry_run,
        "queued": queued.0,
        "sent_today": sent_today.0,
        "ips": ip_json,
    }))
    .into_response()
}

async fn admin_dns(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let ips = match load_ips(&app.pool).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let domains: Vec<(String, String, String)> =
        sqlx::query_as("SELECT name, selector, dkim_public FROM domains")
            .fetch_all(&app.pool)
            .await
            .unwrap_or_default();
    let ip_dns: Vec<IpDns> = ips
        .iter()
        .map(|i| IpDns {
            address: i.address.clone(),
            ehlo: i.hostname.clone(),
            role: format!("{:?}", i.role).to_lowercase(),
        })
        .collect();
    let dkim: Vec<DkimDns> = domains
        .into_iter()
        .map(|(name, selector, public_b64)| DkimDns {
            domain: name,
            selector,
            public_b64,
        })
        .collect();
    let bundle = generate_dns(
        &app.cfg.root_domain,
        &app.cfg.mail_hostname,
        &ip_dns,
        &dkim,
        "none",
        &format!("dmarc@{}", app.cfg.root_domain),
    );
    Json(serde_json::json!({
        "records": bundle.records,
        "ptr": bundle.ptr,
        "notes": bundle.notes,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct KeyReq {
    name: String,
}

async fn admin_create_key(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<KeyReq>,
) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    match create_api_key(&app.pool, &body.name).await {
        Ok((id, secret)) => Json(serde_json::json!({"id": id, "secret": secret, "name": body.name}))
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_list_keys(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let rows: Vec<(String, String, String, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, name, prefix, created_at FROM api_keys ORDER BY created_at DESC",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    Json(rows
        .into_iter()
        .map(|(id, name, prefix, created_at)| {
            serde_json::json!({"id": id, "name": name, "prefix": prefix, "created_at": created_at})
        })
        .collect::<Vec<_>>())
    .into_response()
}

#[derive(Deserialize)]
struct HealthReq {
    health: String,
}

async fn admin_set_health(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<HealthReq>,
) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let Some(h) = parse_health(&body.health) else {
        return err(StatusCode::BAD_REQUEST, "health must be warmup|active|quarantine");
    };
    if let Err(e) = sqlx::query("UPDATE ips SET health=$2 WHERE id=$1")
        .bind(&id)
        .bind(health_str(h))
        .execute(&app.pool)
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(serde_json::json!({"id": id, "health": health_str(h)})).into_response()
}

#[derive(Deserialize)]
struct MsgQuery {
    #[serde(default)]
    limit: Option<i64>,
}

async fn admin_messages(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<MsgQuery>,
) -> Response {
    if let Err(resp) = require_admin(&app, &headers) {
        return resp;
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows: Vec<(Uuid, String, String, String, Option<i32>, chrono::DateTime<Utc>)> =
        match sqlx::query_as(
            "SELECT id, status, recipient, stream, smtp_code, created_at FROM messages ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&app.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    Json(rows
        .into_iter()
        .map(|(id, status, recipient, stream, smtp_code, created_at)| {
            serde_json::json!({
                "id": id, "status": status, "recipient": recipient,
                "stream": stream, "smtp_code": smtp_code, "created_at": created_at
            })
        })
        .collect::<Vec<_>>())
    .into_response()
}

async fn unsub_get(State(app): State<Arc<App>>, Path(token): Path<String>) -> Response {
    match decode_unsub(&app.cfg, &token) {
        Some(email) => Html(format!(
            "<!doctype html><meta charset=utf-8><title>Unsubscribe</title><body style='font-family:serif;background:#111;color:#eee;padding:4rem'><h1>Stop mail to {email}?</h1><form method=post><button>Unsubscribe</button></form>"
        ))
        .into_response(),
        None => err(StatusCode::BAD_REQUEST, "bad token"),
    }
}

async fn unsub_post(State(app): State<Arc<App>>, Path(token): Path<String>) -> Response {
    let Some(email) = decode_unsub(&app.cfg, &token) else {
        return err(StatusCode::BAD_REQUEST, "bad token");
    };
    let _ = sqlx::query(
        "INSERT INTO suppressions (email, reason) VALUES ($1,'unsubscribe') ON CONFLICT DO NOTHING",
    )
    .bind(&email)
    .execute(&app.pool)
    .await;
    Html("<!doctype html><meta charset=utf-8><body style='font-family:serif;background:#111;color:#eee;padding:4rem'><h1>You are unsubscribed.</h1>").into_response()
}

async fn mta_sts(State(app): State<Arc<App>>) -> impl IntoResponse {
    let body = format!(
        "version: STSv1\nmode: enforce\nmx: {}\nmax_age: 604800\n",
        app.cfg.mail_hostname
    );
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
}

async fn require_api(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return Err(err(StatusCode::UNAUTHORIZED, "missing bearer token"));
    };
    match api_key_ok(&app.pool, raw).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(err(StatusCode::UNAUTHORIZED, "bad api key")),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
    }
}

fn require_admin(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    let Some(h) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return Err(admin_challenge());
    };
    let Some(b64) = h.strip_prefix("Basic ") else {
        return Err(admin_challenge());
    };
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return Err(admin_challenge());
    };
    let Ok(pair) = String::from_utf8(bytes) else {
        return Err(admin_challenge());
    };
    let Some((user, pass)) = pair.split_once(':') else {
        return Err(admin_challenge());
    };
    if user == "admin" && pass == app.cfg.admin_password {
        Ok(())
    } else {
        Err(admin_challenge())
    }
}

fn admin_challenge() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"mta console\"")],
        "auth required",
    )
        .into_response()
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({"error": msg}))).into_response()
}

fn extract_email(from: &str) -> String {
    if let (Some(a), Some(b)) = (from.find('<'), from.find('>')) {
        from[a + 1..b].trim().to_ascii_lowercase()
    } else {
        from.trim().to_ascii_lowercase()
    }
}

fn unsub_url(cfg: &Config, email: &str) -> String {
    format!("{}/u/{}", cfg.api_public_url.trim_end_matches('/'), unsub_token(cfg, email))
}

fn unsub_token(cfg: &Config, email: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(cfg.admin_password.as_bytes()).expect("hmac");
    mac.update(email.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("{}.{}", hex::encode(email.as_bytes()), sig)
}

fn decode_unsub(cfg: &Config, token: &str) -> Option<String> {
    let (email_hex, sig) = token.split_once('.')?;
    let email = String::from_utf8(hex::decode(email_hex).ok()?).ok()?;
    let expect = unsub_token(cfg, &email);
    if expect == token && sig.len() == 64 {
        Some(email)
    } else {
        None
    }
}

#[allow(dead_code)]
fn _unused_eval(r: Reputation) -> crate::warming::Health {
    evaluate_health(&r)
}

#[allow(dead_code)]
fn _redirect() -> Redirect {
    Redirect::to("/console")
}

#[allow(dead_code)]
fn _body() -> Body {
    Body::empty()
}
