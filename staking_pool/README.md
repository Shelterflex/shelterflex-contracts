# Staking Pool Contract

A Soroban smart contract for staking USDC tokens with pause functionality for emergencies.

## Overview

The Staking Pool contract allows users to stake and unstake USDC tokens while providing administrators with emergency pause capabilities. This is an MVP implementation focused on core staking functionality without reward distribution.

## Features

- **Staking**: Users can stake USDC tokens
- **Unstaking**: Users can unstake their tokens (with optional lock period)
- **Lock Periods**: Optional time-based lockups to stabilize liquidity
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
- Requires: Lock period expired (if lock period > 0)
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

### Lock Period Functions

#### set_lock_period
```rust
set_lock_period(seconds: u64)
```
- `seconds`: Lock period duration in seconds (0 = no lock period)
- Requires: Admin authorization
- Emits: `("set_lock_period",)` with data: `(seconds)`

#### get_lock_period
```rust
get_lock_period() -> u64
```
- Returns: Current lock period in seconds

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

### Set Lock Period Event
```
Topics: ["set_lock_period"]
Data: [lock_period_seconds]
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
- `LockPeriod`: Lock period duration in seconds (0 = no lock period)
- `StakeTimestamps`: Map of user addresses to their last stake timestamp

## Usage Example

```rust
// Initialize contract
staking_pool.init(admin_address, usdc_token_address);

// Set lock period (admin only) - 1 hour lock period
staking_pool.set_lock_period(3600u64);

// Check current lock period
let lock_period = staking_pool.get_lock_period();

// Stake tokens (user must approve token transfer first)
staking_pool.stake(user_address, 1000i128);

// Check balances
let user_balance = staking_pool.staked_balance(user_address);
let total_balance = staking_pool.total_staked();

// Unstake tokens (will fail if lock period not expired)
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

## Lock Period Behavior

- **Default**: No lock period (0 seconds) - tokens can be unstaked immediately
- **Timer Reset**: Each new stake resets the lock timer for that user's entire staked balance
- **Validation**: Unstake operations fail if `current_time < stake_timestamp + lock_period`
- **Cleanup**: Stake timestamps are removed when users fully unstake (balance becomes 0)
- **Admin Control**: Only administrators can set/change the lock period
- **Granularity**: Lock periods are specified in seconds using the ledger timestamp

### Example Lock Period Flow

1. Admin sets lock period to 1 hour (3600 seconds)
2. User stakes 100 tokens at timestamp 1000
3. User cannot unstake until timestamp 4600 (1000 + 3600)
4. User stakes 50 more tokens at timestamp 2000
5. Lock timer resets - user cannot unstake until timestamp 5600 (2000 + 3600)
6. User fully unstakes 150 tokens after timestamp 5600
7. Stake timestamp for user is cleaned up
