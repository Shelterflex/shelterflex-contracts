# Staking Pool Contract

A Soroban smart contract for staking USDC tokens with pause functionality for emergencies.

## Overview

The Staking Pool contract allows users to stake and unstake USDC tokens while providing administrators with emergency pause capabilities. This is an MVP implementation focused on core staking functionality without reward distribution.

## Features

- **Staking**: Users can stake USDC tokens
- **Unstaking**: Users can unstake their tokens
- **Emergency Controls**: Admin can pause/unpause the contract
- **Token Integration**: Uses Soroban token interface for secure transfers
- **Event Emission**: Comprehensive events for indexers and monitoring

## Contract Interface

### Initialization

```rust
init(admin: Address, token: Address)
```
- `admin`: Address of the contract administrator
- `token`: Address of the USDC token contract
- Can only be called once
- Emits: `("init", admin)`

### Core Functions

#### stake
```rust
stake(from: Address, amount: i128)
```
- `from`: Address of the user staking tokens
- `amount`: Amount of tokens to stake (must be positive)
- Requires: `from.require_auth()`
- Requires: Contract not paused
- Transfers tokens from user to contract
- Emits: `("stake", user)` with data: `(amount, new_user_balance, new_total)`

#### unstake
```rust
unstake(to: Address, amount: i128)
```
- `to`: Address of the user unstaking tokens
- `amount`: Amount of tokens to unstake (must be positive)
- Requires: `to.require_auth()`
- Requires: Contract not paused
- Requires: Sufficient staked balance
- Transfers tokens from contract to user
- Emits: `("unstake", user)` with data: `(amount, new_user_balance, new_total)`

#### staked_balance
```rust
staked_balance(user: Address) -> i128
```
- `user`: Address to query
- Returns: Amount of tokens staked by the user

#### total_staked
```rust
total_staked() -> i128
```
- Returns: Total amount of tokens staked in the contract

### Admin Functions

#### pause
```rust
pause()
```
- Requires: Admin authorization
- Pauses all staking/unstaking operations
- Emits: `("pause",)`

#### unpause
```rust
unpause()
```
- Requires: Admin authorization
- Resumes normal contract operations
- Emits: `("unpause",)`

#### is_paused
```rust
is_paused() -> bool
```
- Returns: Current pause state of the contract

## Event Shapes

### Stake Event
```
Topics: ["stake", user_address]
Data: [amount, new_user_balance, new_total]
```

### Unstake Event
```
Topics: ["unstake", user_address]
Data: [amount, new_user_balance, new_total]
```

### Pause Event
```
Topics: ["pause"]
Data: []
```

### Unpause Event
```
Topics: ["unpause"]
Data: []
```

### Init Event
```
Topics: ["init"]
Data: [admin_address]
```

## Security Features

- **Authorization**: All operations require proper user authentication
- **Admin Controls**: Only admin can pause/unpause the contract
- **Input Validation**: All amounts must be positive
- **Balance Checks**: Unstake operations validate sufficient balance
- **Pause Protection**: Critical operations are blocked when paused

## Data Storage

The contract uses instance storage for:
- `Admin`: Administrator address
- `Token`: USDC token contract address
- `StakedBalances`: Map of user addresses to staked amounts
- `TotalStaked`: Total amount staked across all users
- `Paused`: Contract pause state

## Usage Example

```rust
// Initialize contract
staking_pool.init(admin_address, usdc_token_address);

// Stake tokens (user must approve token transfer first)
staking_pool.stake(user_address, 1000i128);

// Check balances
let user_balance = staking_pool.staked_balance(user_address);
let total_balance = staking_pool.total_staked();

// Unstake tokens
staking_pool.unstake(user_address, 500i128);

// Emergency pause (admin only)
staking_pool.pause();

// Resume operations (admin only)
staking_pool.unpause();
```

## Testing

Run the comprehensive test suite:

```bash
cargo test
```

Test coverage includes:
- Happy path stake/unstake operations
- Insufficient balance scenarios
- Pause/unpause functionality
- Authorization controls
- Event emission verification
- Edge cases and error conditions

## Requirements

- Soroban SDK v22.0.7
- Rust 2021 edition
- Token contract implementing Soroban token interface

## Notes

- This is an MVP implementation without reward distribution
- Rewards are planned for a separate issue/implementation
- Contract uses instance storage for data persistence
- All token amounts use i128 type and must be positive
