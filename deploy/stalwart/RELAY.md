# Point Stalwart outbound at the engine

Stalwart owns IMAP, JMAP, inbound MX, and human submission (587/465).
It must **not** deliver to Gmail itself.

After `docker compose up`, open Stalwart admin at `http://VPS:8080`
(user `admin`, password `STALWART_ADMIN_PASSWORD`).

1. Create the mailbox domain (`example.com`) and users as usual.
2. Settings → MTA → Outbound → Routes → add a **Relay**:
   - name: `engine`
   - address: `host.docker.internal`
   - port: `2525`
   - protocol: `smtp`
   - implicit TLS: off
   - allow invalid certs: on (local hop)
3. Outbound strategy route expression: remote recipients use `engine`.
   Local domains (`example.com`) stay **Local**.
4. Optional: route `bounce+*@bounces.example.com` to the engine too,
   so async DSNs update suppression.

Inbound ports 25/465/587/143/993 stay on Stalwart.
Webmail (Bulwark) talks JMAP to Stalwart on port 8080.
