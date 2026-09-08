# Dispatch — company MTA

[![Rust](https://github.com/imariusalin/mta24x/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/imariusalin/mta24x/actions/workflows/rust.yml)

Single-tenant outbound engine with **IP + domain warming**, isolated pools, DKIM/SPF/DMARC/MTA-STS, a transactional HTTP API, and SMTP intake. **Stalwart** is IMAP/JMAP/inbound. **Bulwark** is webmail.

Designed for **5k–50k mail/day** on a Linux VPS with **any number of dedicated IPs** (1, 3, or 200+). The installer detects host IPv4s and the engine splits them into pools.

| Host IPs | Split |
|---|---|
| 1 | Shared (transactional + marketing + mailbox) |
| 2 | 1 transactional, 1 marketing |
| 3 | 1 transactional, 1 marketing, 1 canary |
| 4+ | ~20% transactional, ~10% canary, rest marketing |

Quarantine is a **state**. Marketing never shares a transactional IP when another pool exists. Extra IPs in a pool are load-balanced (least sent today).

## What you get

- Warmup curves per ISP (Gmail, Microsoft, Yahoo, Apple, other)
- Auto volume: **stop / cut 25% / slow 50% / hold / push +20%** from live bounce, deferral, complaint, block, DNSBL
- Reputation score 0–100 per IP in the console (Spamhaus, SpamCop, Barracuda, SORBS every 6h)
- Auto graduate after 14 clean days; auto quarantine on bounce/complaint/block
- VERP return-path + suppression list
- One-click `List-Unsubscribe` on marketing
- DNS wizard + PTR checklist in the console
- `POST /v1/messages` transactional API

## One-click install (Ubuntu 24.04 / Debian 12)

On a fresh VPS (any number of public IPs; rDNS set at the provider).

The repo is **private**, so raw `curl | bash` returns 404. Clone with your GitHub login (or a PAT), then install:

```bash
git clone https://github.com/imariusalin/mta24x.git
cd mta24x
sudo ./install.sh
```

SSH instead of HTTPS:

```bash
git clone git@github.com:imariusalin/mta24x.git
cd mta24x
sudo ./install.sh
```

If you make the repository **public**, this one-liner works (it detects IPs; it still asks for the domain unless you pass env vars):

```bash
curl -fsSL https://raw.githubusercontent.com/imariusalin/mta24x/main/install.sh | sudo bash
```

If that clone already finished (Docker installed, then stopped), resume:

```bash
cd /opt/mta24x && sudo git pull && sudo ./install.sh
```

Unattended (detects every public IPv4 on the box):

```bash
# only ROOT_DOMAIN is required
sudo ROOT_DOMAIN=example.com ACME_EMAIL=you@example.com ./install.sh --non-interactive
# or:
sudo ./install.sh --non-interactive --env deploy/install.env.example
# fleet:
./deploy/rollout.sh deploy/inventory.example
```

Go live after DNS/PTR/DKIM match:

```bash
sudo ./install.sh --go-live
```

The script installs Docker, writes `/opt/mta24x/.env` (secrets generated), opens ufw ports, and `docker compose up`. Passwords stay in `.env` (mode 600).

After install:

- Console: `https://mail.example.com/console` (user `admin`, password `ADMIN_PASSWORD` in `.env`)
- Webmail: `https://mail.example.com`
- Stalwart admin: `http://127.0.0.1:8080` on the VPS — set outbound relay (`deploy/stalwart/RELAY.md`)
- Publish the DNS records the console prints, then `sudo ./install.sh --go-live`
- Mint an API key in the console

```bash
curl -s https://mail.example.com/v1/messages \
  -H "Authorization: Bearer sk_live_..." \
  -H "Content-Type: application/json" \
  -d '{
    "from": "Billing <billing@notify.example.com>",
    "to": ["you@gmail.com"],
    "subject": "Invoice 1042",
    "text": "Pay when you can.",
    "html": "<p>Pay when you can.</p>",
    "stream": "transactional"
  }'
```

Marketing must use `"stream": "marketing"` and `news@news.example.com`.

## Local (no public IPs)

```bash
cp .env.example .env
# leave IPs at 127.0.0.x and DRY_RUN=true
docker compose up -d postgres
cargo run
```

Console: http://127.0.0.1:8787/console

`network_mode: host` on the engine **and Caddy** is **Linux** (needed so Caddy can reach the engine on `127.0.0.1:8787`; Docker-bridge → host:8787 is a UFW 502). On Docker Desktop, run the engine with `cargo run` instead.

## CI

GitHub Actions on every push and PR to `main`. The badge at the top is the **last build**.

| Order | Step | Command |
|---|---|---|
| 1 | 📦 Checkout | `actions/checkout@v4` |
| 2 | 🔨 Build | `cargo build --verbose` |
| 3 | 🧪 Test | `cargo test --verbose` |

✅ green = last build passed · ❌ red = last build failed · 🟡 yellow = running

Workflow: [`.github/workflows/rust.yml`](.github/workflows/rust.yml)

## DMARC path

`p=none` (wizard default) → watch reports a week → `quarantine` → `reject`. Do not jump to reject on day one.

## Ports

| Port | Service |
|---|---|
| 25/465/587 | Stalwart SMTP |
| 143/993 | Stalwart IMAP |
| 80/443 | Caddy (webmail + console + API) |
| 2525 | Engine SMTP intake (localhost) |
| 8787 | Engine HTTP (localhost / Caddy) |
| 8080 | Stalwart admin (localhost) |
