#!/usr/bin/env bash
# Failure tests: stop one data store at a time under the running stack (make dev + make seed)
# and check the behaviour documented in docs/SYSTEM_DESIGN.md §8, then that everything
# catches up once it is back. Every stopped container is started again, also on failure.
#   tests/resilience/run.sh [kafka|opensearch|clickhouse|redis|postgres ...]   (default: all)
set -uo pipefail
cd "$(dirname "$0")/../.."
API="${API:-http://localhost:8080/api/v1}"
PASSWORD="${SEED_PASSWORD:-demo-password-123}"
SCENARIOS=("$@"); [ ${#SCENARIOS[@]} -eq 0 ] && SCENARIOS=(kafka opensearch clickhouse redis postgres)
failures=0; STOPPED=""

trap '[ -n "$STOPPED" ] && docker compose start "$STOPPED" > /dev/null' EXIT
pass() { printf '  \033[32mPASS\033[0m %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; failures=$((failures + 1)); }
expect() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1 (expected $3, got $2)"; fi; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
stop() { STOPPED="$1"; docker compose stop "$1" > /dev/null; }
start() { docker compose start "$1" > /dev/null; STOPPED=""; }
# until <seconds> <command…>: retry until it succeeds or time runs out.
until_ok() { local deadline=$((SECONDS + $1)); shift; while ! "$@" > /dev/null 2>&1; do [ $SECONDS -ge $deadline ] && return 1; sleep 2; done; }

account=$(curl -fsS -H 'Content-Type: application/json' -X POST "$API/auth/login" -d "{\"email\":\"olivia@demo.example\",\"password\":\"$PASSWORD\"}" | json 'd["access_token"]') \
  || { echo "sign-in failed: run make dev and make seed first" >&2; exit 2; }
org=$(curl -fsS -H "Authorization: Bearer $account" "$API/organizations" | json '[o for o in d["items"] if o["slug"]=="demo-workspace"][0]["id"]')
TOKEN=$(curl -fsS -H "Authorization: Bearer $account" -H 'Content-Type: application/json' -X POST "$API/auth/token/exchange" -d "{\"organization_id\":\"$org\"}" | json 'd["access_token"]')
H=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 15 "${H[@]}" "$@"; }

# The scenarios write into their own project.
[ "$(code "$API/projects/RES")" = 404 ] && curl -fsS "${H[@]}" -X POST "$API/projects" \
  -d '{"name":"Resilience tests","key":"RES","project_type":"kanban","access":"open","description":"Issues created by tests/resilience."}' > /dev/null
RES_ID=$(curl -fsS "${H[@]}" "$API/projects/RES" | json 'd["id"]')
new_issue() { curl -fsS --max-time 15 "${H[@]}" -X POST "$API/issues" -d "{\"project_key\":\"RES\",\"type\":\"task\",\"summary\":\"$1\"}" | json 'd["key"]'; }
searchable() { curl -fsS "${H[@]}" "$API/search/issues?q=$1&project_id=$RES_ID" | json 'len(d["items"])' | grep -qx 1; }
charted() { curl -fsS "${H[@]}" -X POST "$API/analytics/query" -d "{\"kind\":\"breakdown\",\"projects\":[\"$RES_ID\"],\"group_by\":\"project\",\"include_done\":true}" | json 'd["total"]' | grep -qx "$1"; }
unpublished() { docker compose exec -T postgres psql -U postgres -d issue_db -tAc "SELECT count(*) FROM outbox WHERE published_at IS NULL"; }

for scenario in "${SCENARIOS[@]}"; do
  word="res$(date +%s)$RANDOM"
  echo "== $scenario down"
  case "$scenario" in
    kafka)
      stop kafka
      key=$(new_issue "Created while Kafka is down $word") && pass "issues are still created ($key)" || fail "issue creation failed with Kafka down"
      sleep 3
      [ "$(unpublished)" -gt 0 ] && pass "the event waits in the outbox" || fail "nothing waiting in the outbox"
      start kafka
      until_ok 120 bash -c "[ \"\$(docker compose exec -T postgres psql -U postgres -d issue_db -tAc 'SELECT count(*) FROM outbox WHERE published_at IS NULL')\" = 0 ]" \
        && pass "the outbox drains once Kafka is back" || fail "the outbox did not drain within 120 s"
      until_ok 60 searchable "$word" && pass "the issue reaches search" || fail "the issue never reached search"
      ;;
    opensearch)
      stop opensearch
      expect "search answers 503" "$(code "$API/search/issues?q=anything")" 503
      key=$(new_issue "Created while OpenSearch is down $word") && pass "issues are still created ($key)" || fail "issue creation failed with OpenSearch down"
      sleep 5
      start opensearch
      until_ok 120 searchable "$word" && pass "the issue changed during the outage is searchable afterwards" \
        || fail "the issue changed during the outage never reached search"
      ;;
    clickhouse)
      before=$(curl -fsS "${H[@]}" -X POST "$API/analytics/query" -d "{\"kind\":\"breakdown\",\"projects\":[\"$RES_ID\"],\"group_by\":\"project\",\"include_done\":true}" | json 'd["total"]')
      stop clickhouse
      expect "charts answer 503" "$(code -X POST "$API/analytics/query" -d '{"kind":"breakdown","group_by":"type"}')" 503
      expect "dashboards still list" "$(code "$API/dashboards")" 200
      expect "analytics reports not ready" "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8113/health/ready)" 503
      new_issue "Created while ClickHouse is down $word" > /dev/null && pass "issues are still created" || fail "issue creation failed with ClickHouse down"
      sleep 5
      start clickhouse
      until_ok 120 charted $((before + 1)) && pass "the issue created during the outage is counted afterwards" \
        || fail "the issue created during the outage was never counted"
      ;;
    redis)
      stop redis
      expect "reads still work (revocation checks fail open)" "$(code "$API/issues?project=DEMO&limit=5")" 200
      new_issue "Created while Redis is down $word" > /dev/null && pass "writes still work" || fail "writes failed with Redis down"
      start redis
      until_ok 30 bash -c "[ \"\$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer $TOKEN' '$API/issues?project=DEMO&limit=5')\" = 200 ]" && pass "recovered" || fail "not recovered after 30 s"
      ;;
    postgres)
      stop postgres
      started=$SECONDS
      headers=$(curl -s -D - -o /dev/null --max-time 20 "${H[@]}" "$API/issues?project=DEMO&limit=5")
      status=$(echo "$headers" | head -1 | awk '{print $2}')
      expect "reads answer 503" "$status" 503
      [ $((SECONDS - started)) -le 6 ] && pass "within $((SECONDS - started)) s (pool timeout 3 s)" || fail "took $((SECONDS - started)) s"
      echo "$headers" | grep -qi '^retry-after:' && pass "with Retry-After" || fail "no Retry-After header"
      expect "issue-service reports not ready" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 http://127.0.0.1:8105/health/ready)" 503
      start postgres
      until_ok 60 bash -c "[ \"\$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer $TOKEN' '$API/issues?project=DEMO&limit=5')\" = 200 ]" && pass "recovered" || fail "not recovered after 60 s"
      ;;
    *) echo "unknown scenario $scenario" >&2; exit 2 ;;
  esac
done

echo
[ $failures -eq 0 ] && echo "all checks passed" || echo "$failures check(s) failed"
exit $((failures > 0))
