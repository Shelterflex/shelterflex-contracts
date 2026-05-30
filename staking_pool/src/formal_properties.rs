//! Kani formal verification harnesses for `staking_pool` safety properties.
//!
//! Soroban storage and cross-contract calls are opaque to Kani, so these proofs
//! use a pure-Rust model that mirrors the arithmetic and state-transition logic
//! in `lib.rs`. Each harness below is a mechanically checked specification.

#![cfg(kani)]

use crate::validation;

// ── Model types ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelError {
    InvalidAmount,
    InsufficientBalance,
    TokensLocked,
    ReentrancyDetected,
    UpgradeDelayNotMet,
    Overflow,
}

/// Stub storage model for up to three concurrent stakers (issue #921).
struct PoolModel {
    balances: [i128; 3],
    total_staked: i128,
    lock_period: u64,
    stake_timestamps: [u64; 3],
    has_stake_timestamp: [bool; 3],
    reentrancy: bool,
    pending_upgrade_at: Option<u64>,
    upgrade_delay: u64,
    current_time: u64,
}

impl PoolModel {
    fn new() -> Self {
        Self {
            balances: [0; 3],
            total_staked: 0,
            lock_period: 0,
            stake_timestamps: [0; 3],
            has_stake_timestamp: [false; 3],
            reentrancy: false,
            pending_upgrade_at: None,
            upgrade_delay: 0,
            current_time: 0,
        }
    }

    fn sum_balances(&self) -> i128 {
        self.balances[0] + self.balances[1] + self.balances[2]
    }

    /// Mirrors `enter_nonreentrant` in `lib.rs` (#390).
    fn enter_nonreentrant(&mut self) -> Result<(), ModelError> {
        if self.reentrancy {
            return Err(ModelError::ReentrancyDetected);
        }
        self.reentrancy = true;
        Ok(())
    }

    /// Mirrors `exit_nonreentrant` in `lib.rs` (#390).
    fn exit_nonreentrant(&mut self) {
        self.reentrancy = false;
    }

    /// Mirrors the balance-update portion of `stake` (lines 451–456 in `lib.rs`).
    fn apply_stake(&mut self, user_idx: usize, amount: i128) -> Result<(), ModelError> {
        validation::require_valid_amount(amount).map_err(|_| ModelError::InvalidAmount)?;

        let new_balance = self.balances[user_idx]
            .checked_add(amount)
            .ok_or(ModelError::Overflow)?;
        let new_total = self
            .total_staked
            .checked_add(amount)
            .ok_or(ModelError::Overflow)?;

        self.balances[user_idx] = new_balance;
        self.total_staked = new_total;
        self.stake_timestamps[user_idx] = self.current_time;
        self.has_stake_timestamp[user_idx] = true;
        Ok(())
    }

    /// Mirrors the balance-check and update portion of `unstake` (lines 486–515).
    fn apply_unstake(&mut self, user_idx: usize, amount: i128) -> Result<(), ModelError> {
        validation::require_valid_amount(amount).map_err(|_| ModelError::InvalidAmount)?;

        if self.balances[user_idx] < amount {
            return Err(ModelError::InsufficientBalance);
        }

        if self.lock_period > 0 {
            if !self.has_stake_timestamp[user_idx] {
                return Err(ModelError::TokensLocked);
            }
            let stake_time = self.stake_timestamps[user_idx];
            if self.current_time < stake_time.saturating_add(self.lock_period) {
                return Err(ModelError::TokensLocked);
            }
        }

        self.balances[user_idx] -= amount;
        self.total_staked -= amount;

        if self.balances[user_idx] == 0 {
            self.has_stake_timestamp[user_idx] = false;
        }
        Ok(())
    }

    /// Mirrors the timelock gate in `execute_upgrade` (lines 669–676 in `lib.rs`).
    fn can_execute_upgrade(&self) -> Result<(), ModelError> {
        let proposed_at = self
            .pending_upgrade_at
            .ok_or(ModelError::UpgradeDelayNotMet)?;

        if self.upgrade_delay > 0 && self.current_time < proposed_at.saturating_add(self.upgrade_delay)
        {
            return Err(ModelError::UpgradeDelayNotMet);
        }
        Ok(())
    }
}

// ── Proof harnesses ───────────────────────────────────────────────────────────

/// **Property:** `total_staked + staked_amount` never overflows `i128` for valid
/// positive amounts within safe bounds.
///
/// **Why it matters:** An overflow would corrupt the global stake counter and
/// could allow withdrawal of more tokens than were deposited.
#[kani::proof]
fn stake_no_overflow() {
    let total_staked: i128 = kani::any();
    let staked_amount: i128 = kani::any();

    kani::assume(staked_amount > 0);
    kani::assume(total_staked >= 0);
    kani::assume(total_staked <= i128::MAX / 2);
    kani::assume(staked_amount <= i128::MAX / 2);

    let direct = total_staked.checked_add(staked_amount);
    assert!(direct.is_some(), "checked_add must succeed within safe bounds");

    let mut model = PoolModel::new();
    model.total_staked = total_staked;
    let result = model.apply_stake(0, staked_amount);
    assert!(result.is_ok(), "model stake must not overflow");
    assert_eq!(model.total_staked, direct.unwrap());
}

/// **Property:** `staked_balance - unstake_amount >= 0` is always maintained;
/// unstake rejects amounts exceeding the user's balance.
///
/// **Why it matters:** Underflow would let users withdraw more than they staked,
/// draining the pool at the expense of other stakers.
#[kani::proof]
fn unstake_no_underflow() {
    let balance: i128 = kani::any();
    let unstake_amount: i128 = kani::any();

    kani::assume(balance >= 0);
    kani::assume(unstake_amount > 0);

    let mut model = PoolModel::new();
    model.balances[0] = balance;
    model.total_staked = balance;

    if balance >= unstake_amount {
        let result = model.apply_unstake(0, unstake_amount);
        assert!(result.is_ok());
        assert!(model.balances[0] >= 0);
        assert_eq!(model.balances[0], balance - unstake_amount);
    } else {
        let result = model.apply_unstake(0, unstake_amount);
        assert_eq!(result, Err(ModelError::InsufficientBalance));
        assert_eq!(model.balances[0], balance);
    }
}

/// **Property:** If `stake_timestamp + lock_period > current_time`, unstake
/// always fails with `TokensLocked`.
///
/// **Why it matters:** The lock period prevents early withdrawal; bypassing it
/// would break the platform's liquidity guarantees.
#[kani::proof]
fn lock_period_enforced() {
    let stake_timestamp: u64 = kani::any();
    let lock_period: u64 = kani::any();
    let current_time: u64 = kani::any();
    let unstake_amount: i128 = kani::any();

    kani::assume(lock_period > 0);
    kani::assume(current_time < stake_timestamp.saturating_add(lock_period));
    kani::assume(unstake_amount > 0);
    kani::assume(unstake_amount <= 1_000_000_000);

    let mut model = PoolModel::new();
    model.lock_period = lock_period;
    model.current_time = current_time;
    model.balances[0] = unstake_amount;
    model.total_staked = unstake_amount;
    model.stake_timestamps[0] = stake_timestamp;
    model.has_stake_timestamp[0] = true;

    let result = model.apply_unstake(0, unstake_amount);
    assert_eq!(result, Err(ModelError::TokensLocked));
    assert_eq!(model.balances[0], unstake_amount);
}

/// **Property:** `total_staked` always equals the sum of individual staker
/// balances (verified for up to three concurrent stakers).
///
/// **Why it matters:** A mismatch between the aggregate and per-user totals
/// indicates accounting corruption that could lead to fund loss.
#[kani::proof]
#[kani::unwind(4)]
fn balance_conservation() {
    let mut model = PoolModel::new();

    let num_stakers: u8 = kani::any();
    kani::assume(num_stakers <= 3);

    for i in 0..num_stakers {
        let amount: i128 = kani::any();
        kani::assume(amount > 0);
        kani::assume(amount <= 1_000_000_000);
        kani::assume(model.total_staked <= i128::MAX - amount);

        let idx = i as usize;
        let _ = model.apply_stake(idx, amount);
        assert_eq!(
            model.total_staked,
            model.sum_balances(),
            "total must equal sum after stake"
        );
    }

    for i in 0..num_stakers {
        let unstake: i128 = kani::any();
        kani::assume(unstake > 0);
        kani::assume(unstake <= model.balances[i as usize]);

        let idx = i as usize;
        if model.apply_unstake(idx, unstake).is_ok() {
            assert_eq!(
                model.total_staked,
                model.sum_balances(),
                "total must equal sum after unstake"
            );
        }
    }

    assert_eq!(model.total_staked, model.sum_balances());
    for b in model.balances {
        assert!(b >= 0);
    }
}

/// **Property:** The reentrancy lock is set before any external call and cleared
/// afterward; a nested entry attempt is rejected.
///
/// **Why it matters:** Without this guard, a malicious token contract could
/// re-enter `stake`/`unstake` during a transfer and corrupt pool state.
#[kani::proof]
fn reentrancy_safety() {
    let attempt_reentry: bool = kani::any();
    let mut model = PoolModel::new();

    assert!(!model.reentrancy);

    model.enter_nonreentrant().unwrap();
    assert!(model.reentrancy, "lock must be set before external call");

    if attempt_reentry {
        assert_eq!(
            model.enter_nonreentrant(),
            Err(ModelError::ReentrancyDetected),
            "nested entry must be rejected"
        );
    }

    model.exit_nonreentrant();
    assert!(!model.reentrancy, "lock must be cleared after external call");
}

/// **Property:** `execute_upgrade` cannot succeed before
/// `PendingUpgradeAt + upgrade_delay` has elapsed.
///
/// **Why it matters:** The timelock gives guardians and users time to react
/// before a contract upgrade takes effect.
#[kani::proof]
fn upgrade_delay_respected() {
    let proposed_at: u64 = kani::any();
    let delay: u64 = kani::any();
    let current_time: u64 = kani::any();

    kani::assume(delay > 0);
    kani::assume(current_time < proposed_at.saturating_add(delay));

    let model = PoolModel {
        pending_upgrade_at: Some(proposed_at),
        upgrade_delay: delay,
        current_time,
        ..PoolModel::new()
    };

    assert_eq!(model.can_execute_upgrade(), Err(ModelError::UpgradeDelayNotMet));

    let elapsed_time = proposed_at.saturating_add(delay);
    let model_ready = PoolModel {
        pending_upgrade_at: Some(proposed_at),
        upgrade_delay: delay,
        current_time: elapsed_time,
        ..PoolModel::new()
    };
    assert!(model_ready.can_execute_upgrade().is_ok());
}
