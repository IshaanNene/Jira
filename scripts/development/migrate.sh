#!/usr/bin/env bash
# Apply every service's migrations as that service's owner role.
set -euo pipefail
. "$(dirname "$0")/services.sh"
load_env
cd "$ROOT"
for entry in "${SERVICES[@]}"; do
  IFS=: read -r name http grpc <<<"$entry"
  service_env "$name" "$http" "$grpc"
  echo "==> migrating ${name}_db"
  cargo run --quiet -p "${name}-service" -- migrate
done
