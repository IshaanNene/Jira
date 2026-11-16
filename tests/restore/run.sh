#!/usr/bin/env bash
# Restore drill: back up every service database, restore each backup into a fresh database,
# and check that what came back is the same data under the same protections:
#   * every table has exactly the rows it had (exact counts, not statistics),
#   * the same migrations are recorded as applied,
#   * the same tables are under forced row-level security, with the same policies,
#   * the runtime role, connected to the restored copy, sees no rows without a tenant and
#     exactly one workspace's rows with one.
# Reports the size of each backup and how long the backup and the restore took.
#
# This drills logical backups (pg_dump custom format), run against the compose PostgreSQL
# (make infra; data from make seed or make load). Production relies on RDS automated backups
# with point-in-time recovery (infrastructure/terraform/modules/postgres); restoring one of
# those is the same check against a restored instance and has not been drilled.
#   tests/restore/run.sh [service ...]   (default: every service database that has tables)
set -uo pipefail
cd "$(dirname "$0")/../.."
failures=0; DRILLS=""

psql_() { docker compose exec -T -e PGOPTIONS='-c client_min_messages=warning' postgres psql -v ON_ERROR_STOP=1 -U postgres -tA "$@"; }
cleanup() {
  for db in $DRILLS; do psql_ -d postgres -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" > /dev/null 2>&1; done
  docker compose exec -T postgres sh -c 'rm -rf /tmp/restore-drill' > /dev/null 2>&1
}
trap cleanup EXIT
pass() { printf '  \033[32mPASS\033[0m %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; failures=$((failures + 1)); }
# expect <what> <got> <wanted>: an empty answer is a failure, never a match.
expect() { if [ -n "$3" ] && [ "$2" = "$3" ]; then pass "$1"; else fail "$1"; diff <(echo "$3") <(echo "$2") | head -10 | sed 's/^/        /'; fi; }
now() { python3 -c 'import time; print(time.time())'; }
since() { python3 -c "import sys; print(f'{float(sys.argv[2]) - float(sys.argv[1]):.1f}')" "$1" "$(now)"; }

psql_ -d postgres -c 'SELECT 1' > /dev/null 2>&1 || { echo "PostgreSQL is not running: make infra first" >&2; exit 2; }

SERVICES=("$@")
if [ ${#SERVICES[@]} -eq 0 ]; then
  for db in $(psql_ -d postgres -c "SELECT datname FROM pg_database WHERE datname LIKE '%\_db' ORDER BY 1"); do
    [ "$(psql_ -d "$db" -c "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")" -gt 0 ] && SERVICES+=("${db%_db}")
  done
fi

# Exact row count of every table, one "table count" line each.
counts() {
  psql_ -d "$1" -c "SELECT format('SELECT %L || '' '' || count(*) FROM public.%I;', tablename, tablename) FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename" \
    | psql_ -d "$1"
}
migrations() { psql_ -d "$1" -c "SELECT string_agg(version::text, ',' ORDER BY version) FROM _sqlx_migrations WHERE success"; }
# Prefixed with the count, so a database without row-level security still answers.
protections() {
  echo "tables with row-level security: $(psql_ -d "$1" -c "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relrowsecurity")"
  psql_ -d "$1" -c "SELECT c.relname || ' forced=' || c.relforcerowsecurity || ' ' || coalesce(string_agg(p.polname || ':' || pg_get_expr(p.polqual, p.polrelid), ';' ORDER BY p.polname), '')
                    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace LEFT JOIN pg_policy p ON p.polrelid = c.oid
                    WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relrowsecurity GROUP BY c.relname, c.relforcerowsecurity ORDER BY 1"
}

started_all=$(now); total_bytes=0
docker compose exec -T postgres mkdir -p /tmp/restore-drill
for svc in "${SERVICES[@]}"; do
  source="${svc}_db"; copy="${svc}_drill"; dump="/tmp/restore-drill/$svc.dump"
  echo "$svc"
  t=$(now)
  docker compose exec -T postgres pg_dump -U postgres -Fc -f "$dump" "$source" || { fail "backup of $source"; continue; }
  dumped=$(since "$t")
  bytes=$(docker compose exec -T postgres stat -c %s "$dump"); total_bytes=$((total_bytes + bytes))

  DRILLS="$DRILLS $copy"
  psql_ -d postgres -c "DROP DATABASE IF EXISTS $copy WITH (FORCE)" -c "CREATE DATABASE $copy OWNER ${svc}_owner" \
    -c "GRANT CONNECT ON DATABASE $copy TO ${svc}_app" > /dev/null
  t=$(now)
  docker compose exec -T postgres pg_restore -U postgres --exit-on-error -d "$copy" "$dump" || { fail "restore of $source"; continue; }
  restored=$(since "$t")
  printf '  backup %s KiB in %ss, restore in %ss\n' "$((bytes / 1024))" "$dumped" "$restored"

  expect "every table has the same rows" "$(counts "$copy")" "$(counts "$source")"
  expect "the same migrations are applied" "$(migrations "$copy")" "$(migrations "$source")"
  expect "the same tables are under forced row-level security, with the same policies" "$(protections "$copy")" "$(protections "$source")"
  unforced=$(psql_ -d "$copy" -c "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relrowsecurity AND NOT c.relforcerowsecurity")
  expect "no restored table has row-level security without FORCE" "$unforced" 0

  # The runtime role on the restored copy: nothing without a tenant, one workspace's rows with one.
  table=$(psql_ -d "$copy" -c "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id'
                               WHERE n.nspname = 'public' AND c.relkind = 'r' AND c.relforcerowsecurity ORDER BY c.reltuples DESC, c.relname LIMIT 1")
  if [ -n "$table" ]; then
    tenant=$(psql_ -d "$copy" -c "SELECT tenant_id FROM $table GROUP BY 1 ORDER BY count(*) DESC LIMIT 1")
    app() { docker compose exec -T -e PGPASSWORD="${svc}_app" postgres psql -v ON_ERROR_STOP=1 -h 127.0.0.1 -U "${svc}_app" -d "$copy" -tAq "$@"; }
    expect "the runtime role sees no $table rows without a tenant" "$(app -c "SELECT count(*) FROM $table")" 0
    if [ -n "$tenant" ]; then
      expect "the runtime role sees exactly one workspace's $table rows with its tenant" \
        "$(app -c "SELECT set_config('app.tenant_id', '$tenant', false)" -c "SELECT count(*) FROM $table" | tail -1)" \
        "$(psql_ -d "$copy" -c "SELECT count(*) FROM $table WHERE tenant_id = '$tenant'")"
    fi
  fi
  psql_ -d postgres -c "DROP DATABASE $copy WITH (FORCE)" > /dev/null
  docker compose exec -T postgres rm -f "$dump"
done

printf '\n%s databases, %s KiB of backups, backed up and restored in %ss\n' "${#SERVICES[@]}" "$((total_bytes / 1024))" "$(since "$started_all")"
[ "$failures" -eq 0 ] && echo "restore drill passed" || { echo "$failures check(s) failed"; exit 1; }
