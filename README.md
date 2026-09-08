# Dispatch — company MTA

[![Rust](https://github.com/imariusalin/mta24x/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/imariusalin/mta24x/actions/workflows/rust.yml)

Single-tenant outbound engine with **IP + domain warming**, isolated pools, DKIM/SPF/DMARC/MTA-STS, a transactional HTTP API, and SMTP intake. **Stalwart** is IMAP/JMAP/inbound. **Bulwark** is webmail.

Designed for **5k–50k mail/day** on a Linux VPS with **3 dedicated IPs**.

| IP | Role | From domains |
|---|---|---|
| `IP_TX` | transactional + people | `notify.example.com`, mailbox users |
| `IP_MKT` | marketing only | `news.example.com` |
| `IP_CANARY` | overflow / new domains | tests, future streams |

Quarantine is a **state**, not a fourth IP. Marketing never shares the transactional IP.

## What you get

- Warmup curves per ISP (Gmail, Microsoft, Yahoo, Apple, other)
- Auto graduate after 14 clean days; auto quarantine on bounce/complaint/block
- VERP return-path + suppression list
- One-click `List-Unsubscribe` on marketing
- DNS wizard + PTR checklist in the console
- `POST /v1/messages` transactional API

## VPS bootstrap

1. Three public IPv4s, rDNS set **at the provider**:

   ```
   IP_TX      PTR  mail.example.com
   IP_MKT     PTR  news-out.example.com
   IP_CANARY  PTR  out.example.com
   ```

2. Copy env and edit:

   ```bash
   cp .env.example .env
   # set ROOT_DOMAIN, the three IPs, ADMIN_PASSWORD, DRY_RUN=true
   ```

3. `docker compose up -d --build`

4. Open:
   - Console: `https://mail.example.com/console` (user `admin`)
   - Webmail: `https://mail.example.com`
   - Stalwart admin: `http://VPS:8080`
   - Follow `deploy/stalwart/RELAY.md` so Stalwart relays outbound to `:2525`

5. Publish the DNS records the console prints. Keep `DRY_RUN=true` until SPF/DKIM/PTR match. Then `DRY_RUN=false` and recreate the engine container.

6. Mint an API key in the console.

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

`network_mode: host` on the engine is **Linux**. On Docker Desktop, run the engine with `cargo run` instead.

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
