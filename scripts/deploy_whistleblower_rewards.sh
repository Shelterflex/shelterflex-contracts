#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"
OPERATOR_IDENTITY="${OPERATOR_IDENTITY:-shelter_operator}"
: "${TOKEN_CONTRACT_ID:?TOKEN_CONTRACT_ID (USDC asset contract) must be set}"

resolve_network "$NETWORK"

echo "==> Building whistleblower_rewards..."
stellar contract build --package whistleblower_rewards --profile release-with-logs 2>&1

WASM="$CONTRACTS_DIR/target/wasm32-unknown-unknown/release-with-logs/whistleblower_rewards.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM not found at $WASM" >&2
  exit 1
fi

echo "==> Deploying whistleblower_rewards to $NETWORK..."
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE")

ADMIN_ADDRESS=$(stellar keys address "$DEPLOY_IDENTITY")
OPERATOR_ADDRESS=$(stellar keys address "$OPERATOR_IDENTITY")

echo "==> Initialising whistleblower_rewards..."
stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$ACTIVE_RPC" \
  --network-passphrase "$ACTIVE_PASSPHRASE" \
  -- init \
  --admin "$ADMIN_ADDRESS" \
  --operator "$OPERATOR_ADDRESS" \
  --token "$TOKEN_CONTRACT_ID"

echo "whistleblower_rewards deployed and initialised:"
echo "  Contract ID : $CONTRACT_ID"
echo "  Admin       : $ADMIN_ADDRESS"
echo "  Operator    : $OPERATOR_ADDRESS"
echo "  Token       : $TOKEN_CONTRACT_ID"
echo "  Network     : $NETWORK"

export WHISTLEBLOWER_REWARDS_ID="$CONTRACT_ID"
