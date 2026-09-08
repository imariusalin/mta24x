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

echo "install helper tests ok"
