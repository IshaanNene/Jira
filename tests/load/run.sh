#!/usr/bin/env bash
# Run the load test and keep the summary next to the script (results/<timestamp>.json).
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p results
k6 run --summary-export "results/$(date +%Y%m%d-%H%M%S).json" platform.js "$@"
