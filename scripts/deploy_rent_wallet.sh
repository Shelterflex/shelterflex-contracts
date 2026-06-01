#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=networks.sh
source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"

resolve_network "$NETWORK"

echo "==> Building rent_wallet..."
stellar contract build --package rent_wallet --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/rent_wallet.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM not found at $WASM" >&2
  exit 1
fi

echo "==> Deploying rent_wallet to $NETWORK..."
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE")

echo "CONTRACT_ID=$CONTRACT_ID"

echo "==> Initialising rent_wallet..."
ADMIN_ADDRESS=$(stellar keys address "$DEPLOY_IDENTITY")

stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE" \
  -- init \
  --admin "$ADMIN_ADDRESS"

echo "rent_wallet deployed and initialised:"
echo "  Contract ID : $CONTRACT_ID"
echo "  Admin       : $ADMIN_ADDRESS"
echo "  Network     : $NETWORK"

export RENT_WALLET_ID="$CONTRACT_ID"
