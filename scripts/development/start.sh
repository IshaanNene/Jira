#!/usr/bin/env bash
# Start infrastructure, apply migrations, then run every service and the web app in the
# background. Logs: .run/logs/<name>.log   Stop: make stop
set -euo pipefail
. "$(dirname "$0")/services.sh"
load_env
cd "$ROOT"
mkdir -p "$RUN_DIR/logs" "$RUN_DIR/pids"

"$ROOT/scripts/development/stop.sh" --quiet || true

echo "==> starting infrastructure"
# object-storage-setup is a one-shot job, so it is started separately from the --wait set.
docker compose up -d --wait postgres redis object-storage gateway
docker compose up -d object-storage-setup

echo "==> building services"
cargo build --workspace --bins

"$ROOT/scripts/development/migrate.sh"

wait_healthy() {
  local name="$1" port="$2"
  for _ in $(seq 1 60); do
    if curl -fsS "http://127.0.0.1:${port}/health/ready" >/dev/null 2>&1; then return 0; fi
    if ! kill -0 "$(cat "$RUN_DIR/pids/${name}.pid")" 2>/dev/null; then
      echo "!! ${name}-service exited during startup. Last log lines:"; tail -n 20 "$RUN_DIR/logs/${name}.log"; return 1
    fi
    sleep 0.5
  done
  echo "!! ${name}-service did not become ready"; tail -n 20 "$RUN_DIR/logs/${name}.log"; return 1
}

for entry in "${SERVICES[@]}"; do
  IFS=: read -r name http grpc <<<"$entry"
  (
    service_env "$name" "$http" "$grpc"
    nohup "$ROOT/target/debug/${name}-service" >"$RUN_DIR/logs/${name}.log" 2>&1 </dev/null &
    echo $! >"$RUN_DIR/pids/${name}.pid"
  )
  wait_healthy "$name" "$http"
  echo "==> ${name}-service ready on :${http}"
done

if [ -d "$ROOT/apps/web/node_modules" ]; then
  # Redirect the whole subshell so nothing keeps the caller's stdout open.
  (
    cd "$ROOT/apps/web"
    nohup pnpm exec vite >"$RUN_DIR/logs/web.log" 2>&1 </dev/null &
    echo $! >"$RUN_DIR/pids/web.pid"
  ) >/dev/null 2>&1
  echo "==> web app starting on http://localhost:5173"
else
  echo "==> web dependencies not installed; run: make web-install"
fi

echo
echo "Gateway   http://localhost:8080"
echo "Web       http://localhost:5173"
echo "Logs      make logs"
