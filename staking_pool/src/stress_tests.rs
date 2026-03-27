#![cfg(test)]

//! Comprehensive stress testing framework for staking pool contract
//! 
//! Tests include:
//! - High-frequency operations
//! - Boundary condition testing
//! - Resource exhaustion testing
//! - Performance benchmarking

use super::{StakingPool, StakingPoolClient};
use soroban_sdk::testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, Env, IntoVal};

fn setup_contract(env: &Env) -> (Address, StakingPoolClient<'_>, Address, Address) {
    let contract_id = env.register(StakingPool, ());
    let client = StakingPoolClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let token_admin = Address::generate(env);

    // Create token contract
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_contract_id = token_contract.address();

    // Initialize contract
    client.try_init(&admin, &token_contract_id).unwrap().unwrap();

    // Mint tokens to token_admin for distribution
    token_contract.mint(&token_admin, &1_000_000_000_000i128);

    (contract_id, client, admin, token_contract_id)
}

// ============================================================================
// High-Frequency Operations Tests
// ============================================================================

#[test]
fn stress_test_multiple_sequential_stakes() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let token_admin = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Create 50 users and perform sequential stakes
    let num_users = 50;
    let stake_amount = 1000i128;
    
    for i in 0..num_users {
        let user = Address::generate(&env);
        
        // Mint tokens to user
        token_contract.mint(&user, &stake_amount);
        
        // Mock auth for stake
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), stake_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        
        // Stake
        client.try_stake(&user, &stake_amount).unwrap().unwrap();
        
        // Verify balance
        assert_eq!(client.staked_balance(&user), stake_amount);
    }
    
    // Verify total staked
    assert_eq!(client.total_staked(), stake_amount * num_users);
}

#[test]
fn stress_test_rapid_stake_unstake_cycles() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Mint large amount to user
    let initial_amount = 100_000i128;
    token_contract.mint(&user, &initial_amount);
    
    // Perform 20 stake/unstake cycles
    let cycles = 20;
    let cycle_amount = 1000i128;
    
    for _ in 0..cycles {
        // Stake
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), cycle_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_stake(&user, &cycle_amount).unwrap().unwrap();
        
        // Unstake
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), cycle_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_unstake(&user, &cycle_amount).unwrap().unwrap();
    }
    
    // Verify final state
    assert_eq!(client.staked_balance(&user), 0);
    assert_eq!(client.total_staked(), 0);
}

// ============================================================================
// Boundary Condition Tests
// ============================================================================

#[test]
fn stress_test_maximum_stake_amount() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Test with very large amount (near i128 max)
    let max_amount = i128::MAX / 2; // Use half to avoid overflow in total
    token_contract.mint(&user, &max_amount);
    
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "stake",
            args: (user.clone(), max_amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    
    client.try_stake(&user, &max_amount).unwrap().unwrap();
    assert_eq!(client.staked_balance(&user), max_amount);
}

#[test]
fn stress_test_minimum_stake_amount() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Test with minimum valid amount (1)
    let min_amount = 1i128;
    token_contract.mint(&user, &min_amount);
    
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "stake",
            args: (user.clone(), min_amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    
    client.try_stake(&user, &min_amount).unwrap().unwrap();
    assert_eq!(client.staked_balance(&user), min_amount);
}

#[test]
fn stress_test_many_small_stakes() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Perform 100 stakes of 1 token each
    let num_stakes = 100;
    let stake_amount = 1i128;
    
    token_contract.mint(&user, &(stake_amount * num_stakes));
    
    for _ in 0..num_stakes {
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), stake_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        
        client.try_stake(&user, &stake_amount).unwrap().unwrap();
    }
    
    assert_eq!(client.staked_balance(&user), stake_amount * num_stakes);
}

// ============================================================================
// Resource Exhaustion Tests
// ============================================================================

#[test]
fn stress_test_many_concurrent_users() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Create 100 users with stakes
    let num_users = 100;
    let stake_amount = 1000i128;
    
    for _ in 0..num_users {
        let user = Address::generate(&env);
        token_contract.mint(&user, &stake_amount);
        
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), stake_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        
        client.try_stake(&user, &stake_amount).unwrap().unwrap();
    }
    
    // Verify total
    assert_eq!(client.total_staked(), stake_amount * num_users);
}

#[test]
fn stress_test_lock_period_boundary() {
    let env = Env::default();
    let (contract_id, client, admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    // Set lock period to 1000 seconds
    let lock_period = 1000u64;
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "set_lock_period",
            args: (admin.clone(), lock_period).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.try_set_lock_period(&admin, &lock_period).unwrap().unwrap();
    
    // Stake
    let stake_amount = 1000i128;
    token_contract.mint(&user, &stake_amount);
    
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "stake",
            args: (user.clone(), stake_amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.try_stake(&user, &stake_amount).unwrap().unwrap();
    
    // Try to unstake immediately (should fail)
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "unstake",
            args: (user.clone(), stake_amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    
    let err = client.try_unstake(&user, &stake_amount).unwrap_err().unwrap();
    assert_eq!(err, super::ContractError::TokensLocked);
    
    // Advance time to exactly lock_period
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + lock_period;
    });
    
    // Now unstake should succeed
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "unstake",
            args: (user.clone(), stake_amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.try_unstake(&user, &stake_amount).unwrap().unwrap();
    
    assert_eq!(client.staked_balance(&user), 0);
}

// ============================================================================
// Performance Benchmarking Tests
// ============================================================================

#[test]
fn benchmark_stake_operation() {
    let env = Env::default();
    let (contract_id, client, _admin, token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    let token_contract = soroban_sdk::token::Client::new(&env, &token_id);
    
    let stake_amount = 1000i128;
    token_contract.mint(&user, &(stake_amount * 10));
    
    // Perform 10 stakes and measure (simulated benchmark)
    for _ in 0..10 {
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), stake_amount).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        
        client.try_stake(&user, &stake_amount).unwrap().unwrap();
    }
    
    // Verify operations completed successfully
    assert_eq!(client.staked_balance(&user), stake_amount * 10);
}

#[test]
fn benchmark_query_operations() {
    let env = Env::default();
    let (_contract_id, client, _admin, _token_id) = setup_contract(&env);
    
    let user = Address::generate(&env);
    
    // Perform 100 balance queries
    for _ in 0..100 {
        let _ = client.staked_balance(&user);
    }
    
    // Perform 100 total staked queries
    for _ in 0..100 {
        let _ = client.total_staked();
    }
    
    // Perform 100 is_paused queries
    for _ in 0..100 {
        let _ = client.is_paused();
    }
    
    // All queries should complete without error
}
