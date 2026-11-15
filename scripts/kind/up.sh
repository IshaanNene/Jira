#!/usr/bin/env bash
# Run the platform in a local Kubernetes cluster (kind) with the Helm chart, against the data
# stores from docker-compose.yml, the way production runs it against managed services.
#   scripts/kind/up.sh        create or update; then open http://localhost:8088
#   scripts/kind/down.sh      delete the cluster
# Needs: docker, kind, kubectl, helm, and `make infra` (the data stores) running.
set -euo pipefail
cd "$(dirname "$0")/../.."
CLUSTER=meridian
NS=meridian
TAG="${TAG:-dev}"
SERVICES=(tenant identity user permission project workflow issue agile realtime notification search analytics)

kind get clusters | grep -qx "$CLUSTER" || kind create cluster --config scripts/kind/cluster.yaml --wait 120s
kind export kubeconfig --name "$CLUSTER" > /dev/null 2>&1   # also makes it the current context

# The data stores join the nodes' Docker network under their service names, so pods reach
# them as postgres:5432, kafka:9092 (Kafka's in-network listener), and so on.
PROJECT="$(docker compose ps --format '{{.Project}}' | head -1)"
[ -n "$PROJECT" ] || { echo "the data stores are not running: make infra" >&2; exit 1; }
for svc in postgres redis kafka opensearch clickhouse object-storage mailpit; do
  container="$(docker compose ps -q "$svc")"
  [ -n "$container" ] || { echo "$svc is not running: make infra" >&2; exit 1; }
  docker network connect --alias "$svc" kind "$container" 2>/dev/null || true
done
SUBNET="$(docker network inspect kind --format '{{range .IPAM.Config}}{{if .Subnet}}{{.Subnet}} {{end}}{{end}}' | tr ' ' '\n' | grep -v ':' | head -1)"

echo "==> loading images ($TAG)"
images=(); for s in "${SERVICES[@]}"; do images+=("meridian/${s}-service:${TAG}"); done
kind load docker-image --name "$CLUSTER" "${images[@]}" "meridian/web:${TAG}" > /dev/null

kubectl get ns "$NS" > /dev/null 2>&1 || kubectl create ns "$NS"

# Secrets: what External Secrets provides in production. Development placeholders only,
# matching infrastructure/postgres/init and .env.example.
set -a; . ./.env.example; set +a
secret() { kubectl -n "$NS" create secret generic "$1" "${@:2}" --dry-run=client -o yaml | kubectl apply -f - > /dev/null; }
secret meridian-shared \
  --from-literal=INTERNAL_SERVICE_SECRET="$INTERNAL_SERVICE_SECRET" \
  --from-literal=REDIS_URL=redis://redis:6379
for s in tenant identity user permission project workflow issue agile notification search analytics; do
  extra=()
  case "$s" in
    identity) extra=(--from-literal=IDENTITY_KEY_ENCRYPTION_KEY="$IDENTITY_KEY_ENCRYPTION_KEY") ;;
    issue) extra=(--from-literal=S3_ACCESS_KEY_ID="$S3_ACCESS_KEY_ID" --from-literal=S3_SECRET_ACCESS_KEY="$S3_SECRET_ACCESS_KEY") ;;
    notification) extra=(--from-literal=SMTP_URL=smtp://mailpit:1025) ;;
    analytics) extra=(--from-literal=CLICKHOUSE_WRITER_PASSWORD="$CLICKHOUSE_WRITER_PASSWORD" --from-literal=CLICKHOUSE_READER_PASSWORD="$CLICKHOUSE_READER_PASSWORD") ;;
  esac
  secret "meridian-$s" --from-literal=DATABASE_URL="postgres://${s}_app:${s}_app@postgres:5432/${s}_db" ${extra[@]+"${extra[@]}"}
  migrate=(--from-literal=DATABASE_MIGRATION_URL="postgres://${s}_owner:${s}_owner@postgres:5432/${s}_db")
  [ "$s" = analytics ] && migrate+=(--from-literal=CLICKHOUSE_OWNER_USER="$CLICKHOUSE_OWNER_USER" --from-literal=CLICKHOUSE_OWNER_PASSWORD="$CLICKHOUSE_OWNER_PASSWORD")
  secret "meridian-$s-migrate" "${migrate[@]}"
done
secret meridian-search --from-literal=OPENSEARCH_URL=http://opensearch:9200

echo "==> installing the chart"
helm upgrade --install meridian infrastructure/helm/meridian -n "$NS" \
  -f infrastructure/helm/meridian/values-kind.yaml \
  --set image.tag="$TAG" --set "networkPolicy.egress.cidrs={$SUBNET}" \
  --wait --timeout 10m
kubectl -n "$NS" get pods -o wide
echo "Open http://localhost:8088"
