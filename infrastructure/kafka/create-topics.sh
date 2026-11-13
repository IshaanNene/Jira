#!/usr/bin/env bash
# Creates the platform topics (docs/EVENT_ARCHITECTURE.md §2) and their dead-letter topics.
# Idempotent. Local development uses replication factor 1; production topics are managed
# by infrastructure code with replication factor 3 and min.insync.replicas 2.
set -euo pipefail

BOOTSTRAP="${1:-localhost:9092}"
PARTITIONS="${PARTITIONS:-6}"
TOPICS="identity tenant organization project issue workflow agile permission automation notification audit"

for name in $TOPICS; do
  topic="platform.${name}.v1"
  for t in "$topic" "${topic}.dlq"; do
    /opt/kafka/bin/kafka-topics.sh --bootstrap-server "$BOOTSTRAP" --create --if-not-exists \
      --topic "$t" --partitions "$PARTITIONS" --replication-factor 1 \
      --config retention.ms=604800000 > /dev/null
    echo "topic ready: $t"
  done
done
