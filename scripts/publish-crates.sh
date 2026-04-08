#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

packages=(
  pdf_graphics
  pdf_objects
  pdf_content
  pdf_targets
  pdf_text
  pdf_redact
  pdf_writer
  open-redact-pdf
)

for package in "${packages[@]}"; do
  echo "Publishing ${package}"
  attempt=1
  until cargo publish -p "${package}" --token "${CARGO_REGISTRY_TOKEN}"; do
    if [[ "${attempt}" -ge 5 ]]; then
      echo "failed to publish ${package} after ${attempt} attempts" >&2
      exit 1
    fi
    attempt=$((attempt + 1))
    echo "Retrying ${package} after crates.io propagation delay"
    sleep 30
  done
  sleep 15
done
