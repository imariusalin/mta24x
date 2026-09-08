#!/usr/bin/env bash
# Smoke-test installer helpers without touching Docker or apt.
set -euo pipefail
cd "$(dirname "$0")/.."
export MTA_INSTALL_LIB=1
# shellcheck disable=SC1091
source ./install.sh

valid_ipv4 "203.0.113.10" || { echo "valid_ipv4 rejected a good address"; exit 1; }
valid_ipv4 "999.1.1.1" && { echo "valid_ipv4 accepted a bad address"; exit 1; }
valid_ipv4 "not-an-ip" && { echo "valid_ipv4 accepted text"; exit 1; }

s="$(rand_secret)"
[[ ${#s} -ge 16 ]] || { echo "rand_secret too short"; exit 1; }

is_public_v4 "203.0.113.10" || { echo "public IP rejected"; exit 1; }
is_public_v4 "10.0.0.1" && { echo "private 10/8 accepted"; exit 1; }
is_public_v4 "192.168.1.1" && { echo "private 192.168 accepted"; exit 1; }
is_public_v4 "172.16.0.1" && { echo "private 172.16 accepted"; exit 1; }

[[ "$(join_pools "1.1.1.1" "" "2.2.2.2")" == "1.1.1.1,2.2.2.2" ]] || { echo "join_pools failed"; exit 1; }

# Caddy must stay on the compose bridge (so it can bind :80/:443). The
# engine stays host-network for SMTP source IPs. They meet on a unix
# socket — not host.docker.internal (UFW 502) and not Caddy host-net
# (cannot bind privileged ports → connection refused).
grep -q 'unix//sock/http.sock' deploy/caddy/Caddyfile \
  || { echo "Caddy must proxy the engine over unix//sock/http.sock"; exit 1; }
grep -q 'webmail:3000' deploy/caddy/Caddyfile \
  || { echo "Caddy must proxy webmail on the compose network"; exit 1; }
grep -q 'host.docker.internal:8787' deploy/caddy/Caddyfile \
  && { echo "Caddy must not reach the engine via host.docker.internal"; exit 1; }
grep -q '127.0.0.1:8787' deploy/caddy/Caddyfile \
  && { echo "Caddy must not assume host-network loopback for the engine"; exit 1; }
awk '
  $1=="caddy:" { in_caddy=1; next }
  in_caddy && /^  [a-z]/ { in_caddy=0 }
  in_caddy && /network_mode: host/ { print "caddy must not use network_mode: host"; exit 1 }
' docker-compose.yml
grep -q '"80:80"' docker-compose.yml \
  || { echo "caddy must publish host port 80"; exit 1; }
grep -q 'engine-proxy:' docker-compose.yml \
  || { echo "engine-proxy sidecar is required"; exit 1; }
grep -q 'alpine/socat' docker-compose.yml \
  || { echo "engine-proxy must use socat"; exit 1; }
# caddy:2-alpine rejects dial_timeout as a reverse_proxy subdirective
# and also fails to parse a nested transport block in this handle.
grep -q 'dial_timeout' deploy/caddy/Caddyfile \
  && { echo "Caddyfile must not contain dial_timeout"; exit 1; }
grep -q 'transport http' deploy/caddy/Caddyfile \
  && { echo "Caddyfile must not use transport http in reverse_proxy"; exit 1; }

echo "install helper tests ok"
