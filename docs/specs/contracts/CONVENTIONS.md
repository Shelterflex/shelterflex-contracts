# Soroban Contract Conventions

This document defines conventions for Soroban smart contracts in this repo to keep indexers and backend integrations stable as new contracts are added.

## Errors

### Use `#[contracterror]` + `Result`

- Public, state-mutating contract functions should return `Result<_, ContractError>`.
- Prefer typed errors over `panic!("...")`.

### Standard error variants

Contracts should implement a `ContractError` enum using:

- `#[contracterror]`
- `#[repr(u32)]`

Recommended shared variants (use when applicable):

- `AlreadyInitialized = 1`
- `NotAuthorized = 2`
- `Paused = 3`
- `InvalidAmount = 4`

Common contract-specific variants (examples):

- `InsufficientBalance`
- `Duplicate`
- `NotFound`
- `InvalidInput`

### Authorization and pause checks

- Admin-only entrypoints should require auth of the caller and compare against stored admin.
- Operator-only entrypoints should require auth of the caller and compare against stored operator (or an operator set).
- If the contract is paused, mutating entrypoints should return `Err(ContractError::Paused)`.

## Events

### Topic shape

Contracts should emit Soroban events with topics structured as:

- `(contract: Symbol, event: Symbol, ...event-specific topics)`

This gives indexers a stable primary discriminator (contract + event) and allows filtering.

### Naming

- `contract` should be a stable snake_case identifier for the contract crate (e.g. `"rent_wallet"`, `"transaction_receipt"`).
- `event` should be a stable snake_case verb phrase (e.g. `"init"`, `"credit"`, `"receipt_recorded"`).

### Examples

- Rent wallet credit:
  - **Topic**: `(rent_wallet, credit, user)`
  - **Data**: `amount`

- Transaction receipt recorded:
  - **Topic**: `(transaction_receipt, receipt_recorded, tx_id)`
  - **Data**: `Receipt`

## Initialization

### Standard init pattern

- Contracts should expose exactly one initialization entrypoint named `init`.
- `init` should be callable once and return `AlreadyInitialized` if called again.
- `init` should store at least an `admin` address. If the contract uses an operator role, also store an `operator` address.

Recommended signatures:

- Admin-only contracts:
  - `init(env: Env, admin: Address) -> Result<(), ContractError>`
- Admin + operator contracts:
  - `init(env: Env, admin: Address, operator: Address) -> Result<(), ContractError>`

### Storage


- Store `Admin` in instance storage.
- If present, store `Operator` in instance storage.
- Store `Paused` in instance storage (default `false`).

## Reentrancy

### Canonical primitive

Any entrypoint that performs an external call (e.g. a `TokenClient` transfer) must hold a
reentrancy lock across that call. Do **not** hand-roll the lock. Use the shared
`soroban_reentrancy_guard` crate — a lib-only crate (like `soroban_access_control` and
`soroban_pausable`) that is generic over the caller's `DataKey` and `ContractError`, so it
carries no `#[contractimpl]` entry points and links cleanly into contract wasm.

- Store the lock flag in **instance** storage under a `DataKey::Reentrancy` variant.
- Reserve a `ReentrancyDetected` error variant. As with all discriminants, never renumber or
  reuse it once shipped.
- Follow the canonical body order: all state writes complete **before** the external call, and
  the external call is wrapped by the guard.

### Two shapes

- **Explicit** (`enter` / `exit`) — for straight-line flows. Bind thin adapters:

  ```rust
  fn enter_nonreentrant(env: &Env) -> Result<(), ContractError> {
      soroban_reentrancy_guard::enter(env, &DataKey::Reentrancy, ContractError::ReentrancyDetected)
  }
  fn exit_nonreentrant(env: &Env) {
      soroban_reentrancy_guard::exit(env, &DataKey::Reentrancy);
  }
  ```

- **Scoped / RAII** (`Scoped`) — releases the lock on drop, so early returns can't leak it:

  ```rust
  fn reentrancy_scope(env: &Env) -> Result<Scoped<'_, DataKey>, ContractError> {
      Scoped::new(env, DataKey::Reentrancy, ContractError::ReentrancyDetected)
  }
  // at the call site:
  let _guard = reentrancy_scope(&env)?;
  ```

`deal_escrow` and `staking_pool` use the explicit shape; `vesting_schedule` and
`inspector_bond` use the scoped shape. Pick whichever matches the surrounding code.

