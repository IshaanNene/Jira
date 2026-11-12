#!/usr/bin/env bash
# Stop host processes started by start.sh. Infrastructure containers keep running
# (use `make clean` to remove them and their data).
set -uo pipefail
. "$(dirname "$0")/services.sh"
quiet="${1:-}"
if [ -d "$RUN_DIR/pids" ]; then
  for pidfile in "$RUN_DIR"/pids/*.pid; do
    [ -e "$pidfile" ] || continue
    pid="$(cat "$pidfile")"
    if kill -0 "$pid" 2>/dev/null; then
      # Kill the process group member and any children (pnpm spawns vite).
      pkill -TERM -P "$pid" 2>/dev/null || true
      kill -TERM "$pid" 2>/dev/null || true
      [ "$quiet" = "--quiet" ] || echo "stopped $(basename "$pidfile" .pid) ($pid)"
    fi
    rm -f "$pidfile"
  done
fi
exit 0
