//! Monthly cap tests for `RentWallet` (#1).
//!
//! Declared in lib.rs as `#[cfg(test)] mod monthly_cap_tests;` — this file
//! *is* the module, so its contents aren't wrapped in another `mod` block
//! (matches how `validation.rs` and `formal_properties.rs` structure their
//! test modules).
//!
//! Run with: cargo test -p rent_wallet monthly_cap

extern crate std;

use crate::{ContractError, RentWallet, RentWalletClient};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

// 30-day month in seconds (same constant as monthly_cap.rs)
const MONTH_SECS: u64 = 2_592_000;

fn make() -> (Env, Address, RentWalletClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(RentWallet, ());
    let client: RentWalletClient<'static> =
        unsafe { std::mem::transmute(RentWalletClient::new(&env, &contract_id)) };

    let admin = Address::generate(&env);
    let user = Address::generate(&env);

    client.try_init(&admin).unwrap().unwrap();
    env.ledger().set_timestamp(MONTH_SECS); // put us in month 1

    // Fund user
    client
        .try_credit(&admin, &user, &10_000i128)
        .unwrap()
        .unwrap();

    (env, contract_id, client, admin, user)
}

// ══════════════════════════════════════════════════════════════════════════
// Debit within cap
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn debit_within_cap_succeeds() {
    let (_env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &5_000i128)
        .unwrap()
        .unwrap();

    client
        .try_debit(&admin, &user, &4_000i128)
        .expect("debit within cap must succeed")
        .expect("inner Ok");

    assert_eq!(client.balance(&user), 6_000i128);
    assert_eq!(client.get_monthly_spent(&user), 4_000i128);
}

// ══════════════════════════════════════════════════════════════════════════
// Debit exceeds cap
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn debit_exceeding_cap_fails_with_monthly_cap_exceeded() {
    let (_env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &1_000i128)
        .unwrap()
        .unwrap();

    let err = client
        .try_debit(&admin, &user, &1_001i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);
    // Balance must be unchanged after a rejected debit
    assert_eq!(client.balance(&user), 10_000i128);
}

#[test]
fn cumulative_debits_respect_cap() {
    let (_env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &1_500i128)
        .unwrap()
        .unwrap();

    client
        .try_debit(&admin, &user, &1_000i128)
        .unwrap()
        .unwrap();
    // 1000 spent, 500 remaining
    client.try_debit(&admin, &user, &500i128).unwrap().unwrap();
    // 1500 spent — exactly at cap; next debit must fail
    let err = client
        .try_debit(&admin, &user, &1i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);
}

// ══════════════════════════════════════════════════════════════════════════
// Cap resets on new month
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn monthly_spend_resets_at_start_of_new_month() {
    let (env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &500i128)
        .unwrap()
        .unwrap();

    // Exhaust cap in month 1
    env.ledger().set_timestamp(MONTH_SECS);
    client.try_debit(&admin, &user, &500i128).unwrap().unwrap();
    let err = client
        .try_debit(&admin, &user, &1i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);

    // Advance to month 2 — cap counter resets
    env.ledger().set_timestamp(MONTH_SECS * 2 + 1);
    client
        .try_debit(&admin, &user, &500i128)
        .expect("new month should reset the spend counter")
        .expect("inner Ok");
}

#[test]
fn monthly_spend_resets_exactly_at_period_boundary() {
    // Exercises the precise instant the period key rolls over, not just
    // "sometime later in the next month": last debit of period N at
    // (N+1)*MONTH_SECS - 1, first debit of period N+1 at exactly
    // (N+1)*MONTH_SECS.
    let (env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &1_000i128)
        .unwrap()
        .unwrap();

    // Last second of period 1.
    let period_1_end = MONTH_SECS * 2 - 1;
    env.ledger().set_timestamp(period_1_end);
    client.try_debit(&admin, &user, &900i128).unwrap().unwrap();
    // Still period 1 — 900 + 200 > 1000 must fail here.
    let err = client
        .try_debit(&admin, &user, &200i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);

    // First instant of period 2 — spend counter must have reset, so the
    // same 200 that just failed now succeeds.
    let period_2_start = MONTH_SECS * 2;
    env.ledger().set_timestamp(period_2_start);
    client
        .try_debit(&admin, &user, &200i128)
        .expect("first debit of the new period must succeed")
        .expect("inner Ok");
    assert_eq!(client.get_monthly_spent(&user), 200i128);
}

#[test]
fn property_randomized_debit_sequence_never_exceeds_monthly_cap() {
    // Mirrors the LCG-based property test in lib.rs
    // (property_randomized_credit_debit_sequence_preserves_balance_invariant):
    // a random cap and a random sequence of debit amounts within a single
    // period must never let recorded spend exceed the cap once one is set.
    let (_env, _id, client, admin, user) = make();
    client
        .try_credit(&admin, &user, &1_000_000i128)
        .unwrap()
        .unwrap();

    let mut rng: u64 = 0xC0FFEE_u64;
    rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    let cap = 1 + (rng % 5_000) as i128;
    client
        .try_set_default_monthly_cap(&admin, &cap)
        .unwrap()
        .unwrap();

    let mut spent: i128 = 0;
    const OPS: u32 = 200;
    for step in 0..OPS {
        rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let amount = ((rng % 300) + 1) as i128;

        let result = client.try_debit(&admin, &user, &amount);
        if matches!(result, Ok(Ok(()))) {
            spent += amount;
        } else if let Err(Ok(ContractError::MonthlyCapExceeded)) = result {
            assert!(
                spent + amount > cap,
                "debit rejected at step {step} when it should have fit under the cap"
            );
        } else {
            panic!("unexpected debit result at step {step}");
        }

        assert!(spent <= cap, "cap invariant violated at step {step}");
        assert_eq!(
            client.get_monthly_spent(&user),
            spent,
            "spent counter drifted from locally tracked total at step {step}"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Per-user override takes precedence
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn per_user_override_cap_takes_precedence_over_default() {
    let (_env, _id, client, admin, user) = make();
    // Default cap is 500, but user has a higher override of 2_000
    client
        .try_set_default_monthly_cap(&admin, &500i128)
        .unwrap()
        .unwrap();
    client
        .try_set_user_monthly_cap(&admin, &user, &2_000i128)
        .unwrap()
        .unwrap();

    // 1_000 would fail against the 500 default but pass against the 2_000 override
    client
        .try_debit(&admin, &user, &1_000i128)
        .expect("per-user override should allow larger debit")
        .expect("inner Ok");

    assert_eq!(client.get_monthly_cap(&user), 2_000i128);
}

#[test]
fn per_user_override_can_restrict_below_default() {
    let (_env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &5_000i128)
        .unwrap()
        .unwrap();
    // Override is smaller than the default
    client
        .try_set_user_monthly_cap(&admin, &user, &100i128)
        .unwrap()
        .unwrap();

    let err = client
        .try_debit(&admin, &user, &101i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);
}

// ══════════════════════════════════════════════════════════════════════════
// Zero default cap (no cap — backward-compatible)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn zero_default_cap_means_no_cap_enforced() {
    // If default has never been set (or is 0), debits should succeed as before,
    // AND no monthly-spend bookkeeping should occur — this is the signal that
    // distinguishes "cap module wired, defaulting to unlimited" from "cap
    // module not wired at all" (the exact bug this issue reports). Without
    // the get_monthly_spent assertion, this test would pass even if debit()
    // never called into monthly_cap at all.
    let (_env, _id, client, admin, user) = make();
    // No set_default_monthly_cap call → cap is 0 → no enforcement
    client
        .try_debit(&admin, &user, &9_000i128)
        .expect("zero default cap must not block debits")
        .expect("inner Ok");
    assert_eq!(client.balance(&user), 1_000i128);

    // Now prove the cap module IS live: setting a cap after the fact must
    // start being enforced against subsequent debits.
    client
        .try_set_default_monthly_cap(&admin, &500i128)
        .unwrap()
        .unwrap();
    let err = client
        .try_debit(&admin, &user, &501i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MonthlyCapExceeded);
}

// ══════════════════════════════════════════════════════════════════════════
// get_monthly_spent and get_monthly_cap helpers
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn get_monthly_spent_is_zero_before_any_debit() {
    let (_env, _id, client, _admin, user) = make();
    assert_eq!(client.get_monthly_spent(&user), 0i128);
}

#[test]
fn get_monthly_cap_returns_default_when_no_override_set() {
    let (_env, _id, client, admin, user) = make();
    client
        .try_set_default_monthly_cap(&admin, &3_000i128)
        .unwrap()
        .unwrap();
    assert_eq!(client.get_monthly_cap(&user), 3_000i128);
}

// ══════════════════════════════════════════════════════════════════════════
// Admin-only functions
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn non_admin_cannot_set_default_monthly_cap() {
    let (env, _id, client, _admin, _user) = make();
    let rogue = Address::generate(&env);
    let err = client
        .try_set_default_monthly_cap(&rogue, &1_000i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::NotAuthorized);
}

#[test]
fn non_admin_cannot_set_user_monthly_cap() {
    let (env, _id, client, _admin, user) = make();
    let rogue = Address::generate(&env);
    let err = client
        .try_set_user_monthly_cap(&rogue, &user, &1_000i128)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::NotAuthorized);
}
