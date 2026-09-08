#!/usr/bin/env bash
# One-click installer for the company MTA (Ubuntu 24.04 / Debian 12).
# Interactive on a single VPS, or non-interactive for fleet rollouts.
#
#   curl -fsSL https://raw.githubusercontent.com/imariusalin/mta24x/main/install.sh | sudo bash
#   sudo ./install.sh
#   sudo ./install.sh --non-interactive --env /root/mta.env
#   ./deploy/rollout.sh inventory.txt
set -euo pipefail

REPO_URL="${REPO_URL:-https://github.com/imariusalin/mta24x.git}"
INSTALL_DIR="${INSTALL_DIR:-/opt/mta24x}"
NONINTERACTIVE=0
FORCE_ENV=0
SKIP_DOCKER=0
SKIP_FIREWALL=0
SKIP_COMPOSE=0
GO_LIVE=0
ENV_FILE=""
CLONE=0

red() { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[33m%s\033[0m\n' "$*"; }
die() { red "error: $*"; exit 1; }

usage() {
  cat <<'EOF'
Usage: install.sh [options]

  --non-interactive     Require env vars / --env; do not prompt
  --env FILE            Read ROOT_DOMAIN, IPs, passwords from FILE
  --dir DIR             Install path (default /opt/mta24x)
  --clone               git clone the repo if docker-compose.yml is missing
  --force-env           Overwrite an existing .env
  --skip-docker         Do not apt-install Docker
  --skip-firewall       Do not configure ufw
  --skip-compose        Stop after writing .env
  --go-live             Set DRY_RUN=false after a healthy start
  -h, --help            Show this help

Fleet (non-interactive) minimum env:
  ROOT_DOMAIN  IP_TX  IP_MKT  IP_CANARY  ACME_EMAIL
Passwords are generated if omitted.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --non-interactive) NONINTERACTIVE=1; shift ;;
    --env) ENV_FILE="${2:-}"; shift 2 ;;
    --dir) INSTALL_DIR="${2:-}"; shift 2 ;;
    --clone) CLONE=1; shift ;;
    --force-env) FORCE_ENV=1; shift ;;
    --skip-docker) SKIP_DOCKER=1; shift ;;
    --skip-firewall) SKIP_FIREWALL=1; shift ;;
    --skip-compose) SKIP_COMPOSE=1; shift ;;
    --go-live) GO_LIVE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown flag $1" ;;
  esac
done

need_root() {
  [[ "$(id -u)" -eq 0 ]] || die "run as root (sudo ./install.sh)"
}

need_linux() {
  [[ "$(uname -s)" == "Linux" ]] || die "this installer is for Linux VPS only (Ubuntu 24.04 / Debian 12)"
}

load_env_file() {
  local f="$1"
  [[ -f "$f" ]] || die "env file not found: $f"
  set -a
  # shellcheck disable=SC1090
  source "$f"
  set +a
}

prompt() {
  local var="$1" label="$2" def="${3:-}"
  local cur="${!var:-}"
  local input
  if [[ -n "$cur" ]]; then
    return 0
  fi
  if [[ "$NONINTERACTIVE" -eq 1 ]]; then
    [[ -n "$def" ]] && printf -v "$var" '%s' "$def" && return 0
    die "missing $var (set it in --env or the environment)"
  fi
  # curl | bash leaves stdin as the script pipe. Always ask on the real TTY.
  if [[ -n "$def" ]]; then
    input="$label [$def]: "
  else
    input="$label: "
  fi
  if [[ -e /dev/tty ]]; then
    read -r -p "$input" cur </dev/tty || die "could not read $var from the terminal"
  else
    die "no terminal for prompts. Re-run: sudo $INSTALL_DIR/install.sh   or pass --non-interactive --env FILE"
  fi
  if [[ -z "$cur" ]]; then
    cur="$def"
  fi
  [[ -n "$cur" ]] || die "$var is required"
  printf -v "$var" '%s' "$cur"
}

rand_secret() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -base64 32 | tr -d '\n='
  else
    head -c 32 /dev/urandom | base64 | tr -d '\n='
  fi
}

valid_ipv4() {
  local ip="$1"
  [[ "$ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || return 1
  local IFS=.
  # shellcheck disable=SC2086
  set -- $ip
  [[ $1 -le 255 && $2 -le 255 && $3 -le 255 && $4 -le 255 ]]
}

ip_on_host() {
  ip -4 -o addr show | awk '{print $4}' | cut -d/ -f1 | grep -qx "$1"
}

ensure_repo() {
  if [[ -f docker-compose.yml && -f Dockerfile ]]; then
    INSTALL_DIR="$(pwd)"
    return 0
  fi
  if [[ -f "$INSTALL_DIR/docker-compose.yml" ]]; then
    cd "$INSTALL_DIR"
    return 0
  fi
  [[ "$CLONE" -eq 1 || "$NONINTERACTIVE" -eq 1 || ! -t 0 ]] || {
    yellow "repo files not in $(pwd); cloning to $INSTALL_DIR"
  }
  command -v git >/dev/null 2>&1 || { apt-get update -qq; apt-get install -y -qq git; }
  mkdir -p "$(dirname "$INSTALL_DIR")"
  if [[ -d "$INSTALL_DIR/.git" ]]; then
    git -C "$INSTALL_DIR" pull --ff-only || true
  else
    git clone "$REPO_URL" "$INSTALL_DIR"
  fi
  cd "$INSTALL_DIR"
  [[ -f docker-compose.yml ]] || die "clone did not contain docker-compose.yml"
}

install_docker() {
  if command -v docker >/dev/null 2>&1; then
    docker compose version >/dev/null 2>&1 || docker-compose version >/dev/null 2>&1 \
      || die "Docker is installed but Compose is missing"
    green "docker already present"
    return 0
  fi
  [[ "$SKIP_DOCKER" -eq 1 ]] && die "docker not found and --skip-docker was set"
  yellow "installing Docker Engine + Compose"
  apt-get update -qq
  apt-get install -y -qq ca-certificates curl gnupg
  install -m 0755 -d /etc/apt/keyrings
  if [[ ! -f /etc/apt/keyrings/docker.gpg ]]; then
    curl -fsSL https://download.docker.com/linux/$(. /etc/os-release && echo "$ID")/gpg \
      | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    chmod a+r /etc/apt/keyrings/docker.gpg
  fi
  # shellcheck disable=SC1091
  . /etc/os-release
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -qq
  apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-compose-plugin
  systemctl enable --now docker
}

compose() {
  if docker compose version >/dev/null 2>&1; then
    docker compose "$@"
  else
    docker-compose "$@"
  fi
}

write_env() {
  local dest="$INSTALL_DIR/.env"
  if [[ -f "$dest" && "$FORCE_ENV" -eq 0 ]]; then
    yellow ".env exists — keeping it (pass --force-env to replace)"
    # shellcheck disable=SC1090
    set -a; source "$dest"; set +a
    return 0
  fi

  ROOT_DOMAIN="${ROOT_DOMAIN:-}"
  IP_TX="${IP_TX:-}"
  IP_MKT="${IP_MKT:-}"
  IP_CANARY="${IP_CANARY:-}"
  ACME_EMAIL="${ACME_EMAIL:-}"
  DRY_RUN="${DRY_RUN:-true}"

  yellow "Enter domain and the 3 public IPs for this VPS."
  prompt ROOT_DOMAIN "Root domain (example.com)"
  prompt IP_TX "Transactional IP"
  prompt IP_MKT "Marketing IP"
  prompt IP_CANARY "Canary IP"
  prompt ACME_EMAIL "Let's Encrypt email"
  if [[ "$NONINTERACTIVE" -eq 0 ]]; then
    prompt DRY_RUN "Keep DRY_RUN until DNS/PTR are live? (true/false)" "true"
  fi

  valid_ipv4 "$IP_TX" || die "IP_TX is not an IPv4: $IP_TX"
  valid_ipv4 "$IP_MKT" || die "IP_MKT is not an IPv4: $IP_MKT"
  valid_ipv4 "$IP_CANARY" || die "IP_CANARY is not an IPv4: $IP_CANARY"
  [[ "$IP_TX" != "$IP_MKT" && "$IP_TX" != "$IP_CANARY" && "$IP_MKT" != "$IP_CANARY" ]] \
    || die "the three IPs must be distinct"

  for ip in "$IP_TX" "$IP_MKT" "$IP_CANARY"; do
    if ! ip_on_host "$ip"; then
      yellow "warning: $ip is not assigned to this host yet (PTR/NIC). Continuing."
    fi
  done

  MAIL_HOSTNAME="${MAIL_HOSTNAME:-mail.${ROOT_DOMAIN}}"
  API_PUBLIC_URL="${API_PUBLIC_URL:-https://${MAIL_HOSTNAME}}"
  IP_TX_EHLO="${IP_TX_EHLO:-$MAIL_HOSTNAME}"
  IP_MKT_EHLO="${IP_MKT_EHLO:-news-out.${ROOT_DOMAIN}}"
  IP_CANARY_EHLO="${IP_CANARY_EHLO:-out.${ROOT_DOMAIN}}"
  ADMIN_PASSWORD="${ADMIN_PASSWORD:-$(rand_secret)}"
  STALWART_ADMIN_PASSWORD="${STALWART_ADMIN_PASSWORD:-$(rand_secret)}"
  SESSION_SECRET="${SESSION_SECRET:-$(rand_secret)}"
  POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-$(rand_secret)}"
  HTTP_BIND="${HTTP_BIND:-0.0.0.0:8787}"
  SMTP_IN_BIND="${SMTP_IN_BIND:-0.0.0.0:2525}"
  WORKER_CONCURRENCY="${WORKER_CONCURRENCY:-16}"

  umask 077
  cat > "$dest" <<EOF
ROOT_DOMAIN=${ROOT_DOMAIN}
MAIL_HOSTNAME=${MAIL_HOSTNAME}
API_PUBLIC_URL=${API_PUBLIC_URL}
ADMIN_PASSWORD=${ADMIN_PASSWORD}
ACME_EMAIL=${ACME_EMAIL}
SESSION_SECRET=${SESSION_SECRET}
STALWART_ADMIN_PASSWORD=${STALWART_ADMIN_PASSWORD}
POSTGRES_PASSWORD=${POSTGRES_PASSWORD}
IP_TX=${IP_TX}
IP_TX_EHLO=${IP_TX_EHLO}
IP_MKT=${IP_MKT}
IP_MKT_EHLO=${IP_MKT_EHLO}
IP_CANARY=${IP_CANARY}
IP_CANARY_EHLO=${IP_CANARY_EHLO}
HTTP_BIND=${HTTP_BIND}
SMTP_IN_BIND=${SMTP_IN_BIND}
WORKER_CONCURRENCY=${WORKER_CONCURRENCY}
DRY_RUN=${DRY_RUN}
DATABASE_URL=postgres://mta:${POSTGRES_PASSWORD}@127.0.0.1:5432/mta
EOF
  chmod 600 "$dest"
  green "wrote $dest"
}

configure_firewall() {
  [[ "$SKIP_FIREWALL" -eq 1 ]] && return 0
  command -v ufw >/dev/null 2>&1 || apt-get install -y -qq ufw
  ufw allow OpenSSH >/dev/null 2>&1 || ufw allow 22/tcp >/dev/null
  for p in 25 80 443 465 587 143 993; do
    ufw allow "${p}/tcp" >/dev/null
  done
  ufw --force enable >/dev/null
  green "firewall: 22,25,80,443,465,587,143,993"
}

start_stack() {
  [[ "$SKIP_COMPOSE" -eq 1 ]] && return 0
  compose pull || true
  compose up -d --build
  yellow "waiting for engine /health …"
  local i
  for i in $(seq 1 60); do
    if curl -fsS http://127.0.0.1:8787/health >/dev/null 2>&1; then
      green "engine is up"
      return 0
    fi
    sleep 2
  done
  yellow "engine did not answer /health yet — check: docker compose logs engine"
}

maybe_go_live() {
  [[ "$GO_LIVE" -eq 1 ]] || return 0
  sed -i 's/^DRY_RUN=.*/DRY_RUN=false/' "$INSTALL_DIR/.env"
  compose up -d engine
  green "DRY_RUN=false — live sending enabled"
}

print_summary() {
  # shellcheck disable=SC1091
  set -a; source "$INSTALL_DIR/.env"; set +a
  cat <<EOF

$(green "MTA installed")
  dir:      $INSTALL_DIR
  domain:   $ROOT_DOMAIN
  console:  ${API_PUBLIC_URL}/console   (user admin)
  webmail:  ${API_PUBLIC_URL}
  stalwart: http://$(hostname -I | awk '{print $1}'):8080
  dry_run:  $DRY_RUN

  PTR at the VPS provider (required):
    $IP_TX      →  $IP_TX_EHLO
    $IP_MKT     →  $IP_MKT_EHLO
    $IP_CANARY  →  $IP_CANARY_EHLO

  Next:
    1. Open the console and publish the DNS wizard records
    2. Point Stalwart outbound relay to host.docker.internal:2525
       (see deploy/stalwart/RELAY.md)
    3. When SPF/DKIM/PTR match:  sudo $INSTALL_DIR/install.sh --go-live

  Passwords are in $INSTALL_DIR/.env (mode 600).

EOF
}

main() {
  need_linux
  need_root
  [[ -n "$ENV_FILE" ]] && load_env_file "$ENV_FILE"
  ensure_repo
  install_docker
  write_env
  configure_firewall
  start_stack
  maybe_go_live
  print_summary
}

# Allow tests to source helpers without running main.
if [[ "${MTA_INSTALL_LIB:-}" != "1" ]]; then
  main "$@"
fi
