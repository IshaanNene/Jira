#!/usr/bin/env bash
# Create demo data through the public API (never by writing to databases directly).
# Idempotent: re-running reuses the accounts and skips what already exists.
# Requires the stack to be running (`make dev`).
set -euo pipefail
API="${API:-http://localhost:8080/api/v1}"
ORIGIN="${ORIGIN:-http://localhost:5173}"
# Development-only demo credentials; printed at the end so you can sign in.
PASSWORD="${SEED_PASSWORD:-demo-password-123}"
JAR_DIR="$(mktemp -d)"; trap 'rm -rf "$JAR_DIR"' EXIT

field() { python3 -c "import sys,json; d=json.load(sys.stdin); print(d$1)"; }

# Sign in, or register on first run. Echoes an account access token.
account_token() {
  local email="$1" name="$2" body
  body=$(curl -fsS -c "$JAR_DIR/$email" -H 'Content-Type: application/json' -X POST "$API/auth/login" \
           -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\"}" 2>/dev/null) ||
  body=$(curl -fsS -c "$JAR_DIR/$email" -H 'Content-Type: application/json' -X POST "$API/auth/register" \
           -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\",\"display_name\":\"$name\"}")
  echo "$body" | field '["access_token"]'
}

OWNER=$(account_token "olivia@demo.example" "Olivia Owner")
ORG_ID=$(curl -fsS -H "Authorization: Bearer $OWNER" "$API/organizations" | field '["items"][0]["id"] if d["items"] else ""')
if [ -z "$ORG_ID" ]; then
  ORG_ID=$(curl -fsS -H "Authorization: Bearer $OWNER" -H 'Content-Type: application/json' -X POST "$API/organizations" \
             -d '{"name":"Demo Workspace","slug":"demo-workspace"}' | field '["id"]')
  echo "created workspace Demo Workspace"
fi
OWNER_ORG=$(curl -fsS -H "Authorization: Bearer $OWNER" -H 'Content-Type: application/json' -X POST "$API/auth/token/exchange" \
              -d "{\"organization_id\":\"$ORG_ID\"}" | field '["access_token"]')

invite() {
  local email="$1" name="$2" role="$3" reply url token
  reply=$(curl -sS -H "Authorization: Bearer $OWNER_ORG" -H 'Content-Type: application/json' -X POST \
            "$API/organizations/current/invitations" -d "{\"email\":\"$email\",\"org_role\":\"$role\"}")
  url=$(echo "$reply" | field '.get("accept_url") or ""')
  if [ -z "$url" ]; then echo "skipped $email (already a member or invited)"; return; fi
  token=$(account_token "$email" "$name")
  curl -fsS -H "Authorization: Bearer $token" -X POST "$API/invitations/${url##*/}/accept" >/dev/null
  echo "added $name as $role"
}

invite "adam@demo.example" "Adam Admin" admin
invite "maya@demo.example" "Maya Member" member
invite "gus@demo.example" "Gus Guest" guest
# Left pending on purpose so the invitations list has something in it.
curl -sS -o /dev/null -H "Authorization: Bearer $OWNER_ORG" -H 'Content-Type: application/json' -X POST \
  "$API/organizations/current/invitations" -d '{"email":"pending@demo.example","org_role":"member"}'


# --- a project with some issues ------------------------------------------------
auth=(-H "Authorization: Bearer $OWNER_ORG" -H 'Content-Type: application/json')
if [ "$(curl -s -o /dev/null -w '%{http_code}' "${auth[@]}" "$API/projects/DEMO")" = "404" ]; then
  curl -fsS "${auth[@]}" -X POST "$API/projects" \
    -d '{"name":"Demo Platform","key":"DEMO","project_type":"scrum","access":"open","description":"Sample project created by the seed script."}' >/dev/null
  echo "created project DEMO"
fi
if [ "$(curl -fsS "${auth[@]}" "$API/issues?project=DEMO&limit=1" | field '["items"].__len__()')" = "0" ]; then
  MAYA=$(curl -fsS "${auth[@]}" "$API/organizations/current/members?query=maya" | field '["items"][0]["user_id"]')
  issue() { curl -fsS "${auth[@]}" -X POST "$API/issues" -d "$1" | field '["key"]'; }
  doc() { printf '{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"%s"}]}]}' "$1"; }
  EPIC=$(issue '{"project_key":"DEMO","type":"epic","summary":"Checkout redesign","priority":"high"}')
  S1=$(issue "{\"project_key\":\"DEMO\",\"type\":\"story\",\"summary\":\"Pay with a saved card\",\"parent_key\":\"$EPIC\",\"assignee_id\":\"$MAYA\",\"story_points\":5,\"labels\":[\"payments\"],\"description\":$(doc 'Returning customers can pick a stored card at checkout.')}")
  S2=$(issue "{\"project_key\":\"DEMO\",\"type\":\"story\",\"summary\":\"Show delivery estimate before payment\",\"parent_key\":\"$EPIC\",\"story_points\":3}")
  BUG=$(issue "{\"project_key\":\"DEMO\",\"type\":\"bug\",\"summary\":\"Total is wrong when a discount and gift card are combined\",\"priority\":\"highest\",\"assignee_id\":\"$MAYA\",\"labels\":[\"payments\",\"regression\"]}")
  issue "{\"project_key\":\"DEMO\",\"type\":\"subtask\",\"summary\":\"Add card picker component\",\"parent_key\":\"$S1\"}" >/dev/null
  issue "{\"project_key\":\"DEMO\",\"type\":\"subtask\",\"summary\":\"Tokenise card on the server\",\"parent_key\":\"$S1\"}" >/dev/null
  issue '{"project_key":"DEMO","type":"task","summary":"Upgrade payment SDK to v4","priority":"low"}' >/dev/null
  curl -fsS "${auth[@]}" -H 'If-Match: "1"' -X PATCH "$API/issues/$S1" -d '{"status":"in_progress"}' >/dev/null
  curl -fsS "${auth[@]}" -H 'If-Match: "1"' -X PATCH "$API/issues/$S2" -d '{"status":"done"}' >/dev/null
  curl -fsS "${auth[@]}" -X POST "$API/issues/$BUG/links" -d "{\"link_type\":\"blocks\",\"target_key\":\"$S1\"}" >/dev/null
  curl -fsS "${auth[@]}" -X POST "$API/issues/$BUG/comments" -d "{\"body\":$(doc 'Reproduced on staging with a 10% code and a partial gift card.')}" >/dev/null
  echo "created 7 issues in DEMO"
fi

# --- a Scrum board with a planned sprint ---------------------------------------
if [ "$(curl -fsS "${auth[@]}" "$API/boards?project_key=DEMO" | field '["items"].__len__()')" = "0" ]; then
  BOARD=$(curl -fsS "${auth[@]}" -X POST "$API/boards" -d '{"project_key":"DEMO","name":"DEMO board","kind":"scrum"}' | field '["id"]')
  SPRINT=$(curl -fsS "${auth[@]}" -X POST "$API/boards/$BOARD/sprints" -d '{"goal":"Returning customers can check out with a saved card."}' | field '["id"]')
  PLANNED=$(curl -fsS "${auth[@]}" "$API/boards/$BOARD/backlog" | python3 -c 'import sys,json; print(json.dumps([c["id"] for c in json.load(sys.stdin)["backlog"][:2]]))')
  curl -fsS "${auth[@]}" -X POST "$API/boards/$BOARD/move" -d "{\"issue_ids\":$PLANNED,\"sprint_id\":\"$SPRINT\"}" >/dev/null
  echo "created a Scrum board for DEMO with a planned sprint"
fi

echo
echo "Sign in at $ORIGIN as olivia@demo.example (owner), adam@ (admin), maya@ (member) or gus@ (guest)"
echo "Password for all demo accounts: $PASSWORD"
