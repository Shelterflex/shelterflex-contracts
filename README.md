# Shelterflex Contracts (Soroban)

This folder contains Soroban smart contracts written in Rust.

## Prerequisites

- Rust (stable)
- Soroban CLI (via `stellar` CLI) installed and configured

## Workspace

- `rent_wallet/` - Minimal reference contract for wallet-like credit/debit flows

## Build & test

```bash
cd contracts

# 1) Run unit tests
cargo test

# 2) Build the deployable contract WASM (recommended)
# Produces an optimized .wasm under each contract crate's `target/` directory.
stellar contract build

# 3) Build the deployable contract WASM (cargo-only alternative)
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown -p rent_wallet
```

The `cargo build --release` (native) build is not the deployable artifact for Soroban.

## Troubleshooting

- **Tooling versions**
  - `rustc --version` should be stable.
  - `stellar --version` should be recent enough to support `stellar contract build`.
  - This repo uses `soroban-sdk = 22.0.7` (see `rent_wallet/Cargo.toml`).

- **`error: toolchain ... does not support target 'wasm32-unknown-unknown'`**
  - Install the target:
    - `rustup target add wasm32-unknown-unknown`

- **`stellar: command not found` / `stellar contract: unknown command`**
  - Install/upgrade the Stellar CLI with Soroban support and verify:
    - `stellar --version`
    - `stellar contract --help`

- **Build succeeds but you can’t find the `.wasm`**
  - If you used `stellar contract build`, look under the relevant contract crate’s `target/` directory.
  - If you used the cargo-only build, look under:
    - `contracts/target/wasm32-unknown-unknown/release/`

## Deploy (example)

Exact commands depend on your Soroban CLI version.

High-level flow:

1. Choose network (local sandbox or testnet)
2. Build the contract WASM
3. Deploy
4. Initialize with an admin address

Recommended contributor expectations for deployment-related issues:

- Provide exact CLI commands you used
- Include network info (rpc url + passphrase)
- Include contract id and init params

## Contract interface

### `rent_wallet`

- `init(admin: Address)`
- `credit(user: Address, amount: i128)` (admin-auth)
- `debit(user: Address, amount: i128)` (admin-auth)
- `balance(user: Address) -> i128`
- `set_admin(new_admin: Address)` (admin-auth)
