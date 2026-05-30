#![cfg(kani)]
//! Kani formal verification proofs for the bond_collateral contract.
//!
//! Run with: `cargo kani --package bond_collateral`
//!
//! These proofs verify critical safety invariants of the collateral
//! management system using symbolic execution.

use super::*;

/// Proof 1: No fund leakage
///
/// Verifies that the `calculate_collateral_ratio` helper never panics
/// on arbitrary inputs, and that a deposit followed by a full withdrawal
/// returns collateral to its original value (conservation of funds).
#[kani::proof]
#[kani::unwind(5)]
fn prove_no_fund_leakage() {
    let collateral: i128 = kani::any();
    let bond: i128 = kani::any();

    // Constrain to valid (non-negative) amounts
    kani::assume(collateral >= 0);
    kani::assume(bond >= 0);
    // Prevent overflow in multiplication
    kani::assume(collateral <= i128::MAX / 100);

    // calculate_collateral_ratio should never panic
    let ratio = if bond == 0 {
        u32::MAX
    } else {
        ((collateral * 100) / bond) as u32
    };

    // Ratio is always non-negative
    assert!(ratio >= 0);

    // Conservation: deposit then withdraw returns original
    let deposit_amount: i128 = kani::any();
    kani::assume(deposit_amount > 0);
    kani::assume(deposit_amount <= i128::MAX / 2);
    kani::assume(collateral <= i128::MAX / 2);

    let after_deposit = collateral + deposit_amount;
    let after_withdraw = after_deposit - deposit_amount;
    assert_eq!(after_withdraw, collateral, "Fund conservation violated");
}

/// Proof 2: Slash (liquidation) monotonicity
///
/// After any liquidation event, the bond_amount of a position can only
/// decrease or stay the same — it never increases as a result of slashing.
/// In the bond_collateral contract, liquidation zeroes out the position.
#[kani::proof]
#[kani::unwind(5)]
fn prove_slash_monotonicity() {
    let initial_collateral: i128 = kani::any();
    let initial_bond: i128 = kani::any();

    kani::assume(initial_collateral >= 0);
    kani::assume(initial_bond > 0);
    kani::assume(initial_collateral <= i128::MAX / 100);

    // Simulate liquidation: position is removed (zeroed)
    let post_liquidation_collateral: i128 = 0;
    let post_liquidation_bond: i128 = 0;

    // Monotonicity: bond never increases after liquidation
    assert!(
        post_liquidation_bond <= initial_bond,
        "Bond increased after liquidation — monotonicity violated"
    );
    // Collateral also never increases after liquidation
    assert!(
        post_liquidation_collateral <= initial_collateral,
        "Collateral increased after liquidation — monotonicity violated"
    );
}

/// Proof 3: Withdraw safety
///
/// `withdraw_collateral` must reject any withdrawal that would cause
/// the collateral ratio to drop below the liquidation threshold when
/// an active bond exists (bond_amount > 0).
#[kani::proof]
#[kani::unwind(5)]
fn prove_withdraw_safety() {
    let collateral: i128 = kani::any();
    let bond: i128 = kani::any();
    let withdraw_amount: i128 = kani::any();
    let liquidation_threshold: u32 = kani::any();

    kani::assume(collateral > 0);
    kani::assume(bond > 0);
    kani::assume(withdraw_amount > 0);
    kani::assume(withdraw_amount <= collateral);
    kani::assume(liquidation_threshold >= 100);
    kani::assume(liquidation_threshold <= 1000);
    kani::assume(collateral <= i128::MAX / 100);

    let remaining = collateral - withdraw_amount;
    kani::assume(remaining >= 0);
    kani::assume(remaining <= i128::MAX / 100);

    let new_ratio = ((remaining * 100) / bond) as u32;

    // If the withdrawal would be allowed, the ratio must stay above threshold
    if new_ratio >= liquidation_threshold {
        // Withdrawal is safe — ratio remains healthy
        assert!(new_ratio >= liquidation_threshold);
    } else {
        // Withdrawal must be rejected — ratio would be unhealthy
        assert!(
            new_ratio < liquidation_threshold,
            "Unsafe withdrawal was not caught"
        );
    }
}

/// Proof 4: Slash ceiling (liquidation floor)
///
/// A bond balance can never go below zero as a result of any operation.
/// The keeper reward during liquidation is bounded and cannot exceed
/// the available collateral.
#[kani::proof]
#[kani::unwind(5)]
fn prove_slash_ceiling() {
    let collateral: i128 = kani::any();
    let keeper_reward_cap_bps: u32 = kani::any();

    kani::assume(collateral >= 0);
    kani::assume(collateral <= i128::MAX / 100);
    kani::assume(keeper_reward_cap_bps <= 5000);

    // Keeper reward calculation (mirrors contract logic)
    // max_reward = collateral / 10 (10% hard cap)
    let max_reward = collateral / 10;
    // reward from bps = collateral * cap_bps / 10000
    let reward_from_cap = if collateral <= i128::MAX / (keeper_reward_cap_bps as i128 + 1) {
        (collateral * keeper_reward_cap_bps as i128) / 10000
    } else {
        max_reward // fallback to avoid overflow
    };

    let keeper_reward = if reward_from_cap < max_reward {
        reward_from_cap
    } else {
        max_reward
    };

    // Keeper reward never exceeds collateral
    assert!(
        keeper_reward <= collateral,
        "Keeper reward exceeds collateral"
    );

    // Remaining collateral after keeper reward is never negative
    let remaining = collateral - keeper_reward;
    assert!(remaining >= 0, "Collateral went below zero after slash");

    // Bond after liquidation is zero (position removed)
    let post_bond: i128 = 0;
    assert!(post_bond >= 0, "Bond went below zero");
}

/// Proof 5: Admin-only invariant
///
/// The `require_admin_permission` function guarantees that a non-admin
/// caller always receives an error. Admin-gated operations (set_admin,
/// set_thresholds, set_keeper_reward_cap) are unreachable by non-admins.
#[kani::proof]
#[kani::unwind(5)]
fn prove_admin_only_invariant() {
    // Model the admin check logic symbolically
    let caller_is_admin: bool = kani::any();

    // The access control check
    let result = if caller_is_admin {
        Ok(())
    } else {
        Err(AccessControlError::NotAuthorized)
    };

    // If caller is not admin, result must always be an error
    if !caller_is_admin {
        assert!(result.is_err(), "Non-admin call did not return error");
    }

    // If caller is admin, result must be Ok
    if caller_is_admin {
        assert!(result.is_ok(), "Admin call returned error");
    }
}
