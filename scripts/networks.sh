#!/usr/bin/env bash
# Network configuration for Shelterflex contract deployment.
# Source this file in per-contract scripts; override via env vars.

# ── Testnet ───────────────────────────────────────────────────────────────────
TESTNET_RPC="${TESTNET_RPC:-https://soroban-testnet.stellar.org}"
TESTNET_PASSPHRASE="${TESTNET_PASSPHRASE:-Test SDF Network ; September 2015}"
TESTNET_FRIENDBOT="${TESTNET_FRIENDBOT:-https://friendbot.stellar.org}"

# ── Futurenet ─────────────────────────────────────────────────────────────────
FUTURENET_RPC="${FUTURENET_RPC:-https://rpc-futurenet.stellar.org}"
FUTURENET_PASSPHRASE="${FUTURENET_PASSPHRASE:-Test SDF Future Network ; October 2022}"

# ── Resolve active network ────────────────────────────────────────────────────
# Usage: source networks.sh && resolve_network testnet
resolve_network() {
  local net="${1:-testnet}"
  case "$net" in
    testnet)
      ACTIVE_RPC="$TESTNET_RPC"
      ACTIVE_PASSPHRASE="$TESTNET_PASSPHRASE"
      ;;
    futurenet)
      ACTIVE_RPC="$FUTURENET_RPC"
      ACTIVE_PASSPHRASE="$FUTURENET_PASSPHRASE"
      ;;
    *)
      echo "ERROR: unknown network '$net'. Valid values: testnet, futurenet" >&2
      exit 1
      ;;
  esac
  export ACTIVE_RPC ACTIVE_PASSPHRASE
  echo "Network: $net  RPC: $ACTIVE_RPC"
}
