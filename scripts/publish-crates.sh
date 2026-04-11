#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

publish_crate() {
  local crate="$1"
  local attempt=1
  local output

  echo "Publishing ${crate}"

  while true; do
    set +e
    output="$(cargo publish -p "${crate}" 2>&1)"
    status=$?
    set -e

    printf '%s\n' "${output}"

    if [[ "${status}" -eq 0 ]]; then
      return 0
    fi

    if grep -qiE "already (?:uploaded|exists)|previously uploaded" <<<"${output}"; then
      echo "${crate} is already published for this version; continuing"
      return 0
    fi

    if [[ "${attempt}" -ge 5 ]]; then
      echo "failed to publish ${crate} after ${attempt} attempts" >&2
      return 1
    fi

    attempt=$((attempt + 1))
    echo "Retrying ${crate} after crates.io propagation delay"
    sleep 30
  done
}

crates=(
  open-redact-pdf-graphics
  open-redact-pdf-objects
  open-redact-pdf-content
  open-redact-pdf-targets
  open-redact-pdf-text
  open-redact-pdf-redact
  open-redact-pdf-writer
  open-redact-pdf
)

for crate in "${crates[@]}"; do
  publish_crate "${crate}"
done
