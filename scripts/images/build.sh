#!/usr/bin/env bash
# Build every service image and the web image.
#   scripts/images/build.sh [tag] [prefix]      (defaults: dev, meridian)
# Each service image is started once without configuration: it must get as far as reading its
# settings (and exit with a configuration error), which catches a missing shared library.
set -euo pipefail
cd "$(dirname "$0")/../.."
TAG="${1:-dev}"
PREFIX="${2:-meridian}"
SERVICES=(tenant identity user permission project workflow issue agile realtime notification search analytics)

for s in "${SERVICES[@]}"; do
  image="${PREFIX}/${s}-service:${TAG}"
  echo "==> ${image}"
  docker build -q --pull -f infrastructure/docker/service.Dockerfile --build-arg SERVICE="${s}-service" -t "${image}" .
  if docker run --rm --network none "${image}" 2>&1 | grep -E "error while loading shared libraries|exec format error|no such file"; then
    echo "!! ${image} does not start" >&2; exit 1
  fi
done
echo "==> ${PREFIX}/web:${TAG}"
docker build -q --pull -f infrastructure/docker/web.Dockerfile -t "${PREFIX}/web:${TAG}" .
