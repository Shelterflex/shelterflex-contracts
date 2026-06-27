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
fn enter_sets_call_depth() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Enter should set depth
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Check that call depth is 1
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 1);
}

#[test]
fn exit_resets_call_depth() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Enter
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Exit
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Check that call depth is back to 0
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 0);
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

    // Set max_depth to 1 for strict reentrancy prevention
    client.try_set_max_call_depth(&admin, &1).unwrap().unwrap();

    // First enter succeeds (depth 0 < max_depth 1)
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Second enter fails — depth 1 >= max_depth 1
    let err = client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MaxDepthExceeded);
}

#[test]
fn exit_restores_depth_on_cleanup() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Enter (simulates guarded call)
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // In real scenario, exit would be called automatically on panic cleanup
    // Here we manually release to simulate cleanup
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Verify depth is restored to 0
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 0);
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

    // Enter contract A
    client
        .try_enter(&contract_a, &entry_point)
        .unwrap()
        .unwrap();

    // Contract A should have depth 1
    assert_eq!(client.get_call_depth(&contract_a, &entry_point), 1);

    // Contract B should still have depth 0
    assert_eq!(client.get_call_depth(&contract_b, &entry_point), 0);

    // Should be able to enter contract B independently
    client
        .try_enter(&contract_b, &entry_point)
        .unwrap()
        .unwrap();

    // Both should have depth 1 now
    assert_eq!(client.get_call_depth(&contract_a, &entry_point), 1);
    assert_eq!(client.get_call_depth(&contract_b, &entry_point), 1);
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

    // Execute guarded logic (in this case, just check depth)
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 1);

    // Exit
    client
        .try_exit(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Verify depth is back to 0
    assert_eq!(client.get_call_depth(&guarded_contract, &entry_point), 0);
}

#[test]
fn call_depth_reflects_enter_exit_state() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let guarded_contract = Address::generate(&env);
    let entry_point = create_entry_point(&env, "transfer");

    // Activate guard
    client
        .try_activate_guard(&admin, &guarded_contract)
        .unwrap()
        .unwrap();

    // Initially depth is 0
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

    // First enter should succeed (depth 0 < max_depth 1)
    client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap()
        .unwrap();

    // Second enter should fail with MaxDepthExceeded (depth 1 >= max_depth 1)
    let err = client
        .try_enter(&guarded_contract, &entry_point)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::MaxDepthExceeded);
}
