#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS_DIR="$ROOT_DIR/artifacts"

mkdir -p "$ARTIFACTS_DIR"

cd "$ROOT_DIR"

stellar contract build --out-dir "$ARTIFACTS_DIR"

required_artifacts=(
  "transaction_receipt_contract.wasm"
  "staking_pool.wasm"
  "staking_rewards.wasm"
)

for artifact in "${required_artifacts[@]}"; do
  if [[ ! -f "$ARTIFACTS_DIR/$artifact" ]]; then
    echo "Missing expected artifact: $ARTIFACTS_DIR/$artifact" >&2
    exit 1
  fi
done

echo "WASM artifacts written to: $ARTIFACTS_DIR"
