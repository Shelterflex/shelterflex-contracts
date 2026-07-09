# Contributing to Shelterflex Contracts

Thanks for contributing! These are the **Soroban smart contracts** for Shelterflex, a Rent
Now, Pay Later (RNPL) rental platform. See the [ecosystem overview](https://github.com/Shelterflex/shelterflex-platform/blob/main/ECOSYSTEM.md)
for how this repo fits with the web app and API.

## Ways to contribute

Contract logic, tests, deployment scripts, gas/optimization, and security hardening across
escrow, staking, rent payments, whistleblower rewards, oracles, and access control.

## Ground rules

- Keep PRs small and focused — 1 issue per PR.
- Link the issue you're addressing (e.g. `Fixes #123`).
- Every contract change needs tests (`cargo test --workspace`).
- Security-sensitive: review `docs/security/CONTRACT_SECURITY_CHECKLIST.md` before submitting.
- Keep `test-vectors.json` in sync with platform (a CI drift check enforces this).
- Never commit secrets, keys, or seed phrases.

## Development setup

```bash
cargo test --workspace
stellar contract build
```

See `DEPLOYMENT.md` and `docs/contracts/` for deployment and upgrade procedures.

## Before you open a PR

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

## Contract upgrades

Contract-upgrade PRs must document: target network, new contract address/hash, upgrade
governance, and verification steps. Follow `docs/contracts/UPGRADE_PROCESS.md`.

## Creating an issue

Use the templates under `.github/ISSUE_TEMPLATE/`. Check existing issues first.
