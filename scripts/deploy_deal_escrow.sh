#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"
: "${RENT_WALLET_ID:?RENT_WALLET_ID must be set before deploying deal_escrow}"

resolve_network "$NETWORK"

echo "==> Building deal_escrow..."
stellar contract build --package deal_escrow --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/deal_escrow.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM not found at $WASM" >&2
  exit 1
fi

echo "==> Deploying deal_escrow to $NETWORK..."
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE")

echo "CONTRACT_ID=$CONTRACT_ID"

ADMIN_ADDRESS=$(stellar keys address "$DEPLOY_IDENTITY")

echo "==> Initialising deal_escrow..."
stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE" \
  -- init \
  --admin "$ADMIN_ADDRESS" \
  --rent_wallet "$RENT_WALLET_ID"

echo "deal_escrow deployed and initialised:"
echo "  Contract ID  : $CONTRACT_ID"
echo "  Admin        : $ADMIN_ADDRESS"
echo "  rent_wallet  : $RENT_WALLET_ID"
echo "  Network      : $NETWORK"

export DEAL_ESCROW_ID="$CONTRACT_ID"
