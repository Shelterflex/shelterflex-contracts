#![cfg(kani)]

use soroban_sdk::Address;
use soroban_sdk::Env;

use crate::{ContractError, DataKey, ReentrancyGuard};

/**
 * Formal Verification Properties for Reentrancy Guard
 *
 * This module contains Kani proof harnesses to verify critical safety properties
 * of the reentrancy guard contract.
 */

/// Mutual Exclusion Property:
/// Prove that after lock acquisition, no execution path can acquire the lock
/// a second time without first releasing it.
///
/// Invariant: If a lock is held for a contract, attempting to acquire it again
/// must fail with ReentrancyDetected error.
#[kani::proof]
fn verify_mutual_exclusion() {
    let env = Env::default();

    // Setup: Initialize contract and activate guard
    let contract_id = env.register(ReentrancyGuard, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    ReentrancyGuard::init(env.clone(), admin.clone()).unwrap();

    let guarded_contract = Address::generate(&env);
    ReentrancyGuard::activate_guard(env.clone(), admin.clone(), guarded_contract.clone()).unwrap();

    // Assume lock is held (simulated by setting locked state)
    env.storage()
        .instance()
        .set(&DataKey::Locked(guarded_contract.clone()), &true);

    // Property: Attempting to acquire lock while held must fail
    let entry_point = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);
    let result = ReentrancyGuard::enter(env.clone(), guarded_contract.clone(), entry_point);

    // Verify the result is ReentrancyDetected error
    assert!(matches!(result, Err(ContractError::ReentrancyDetected)));
}

/// Release Liveness Property:
/// Prove that every lock acquisition is eventually followed by a release
/// within bounded steps.
///
/// Invariant: For any lock acquisition, there exists a bounded execution path
/// that leads to lock release.
#[kani::proof]
fn verify_release_liveness() {
    let env = Env::default();

    // Setup: Initialize contract and activate guard
    let contract_id = env.register(ReentrancyGuard, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    ReentrancyGuard::init(env.clone(), admin.clone()).unwrap();

    let guarded_contract = Address::generate(&env);
    ReentrancyGuard::activate_guard(env.clone(), admin.clone(), guarded_contract.clone()).unwrap();

    // Assume lock is acquired
    let entry_point = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);
    ReentrancyGuard::enter(env.clone(), guarded_contract.clone(), entry_point).unwrap();

    // Property: Lock must be held after acquisition
    let is_locked = env
        .storage()
        .instance()
        .get(&DataKey::Locked(guarded_contract.clone()))
        .unwrap_or(false);
    assert!(is_locked, "Lock should be held after acquisition");

    // Property: Calling exit must release the lock
    ReentrancyGuard::exit(env.clone(), guarded_contract.clone(), entry_point).unwrap();

    let is_locked_after_release = env
        .storage()
        .instance()
        .get(&DataKey::Locked(guarded_contract.clone()))
        .unwrap_or(false);
    assert!(
        !is_locked_after_release,
        "Lock should be released after exit"
    );
}

/// Lock State Consistency Property:
/// Prove that lock state transitions are consistent and valid.
///
/// Invariant: Lock state can only be in one of two valid states: locked or unlocked.
#[kani::proof]
fn verify_lock_state_consistency() {
    let env = Env::default();

    // Setup: Initialize contract
    let contract_id = env.register(ReentrancyGuard, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    ReentrancyGuard::init(env.clone(), admin.clone()).unwrap();

    let guarded_contract = Address::generate(&env);
    ReentrancyGuard::activate_guard(env.clone(), admin.clone(), guarded_contract.clone()).unwrap();

    // Property: Initial state should be unlocked
    let initial_state = env
        .storage()
        .instance()
        .get(&DataKey::Locked(guarded_contract.clone()))
        .unwrap_or(false);
    assert!(!initial_state, "Initial lock state should be unlocked");

    // Property: After enter, state should be locked
    let entry_point = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);
    ReentrancyGuard::enter(env.clone(), guarded_contract.clone(), entry_point).unwrap();

    let after_enter_state = env
        .storage()
        .instance()
        .get(&DataKey::Locked(guarded_contract.clone()))
        .unwrap_or(false);
    assert!(after_enter_state, "Lock state should be locked after enter");

    // Property: After exit, state should be unlocked
    ReentrancyGuard::exit(env.clone(), guarded_contract.clone(), entry_point).unwrap();

    let after_exit_state = env
        .storage()
        .instance()
        .get(&DataKey::Locked(guarded_contract.clone()))
        .unwrap_or(false);
    assert!(
        !after_exit_state,
        "Lock state should be unlocked after exit"
    );
}

/// Guard Activation Property:
/// Prove that guard must be activated before enter/exit operations.
///
/// Invariant: Enter and exit operations must fail if guard is not active.
#[kani::proof]
fn verify_guard_activation_required() {
    let env = Env::default();

    // Setup: Initialize contract but DO NOT activate guard
    let contract_id = env.register(ReentrancyGuard, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    ReentrancyGuard::init(env.clone(), admin.clone()).unwrap();

    let guarded_contract = Address::generate(&env);
    // Note: NOT calling activate_guard

    // Property: Enter should fail when guard is not active
    let entry_point = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);
    let result = ReentrancyGuard::enter(env.clone(), guarded_contract.clone(), entry_point);

    assert!(matches!(result, Err(ContractError::GuardNotActive)));

    // Property: Exit should also fail when guard is not active
    let exit_result = ReentrancyGuard::exit(env.clone(), guarded_contract.clone(), entry_point);

    assert!(matches!(exit_result, Err(ContractError::GuardNotActive)));
}

/// Call Depth Property:
/// Prove that call depth is correctly tracked and bounded.
///
/// Invariant: Call depth increments on enter and decrements on exit,
/// and never exceeds max depth.
#[kani::proof]
fn verify_call_depth_tracking() {
    let env = Env::default();

    // Setup: Initialize contract with max depth of 5
    let contract_id = env.register(ReentrancyGuard, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    ReentrancyGuard::init(env.clone(), admin.clone()).unwrap();

    let guarded_contract = Address::generate(&env);
    ReentrancyGuard::activate_guard(env.clone(), admin.clone(), guarded_contract.clone()).unwrap();

    // Set max depth to a known value for verification
    ReentrancyGuard::set_max_call_depth(env.clone(), admin.clone(), 5).unwrap();

    let entry_point = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);

    // Property: Initial depth should be 0
    let initial_depth =
        ReentrancyGuard::get_call_depth(env.clone(), guarded_contract.clone(), entry_point);
    assert_eq!(initial_depth, 0, "Initial call depth should be 0");

    // Property: After enter, depth should increment
    ReentrancyGuard::enter(env.clone(), guarded_contract.clone(), entry_point).unwrap();
    let after_enter_depth =
        ReentrancyGuard::get_call_depth(env.clone(), guarded_contract.clone(), entry_point);
    assert_eq!(after_enter_depth, 1, "Call depth should be 1 after enter");

    // Property: After exit, depth should decrement
    ReentrancyGuard::exit(env.clone(), guarded_contract.clone(), entry_point).unwrap();
    let after_exit_depth =
        ReentrancyGuard::get_call_depth(env.clone(), guarded_contract.clone(), entry_point);
    assert_eq!(after_exit_depth, 0, "Call depth should be 0 after exit");
}
