#!/usr/bin/env bash
set -euo pipefail

ADMIN_IDENTITY="${1:-}"
OPERATOR_IDENTITY="${2:-}"
ISSUER_IDENTITY="${3:-}"

if [[ -z "$ADMIN_IDENTITY" || -z "$OPERATOR_IDENTITY" || -z "$ISSUER_IDENTITY" ]]; then
  echo "Usage: $0 <admin_identity> <operator_identity> <issuer_identity>" >&2
  echo "Example: $0 shelter_admin shelter_operator shelter_issuer" >&2
  exit 1
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "stellar CLI is not installed or not on PATH." >&2
  exit 1
fi

create_or_refund() {
  local identity="$1"
  echo "Creating/funding testnet identity: $identity"
  stellar keys generate "$identity" --network testnet --fund --overwrite
}

print_address() {
  local identity="$1"
  local addr
  addr="$(stellar keys address "$identity")"
  echo "  $identity=$addr"
}

create_or_refund "$ADMIN_IDENTITY"
create_or_refund "$OPERATOR_IDENTITY"
create_or_refund "$ISSUER_IDENTITY"

echo ""
echo "Testnet identities ready:"
print_address "$ADMIN_IDENTITY"
print_address "$OPERATOR_IDENTITY"
print_address "$ISSUER_IDENTITY"
