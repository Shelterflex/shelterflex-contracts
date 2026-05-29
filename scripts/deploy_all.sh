#!/usr/bin/env bash
# Orchestration script — deploys all Shelterflex contracts in dependency order,
# writes their IDs to contracts/deployments/<network>.json, and is idempotent
# (skips already-deployed contracts unless --force is passed).
#
# Usage:
#   DEPLOY_IDENTITY=shelter_admin ./scripts/deploy_all.sh testnet
#   DEPLOY_IDENTITY=shelter_admin ./scripts/deploy_all.sh testnet --force

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_DIR="$CONTRACTS_DIR/deployments"

source "$SCRIPT_DIR/networks.sh"

NETWORK="${1:-testnet}"
FORCE=false
if [[ "${2:-}" == "--force" ]]; then
  FORCE=true
fi

DEPLOY_IDENTITY="${DEPLOY_IDENTITY:-shelter_admin}"
OPERATOR_IDENTITY="${OPERATOR_IDENTITY:-shelter_operator}"
ISSUER_IDENTITY="${ISSUER_IDENTITY:-shelter_issuer}"

resolve_network "$NETWORK"
mkdir -p "$DEPLOYMENTS_DIR"

JSON_FILE="$DEPLOYMENTS_DIR/${NETWORK}.json"
if [[ ! -f "$JSON_FILE" ]]; then
  echo '{}' > "$JSON_FILE"
fi

read_id() { python3 -c "import json,sys; d=json.load(open('$JSON_FILE')); print(d.get('$1',{}).get('contractId',''))" 2>/dev/null || true; }

write_id() {
  local name="$1" id="$2"
  local ts
  ts=$(python3 -c "from datetime import datetime,timezone; print(datetime.now(timezone.utc).isoformat())")
  python3 - <<EOF
import json, sys
with open('$JSON_FILE') as f:
    d = json.load(f)
d['$name'] = {'contractId': '$id', 'network': '$NETWORK', 'deployedAt': '$ts'}
with open('$JSON_FILE', 'w') as f:
    json.dump(d, f, indent=2)
EOF
  echo "  Saved $name=$id → $JSON_FILE"
}

deploy_or_skip() {
  local name="$1"
  local script="$2"
  local var_name="$3"

  local existing
  existing=$(read_id "$name")
  if [[ -n "$existing" && "$FORCE" == "false" ]]; then
    echo "==> Skipping $name (already deployed: $existing) — use --force to redeploy"
    export "$var_name=$existing"
    return 0
  fi

  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "Deploying: $name"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  bash "$SCRIPT_DIR/$script" "$NETWORK"

  local id="${!var_name:-}"
  if [[ -z "$id" ]]; then
    echo "ERROR: $var_name was not exported by $script" >&2
    exit 1
  fi
  write_id "$name" "$id"
}

# ── Step 1: Token asset contract ──────────────────────────────────────────────
TOKEN_CONTRACT_ID=$(read_id "token_usdc")
if [[ -z "$TOKEN_CONTRACT_ID" || "$FORCE" == "true" ]]; then
  echo "==> Deploying USDC asset contract..."
  ISSUER_ADDRESS=$(stellar keys address "$ISSUER_IDENTITY")
  TOKEN_CONTRACT_ID=$(stellar contract asset deploy \
    --source "$ISSUER_IDENTITY" \
    --network "$NETWORK" \
    --rpc-url "$ACTIVE_RPC" \
    --network-passphrase "$ACTIVE_PASSPHRASE" \
    --asset USDC:"$ISSUER_ADDRESS" 2>&1 | tail -1)
  write_id "token_usdc" "$TOKEN_CONTRACT_ID"
else
  echo "==> Skipping token_usdc (already deployed: $TOKEN_CONTRACT_ID)"
fi
export TOKEN_CONTRACT_ID

# ── Step 2: rent_wallet (no dependencies) ─────────────────────────────────────
deploy_or_skip "rent_wallet" "deploy_rent_wallet.sh" "RENT_WALLET_ID"

# ── Step 3: deal_escrow (depends on rent_wallet) ──────────────────────────────
deploy_or_skip "deal_escrow" "deploy_deal_escrow.sh" "DEAL_ESCROW_ID"

# ── Step 4: rent_payments ─────────────────────────────────────────────────────
deploy_or_skip "rent_payments" "deploy_rent_payments.sh" "RENT_PAYMENTS_ID"

# ── Step 5: whistleblower_rewards (depends on token) ─────────────────────────
deploy_or_skip "whistleblower_rewards" "deploy_whistleblower_rewards.sh" "WHISTLEBLOWER_REWARDS_ID"

# ── Step 6: staking_pool ─────────────────────────────────────────────────────
deploy_or_skip "staking_pool" "deploy_staking_pool.sh" "STAKING_POOL_ID"

# ── Step 7: oracle_price_feeds ────────────────────────────────────────────────
deploy_or_skip "oracle_price_feeds" "deploy_oracle_price_feeds.sh" "ORACLE_PRICE_FEEDS_ID"

# ── Step 8: tenant_reputation ────────────────────────────────────────────────
deploy_or_skip "tenant_reputation" "deploy_tenant_reputation.sh" "TENANT_REPUTATION_ID"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "All contracts deployed. IDs written to:"
echo "  $JSON_FILE"
echo ""
echo "Backend .env snippet:"
echo "  RENT_WALLET_CONTRACT_ID=${RENT_WALLET_ID:-<not deployed>}"
echo "  DEAL_ESCROW_CONTRACT_ID=${DEAL_ESCROW_ID:-<not deployed>}"
echo "  RENT_PAYMENTS_CONTRACT_ID=${RENT_PAYMENTS_ID:-<not deployed>}"
echo "  WHISTLEBLOWER_REWARDS_CONTRACT_ID=${WHISTLEBLOWER_REWARDS_ID:-<not deployed>}"
echo "  STAKING_POOL_CONTRACT_ID=${STAKING_POOL_ID:-<not deployed>}"
echo "  ORACLE_PRICE_FEEDS_CONTRACT_ID=${ORACLE_PRICE_FEEDS_ID:-<not deployed>}"
echo "  TENANT_REPUTATION_CONTRACT_ID=${TENANT_REPUTATION_ID:-<not deployed>}"
echo "  TOKEN_USDC_CONTRACT_ID=${TOKEN_CONTRACT_ID:-<not deployed>}"
