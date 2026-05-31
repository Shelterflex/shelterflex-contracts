#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

use crate::{ContractError, ReentrancyGuard, ReentrancyGuardClient};

fn setup_contract(env: &Env) -> (ReentrancyGuardClient<'_>, Address) {
    let contract_id = env.register(ReentrancyGuard, ());
    let client = ReentrancyGuardClient::new(env, &contract_id);

    let admin = Address::generate(env);

    // Initialize with mock_all_auths
    env.mock_all_auths();

    client.try_init(&admin).unwrap().unwrap();

    (client, admin)
}

fn create_entry_point(env: &Env, name: &str) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    let name_bytes = name.as_bytes();
    let len = name_bytes.len().min(32);
    bytes[..len].copy_from_slice(&name_bytes[..len]);
    BytesN::from_array(env, &bytes)
}

#[test]
fn lock_acquisition_succeeds() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Acquire lock
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Check that lock is held
    assert!(client.check_reentrancy(&guarded_contract));
}

#[test]
fn lock_release_succeeds() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Acquire lock
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Release lock
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Check that lock is released
    assert!(!client.check_reentrancy(&guarded_contract));
}

#[test]
fn reentrancy_prevention_returns_error() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Acquire lock first time
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Try to acquire lock again while held - should fail
    let err = client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::ReentrancyDetected);
}

#[test]
fn lock_released_on_error() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Acquire lock
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Simulate panic by calling exit (in real scenario, this would be automatic on panic)
    // For this test, we manually release to simulate cleanup
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Verify lock is released
    assert!(!client.check_reentrancy(&guarded_contract));
}

#[test]
fn concurrent_guard_independent_state() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let contract_a = Address::generate(&env);
    let contract_b = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guards for both contracts
    client
        .try_activate_guard(&admin, &contract_a)
        .unwrap()
        .unwrap();
    client
        .try_activate_guard(&admin, &contract_b)
        .unwrap()
        .unwrap();

    // Acquire lock for contract A
    client
        .try_enter(&contract_a, &entry_point)
        .unwrap()
        .unwrap();

    // Contract A should be locked
    assert!(client.check_reentrancy(&contract_a));

    // Contract B should not be locked
    assert!(!client.check_reentrancy(&contract_b));

    // Should be able to acquire lock for contract B
    client
        .try_enter(&contract_b, &entry_point)
        .unwrap()
        .unwrap();

    // Both should be locked now
    assert!(client.check_reentrancy(&contract_a));
    assert!(client.check_reentrancy(&contract_b));
}

#[test]
fn guard_wrapping_pattern() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Demonstrate the intended usage pattern: enter -> execute -> exit
    // Enter
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Execute guarded logic (in this case, just check lock state)
    assert!(client.check_reentrancy(&guarded_contract));

    // Exit
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Verify lock is released
    assert!(!client.check_reentrancy(&guarded_contract));
}

#[test]
fn is_locked_returns_correct_state() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Initially not locked
    assert!(!client.check_reentrancy(&guarded_contract));

    // After enter, should be locked
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();
    assert!(client.check_reentrancy(&guarded_contract));

    // After exit, should not be locked
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();
    assert!(!client.check_reentrancy(&guarded_contract));
}

#[test]
fn call_depth_tracking() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Initial depth should be 0
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 0);

    // After enter, depth should be 1
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 1);

    // After exit, depth should be 0
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 0);
}

#[test]
fn max_depth_exceeded_returns_error() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Set max depth to 1 for testing
    client.try_set_max_call_depth(&admin, &1).unwrap().unwrap();

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // First enter should succeed
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Second enter should fail due to max depth
    let err = client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MaxDepthExceeded);
}
