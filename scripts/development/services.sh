#!/usr/bin/env bash
# Single source of truth for which services exist locally and where they listen.
# Format: name:http_port:grpc_port   (grpc_port "-" when the service has none)
SERVICES=(
  "tenant:8102:9102"
  "identity:8101:-"
  "user:8103:9103"
  "permission:8108:9108"
  "project:8104:9104"
  "workflow:8106:9106"
  "issue:8105:9105"
  "agile:8107:-"
  "realtime:8109:-"
  "notification:8110:-"
  "search:8111:-"
)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUN_DIR="$ROOT/.run"

load_env() {
  [ -f "$ROOT/.env" ] || cp "$ROOT/.env.example" "$ROOT/.env"
  set -a; . "$ROOT/.env"; set +a
}

# Per-service database credentials follow the convention in infrastructure/postgres/init.
service_env() {
  local name="$1" http="$2" grpc="$3"
  export HTTP_ADDR="127.0.0.1:${http}"
  if [ "$grpc" != "-" ]; then export GRPC_ADDR="127.0.0.1:${grpc}"; else unset GRPC_ADDR; fi
  export DATABASE_URL="postgres://${name}_app:${name}_app@${POSTGRES_HOST}:${POSTGRES_PORT}/${name}_db"
  export DATABASE_MIGRATION_URL="postgres://${name}_owner:${name}_owner@${POSTGRES_HOST}:${POSTGRES_PORT}/${name}_db"
}
