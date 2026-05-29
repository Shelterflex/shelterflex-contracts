#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"

resolve_network "$NETWORK"

echo "==> Building oracle_price_feeds..."
stellar contract build --package oracle_price_feeds --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/oracle_price_feeds.wasm"
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

echo "oracle_price_feeds deployed: $CONTRACT_ID"
export ORACLE_PRICE_FEEDS_ID="$CONTRACT_ID"
