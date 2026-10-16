#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
config=${FOUNDATION_LOCAL_CONFIG:-env/local.secrets}
compose() {
  if [ -f "$config" ]; then
    docker compose --env-file "$config" "$@"
  else
    docker compose "$@"
  fi
}
case "${1:-}" in
  up) compose up --detach --wait ;;
  down) compose down ;;
  status) compose ps ;;
  provision) compose exec -T postgres psql -X -U admin -d foundation -f /provision/provision.sql ;;
  *) echo 'Usage: db.sh up|down|status|provision' >&2; exit 2 ;;
esac
