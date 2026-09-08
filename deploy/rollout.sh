#!/usr/bin/env bash
# Scale: install or update the MTA on many VPS hosts.
#
# inventory.txt — one host per line:
#   root@203.0.113.10  nodes/mail1.env
#   root@203.0.113.20  nodes/mail2.env
#
#   ./deploy/rollout.sh inventory.txt
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INV="${1:-}"
[[ -n "$INV" && -f "$INV" ]] || {
  echo "usage: $0 inventory.txt" >&2
  exit 1
}

REMOTE_DIR="${REMOTE_DIR:-/opt/mta24x}"
SSH_OPTS="${SSH_OPTS:--o StrictHostKeyChecking=accept-new}"

while read -r host envfile extra; do
  [[ -z "${host:-}" || "$host" =~ ^# ]] && continue
  [[ -f "$envfile" ]] || { echo "missing env $envfile for $host" >&2; exit 1; }
  echo "==> $host  ($envfile)"
  ssh $SSH_OPTS "$host" "mkdir -p $REMOTE_DIR"
  rsync -az --delete \
    --exclude '.git' --exclude 'target' --exclude 'data' --exclude '.env' \
    "$ROOT/" "$host:$REMOTE_DIR/"
  scp $SSH_OPTS "$envfile" "$host:$REMOTE_DIR/.install.env"
  ssh $SSH_OPTS "$host" "chmod +x $REMOTE_DIR/install.sh; cd $REMOTE_DIR && sudo ./install.sh --non-interactive --env $REMOTE_DIR/.install.env --dir $REMOTE_DIR $extra"
  echo "==> $host done"
done < "$INV"
