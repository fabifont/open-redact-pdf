#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

echo "Publishing open-redact-pdf"
attempt=1
until cargo publish -p open-redact-pdf --token "${CARGO_REGISTRY_TOKEN}"; do
  if [[ "${attempt}" -ge 5 ]]; then
    echo "failed to publish open-redact-pdf after ${attempt} attempts" >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  echo "Retrying after crates.io propagation delay"
  sleep 30
done
