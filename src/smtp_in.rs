use crate::bounce::parse_verp_local_part;
use crate::config::Config;
use crate::queue::{enqueue_raw, record_verp_bounce};
use crate::routing::Stream;
use anyhow::Result;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};
use uuid::Uuid;

pub async fn serve(cfg: Config, pool: PgPool) -> Result<()> {
    let listener = TcpListener::bind(cfg.smtp_in_bind).await?;
    info!(addr = %cfg.smtp_in_bind, "smtp intake listening");
    loop {
        let (socket, peer) = listener.accept().await?;
        let cfg = cfg.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(socket, &cfg, &pool).await {
                warn!(peer = %peer, error = %e, "smtp session failed");
            }
        });
    }
}

async fn handle(stream: TcpStream, cfg: &Config, pool: &PgPool) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    writer
        .write_all(format!("220 {} ESMTP mta\r\n", cfg.mail_hostname).as_bytes())
        .await?;

    let mut mail_from = String::new();
    let mut rcpt: Vec<String> = Vec::new();
    let mut data_mode = false;
    let mut data = Vec::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        if data_mode {
            if line == ".\r\n" || line == ".\n" {
                data_mode = false;
                for to in &rcpt {
                    if let Some(id) = bounce_id(to) {
                        record_verp_bounce(pool, id, &data).await.ok();
                        continue;
                    }
                    match enqueue_raw(pool, cfg, Stream::Mailbox, &mail_from, to, &data).await {
                        Ok(id) => {
                            writer
                                .write_all(format!("250 2.0.0 queued {id}\r\n").as_bytes())
                                .await?;
                        }
                        Err(e) => {
                            writer
                                .write_all(format!("451 4.3.0 {e}\r\n").as_bytes())
                                .await?;
                        }
                    }
                }
                if rcpt.is_empty() {
                    writer.write_all(b"250 2.0.0 ok\r\n").await?;
                }
                mail_from.clear();
                rcpt.clear();
                data.clear();
                continue;
            }
            let bytes = if let Some(rest) = line.strip_prefix('.') {
                rest.as_bytes()
            } else {
                line.as_bytes()
            };
            data.extend_from_slice(bytes);
            continue;
        }

        let cmd = line.trim_end_matches(['\r', '\n']);
        let upper = cmd.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer
                .write_all(
                    format!(
                        "250-{}\r\n250-PIPELINING\r\n250-8BITMIME\r\n250 SIZE 26214400\r\n",
                        cfg.mail_hostname
                    )
                    .as_bytes(),
                )
                .await?;
        } else if upper.starts_with("MAIL FROM:") {
            mail_from = extract_addr(cmd).unwrap_or_default();
            writer.write_all(b"250 2.1.0 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if let Some(addr) = extract_addr(cmd) {
                rcpt.push(addr);
                writer.write_all(b"250 2.1.5 ok\r\n").await?;
            } else {
                writer.write_all(b"501 5.1.3 bad address\r\n").await?;
            }
        } else if upper == "DATA" {
            writer.write_all(b"354 go ahead\r\n").await?;
            data_mode = true;
        } else if upper == "RSET" {
            mail_from.clear();
            rcpt.clear();
            writer.write_all(b"250 2.0.0 ok\r\n").await?;
        } else if upper == "NOOP" {
            writer.write_all(b"250 2.0.0 ok\r\n").await?;
        } else if upper == "QUIT" {
            writer.write_all(b"221 2.0.0 bye\r\n").await?;
            break;
        } else {
            writer.write_all(b"502 5.5.1 command unrecognized\r\n").await?;
        }
    }
    Ok(())
}

fn extract_addr(cmd: &str) -> Option<String> {
    let start = cmd.find('<')?;
    let end = cmd.find('>')?;
    if end <= start {
        return None;
    }
    let addr = cmd[start + 1..end].trim();
    if addr.is_empty() || !addr.contains('@') {
        None
    } else {
        Some(addr.to_ascii_lowercase())
    }
}

fn bounce_id(rcpt: &str) -> Option<Uuid> {
    let (local, domain) = rcpt.split_once('@')?;
    if !domain.starts_with("bounces.") {
        return None;
    }
    let id = parse_verp_local_part(local)?;
    Uuid::parse_str(id).ok()
}
