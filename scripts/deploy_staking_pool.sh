#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"
: "${TOKEN_CONTRACT_ID:?TOKEN_CONTRACT_ID must be set}"

resolve_network "$NETWORK"

echo "==> Building staking_pool..."
stellar contract build --package staking_pool --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/staking_pool.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM not found at $WASM" >&2
  exit 1
fi

echo "==> Deploying staking_pool to $NETWORK..."
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
  --admin "$ADMIN_ADDRESS" \
  --token "$TOKEN_CONTRACT_ID"

echo "staking_pool deployed: $CONTRACT_ID"
export STAKING_POOL_ID="$CONTRACT_ID"
