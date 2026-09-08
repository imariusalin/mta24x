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

# Caddy is on the Docker bridge; the engine is host-network. Proxying
# through host.docker.internal:8787 502s when UFW drops docker→host.
# Console/API must go via host loopback, which means Caddy itself is host-net.
grep -q 'reverse_proxy 127.0.0.1:8787' deploy/caddy/Caddyfile \
  || { echo "Caddy must proxy the engine on 127.0.0.1:8787"; exit 1; }
grep -q 'host.docker.internal:8787' deploy/caddy/Caddyfile \
  && { echo "Caddy must not reach the engine via host.docker.internal"; exit 1; }
awk '
  $1=="caddy:" { in_caddy=1; next }
  in_caddy && /^  [a-z]/ { in_caddy=0 }
  in_caddy && /network_mode: host/ { found=1 }
  END { if (!found) { print "caddy service must use network_mode: host"; exit 1 } }
' docker-compose.yml
grep -q '127.0.0.1:3000:3000' docker-compose.yml \
  || { echo "webmail must publish 127.0.0.1:3000 for host-network Caddy"; exit 1; }

echo "install helper tests ok"
