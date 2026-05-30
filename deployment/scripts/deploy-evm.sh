#!/usr/bin/env bash
# =============================================================================
# deploy-evm.sh — EVM contract deployment (PLACEHOLDER)
#
# This script is a placeholder for future EVM (Base / Ethereum) deployments.
# Soroban contracts are the primary deployment target today.  When Solidity
# contracts are added to the monorepo, this script will be fleshed out.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="$SCRIPT_DIR/../config/evm-testnet.json"

cat <<'EOF'
═══════════════════════════════════════════════════════════════
  EVM Contract Deployment — PLACEHOLDER
═══════════════════════════════════════════════════════════════

  This script is a stub.  No EVM contracts are deployed yet.

  When ready, an EVM deployment pipeline will require:

  1. Solidity contracts compiled via:
       • Hardhat  (Node.js)     — npx hardhat compile
       • Foundry  (Rust/CLI)    — forge build

  2. Deployment to Base Sepolia (chain 84532) using:
       • ethers.js + Hardhat    — npx hardhat run scripts/deploy.ts --network base-sepolia
       • Foundry                — forge script script/Deploy.s.sol --rpc-url $EVM_RPC_URL --broadcast

  3. Required environment variables:
       • EVM_DEPLOYER_PRIVATE_KEY   — Private key of the deploying wallet
       • EVM_RPC_URL                — JSON-RPC endpoint (default in config: https://sepolia.base.org)
       • EVM_DEPLOYER_ADDRESS       — Public address of the deploying wallet

  4. Required tooling:
       • Node.js >= 18 + Hardhat   OR
       • Rust + Foundry (forge, cast, anvil)

  5. Network configuration is defined in:
       contracts/deployment/config/evm-testnet.json

═══════════════════════════════════════════════════════════════
EOF

if [[ -f "$CONFIG_FILE" ]]; then
  echo "  EVM config found: $CONFIG_FILE"
  echo ""
  echo "  Contents:"
  cat "$CONFIG_FILE"
  echo ""
fi

echo "  No action taken. Exiting."
exit 0
