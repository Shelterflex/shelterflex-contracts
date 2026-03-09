# Contracts Deployment & IDs (Soroban CLI)

This runbook is the single source of truth for deploying the core contracts on Soroban testnet and wiring the resulting IDs into backend environment variables.

It covers:

- identity setup
- contract builds
- deploy + init flows
- copy/paste env values for backend

## Prerequisites

- Rust stable toolchain
- `stellar` CLI with Soroban support
- Access to Soroban testnet Friendbot

Verify tooling:

```bash
rustc --version
stellar --version
stellar contract --help
```

Identity requirements:

- `admin` identity: deploys and initializes contracts
- `operator` identity: operator address for contracts that require it
- `issuer` identity: used to deploy the test USDC asset contract

## One-command Contributor Flow (Recommended)

From repository root:

```bash
# 1) (Optional) Create/fund testnet identities
bash contracts/scripts/setup-testnet-identities.sh shelter_admin shelter_operator shelter_issuer

# 2) Build all required WASM artifacts
bash contracts/scripts/build-wasm.sh

# 3) Deploy + initialize and print backend env vars
bash contracts/scripts/deploy-all.sh testnet shelter_admin shelter_operator shelter_issuer
```

Expected output:

- contract IDs that start with `C...`
- an env snippet containing:
  - `SOROBAN_CONTRACT_ID`
  - `SOROBAN_STAKING_POOL_ID`
  - `SOROBAN_STAKING_REWARDS_ID`
  - `SOROBAN_USDC_TOKEN_ID`

Optional: write env output directly to a file:

```bash
bash contracts/scripts/deploy-all.sh \
  --env-file backend/.env.soroban.testnet \
  testnet shelter_admin shelter_operator shelter_issuer
```

## Manual Commands Per Contract (Soroban CLI)

Use this section when you want transparent step-by-step commands instead of the helper script.

### 1) Build Commands

Build all contracts (recommended, stable command):

```bash
stellar contract build --out-dir contracts/artifacts
```

Expected WASM files for this workflow:

- `contracts/artifacts/transaction_receipt_contract.wasm`
- `contracts/artifacts/staking_pool.wasm`
- `contracts/artifacts/staking_rewards.wasm`

### 2) Resolve Identity Addresses

```bash
ADMIN_ADDR="$(stellar keys address shelter_admin)"
OPERATOR_ADDR="$(stellar keys address shelter_operator)"
ISSUER_ADDR="$(stellar keys address shelter_issuer)"
```

### 3) Deploy Token (USDC Asset Contract)

```bash
USDC_TOKEN_ID="$(stellar contract asset deploy \
  --asset "USDC:${ISSUER_ADDR}" \
  --source-account shelter_admin \
  --network testnet)"
echo "$USDC_TOKEN_ID"
```

Expected output: one contract ID, e.g. `C...`

### 4) Deploy + Init: `transaction-receipt-contract`

```bash
TRANSACTION_RECEIPT_ID="$(stellar contract deploy \
  --wasm contracts/artifacts/transaction_receipt_contract.wasm \
  --source-account shelter_admin \
  --network testnet \
  --alias transaction_receipt)"

stellar contract invoke \
  --id "$TRANSACTION_RECEIPT_ID" \
  --source-account shelter_admin \
  --network testnet \
  --send=yes \
  -- init --admin "$ADMIN_ADDR" --operator "$OPERATOR_ADDR"
```

### 5) Deploy + Init: `staking_pool`

```bash
STAKING_POOL_ID="$(stellar contract deploy \
  --wasm contracts/artifacts/staking_pool.wasm \
  --source-account shelter_admin \
  --network testnet \
  --alias staking_pool)"

stellar contract invoke \
  --id "$STAKING_POOL_ID" \
  --source-account shelter_admin \
  --network testnet \
  --send=yes \
  -- init --admin "$ADMIN_ADDR" --token "$USDC_TOKEN_ID"
```

### 6) Deploy + Init: `staking_rewards`

```bash
STAKING_REWARDS_ID="$(stellar contract deploy \
  --wasm contracts/artifacts/staking_rewards.wasm \
  --source-account shelter_admin \
  --network testnet \
  --alias staking_rewards)"

stellar contract invoke \
  --id "$STAKING_REWARDS_ID" \
  --source-account shelter_admin \
  --network testnet \
  --send=yes \
  -- init --admin "$ADMIN_ADDR"
```

## Backend Env Vars

Set these in your backend environment:

```dotenv
SOROBAN_RPC_URL=https://soroban-testnet.stellar.org
SOROBAN_NETWORK_PASSPHRASE=Test SDF Network ; September 2015
SOROBAN_CONTRACT_ID=<TRANSACTION_RECEIPT_ID>
SOROBAN_STAKING_POOL_ID=<STAKING_POOL_ID>
SOROBAN_STAKING_REWARDS_ID=<STAKING_REWARDS_ID>
SOROBAN_USDC_TOKEN_ID=<USDC_TOKEN_ID>
```

Optional signer secret (if backend submits txs):

```bash
stellar keys secret shelter_operator
```

Copy the returned secret (`S...`) into `SOROBAN_ADMIN_SECRET`.

## Troubleshooting

- `stellar: command not found`
  - Install/upgrade the Stellar CLI, then re-run `stellar --version`.
- missing wasm file in `contracts/artifacts/`
  - Re-run `bash contracts/scripts/build-wasm.sh`.
- `insufficient balance` / tx submission errors on testnet
  - Re-fund identities with Friendbot:
    - `stellar keys generate <identity> --network testnet --fund --overwrite`
- `already initialized` on init command
  - You are reusing an existing contract ID. Deploy a fresh contract and retry init.
- alias already exists
  - Use a different alias or remove old alias entries from your local CLI config.
