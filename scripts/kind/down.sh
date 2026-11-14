#!/usr/bin/env bash
# Delete the local cluster. The data stores (docker compose) are left as they are.
set -euo pipefail
cd "$(dirname "$0")/../.."
kind delete cluster --name meridian
for svc in postgres redis kafka opensearch clickhouse object-storage mailpit; do
  container="$(docker compose ps -q "$svc" 2>/dev/null || true)"
  [ -n "$container" ] && docker network disconnect kind "$container" 2>/dev/null || true
done
