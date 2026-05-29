#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"

resolve_network "$NETWORK"

echo "==> Building tenant_reputation..."
stellar contract build --package tenant_reputation --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/tenant_reputation.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM not found at $WASM" >&2
  exit 1
fi

CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE")

ADMIN_ADDRESS=$(stellar keys address "$DEPLOY_IDENTITY")

stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE" \
  -- init \
  --admin "$ADMIN_ADDRESS"

echo "tenant_reputation deployed: $CONTRACT_ID"
export TENANT_REPUTATION_ID="$CONTRACT_ID"
