#![cfg(test)]

use super::*;
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, Env, IntoVal};

fn setup(env: &Env) -> (Address, StakingRewardsClient<'_>) {
    env.mock_all_auths();
    let contract_id = env.register(StakingRewards, ());
    let client = StakingRewardsClient::new(env, &contract_id);

    let admin = Address::generate(env);
    client.try_init(&admin).unwrap().unwrap();

    (contract_id, client)
}

#[test]
fn single_staker_full_period() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user = Address::generate(&env);
    client.stake(&user, &1000);
    client.distribute_rewards(&500);

    assert_eq!(client.get_claimable(&user), 500);
}

#[test]
fn two_stakers_equal_stake() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);

    client.stake(&user1, &1000);
    client.stake(&user2, &1000);
    client.distribute_rewards(&1000);

    let claimable1 = client.get_claimable(&user1);
    let claimable2 = client.get_claimable(&user2);

    // Each should get 50% (allowing for 1 stroop rounding)
    assert!((claimable1 - 500).abs() <= 1);
    assert!((claimable2 - 500).abs() <= 1);
}

#[test]
fn weighted_distribution_2_1_1() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);
    let user3 = Address::generate(&env);

    client.stake(&user1, &200);
    client.stake(&user2, &100);
    client.stake(&user3, &100);
    client.distribute_rewards(&1000);

    let claimable1 = client.get_claimable(&user1);
    let claimable2 = client.get_claimable(&user2);
    let claimable3 = client.get_claimable(&user3);

    // Ratios: 2:1:1 -> 50%, 25%, 25%
    assert!((claimable1 - 500).abs() <= 1);
    assert!((claimable2 - 250).abs() <= 1);
    assert!((claimable3 - 250).abs() <= 1);
}

#[test]
fn claim_before_distribution_returns_zero() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user = Address::generate(&env);
    client.stake(&user, &1000);

    // Claim before any distribution
    let claimed = client.claim(&user);
    assert_eq!(claimed, 0);
}

#[test]
fn double_claim_returns_zero() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user = Address::generate(&env);
    client.stake(&user, &1000);
    client.distribute_rewards(&500);

    let first_claim = client.claim(&user);
    assert_eq!(first_claim, 500);

    let second_claim = client.claim(&user);
    assert_eq!(second_claim, 0);
}

#[test]
fn zero_reward_period_no_distribution() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);

    client.stake(&user1, &1000);
    client.stake(&user2, &1000);

    // Don't distribute any rewards - just check claimable is 0
    assert_eq!(client.get_claimable(&user1), 0);
    assert_eq!(client.get_claimable(&user2), 0);

    // Should not panic when claiming with no rewards
    let claimed1 = client.claim(&user1);
    let claimed2 = client.claim(&user2);
    assert_eq!(claimed1, 0);
    assert_eq!(claimed2, 0);
}

#[test]
fn unstake_before_claim_still_succeeds() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user = Address::generate(&env);
    client.stake(&user, &1000);
    client.distribute_rewards(&500);

    // Claim rewards before unstaking
    let claimed_before = client.claim(&user);
    assert_eq!(claimed_before, 500);

    // Unstake
    client.unstake(&user, &1000);

    // After unstaking, claimable should be 0 (already claimed)
    let claimed_after = client.claim(&user);
    assert_eq!(claimed_after, 0);
}

#[test]
fn reward_precision_no_dust_lost() {
    let env = Env::default();
    let (_contract_id, client) = setup(&env);

    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);
    let user3 = Address::generate(&env);

    client.stake(&user1, &333);
    client.stake(&user2, &333);
    client.stake(&user3, &334);
    client.distribute_rewards(&1000);

    let claimed1 = client.claim(&user1);
    let claimed2 = client.claim(&user2);
    let claimed3 = client.claim(&user3);

    let total_claimed = claimed1 + claimed2 + claimed3;
    assert_eq!(total_claimed, 1000);
}

#[test]
fn non_admin_cannot_distribute_rewards() {
    let env = Env::default();
    let contract_id = env.register(StakingRewards, ());
    let client = StakingRewardsClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.init(&admin);

    let non_admin = Address::generate(&env);
    let user = Address::generate(&env);

    // Mock auth for stake
    env.mock_auths(&[MockAuth {
        address: &user,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "stake",
            args: (user.clone(), 1000i128).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.stake(&user, &1000);

    // Mock auth for non-admin distribute_rewards
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "distribute_rewards",
            args: (500i128,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_distribute_rewards(&500);
    assert!(result.is_err());
}

#[test]
fn claim_rewards_when_paused() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(StakingRewards, ());
    let client = StakingRewardsClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.init(&admin);

    let user = Address::generate(&env);
    client.stake(&user, &1000);
    client.distribute_rewards(&500);

    // Pause the contract
    client.pause(&admin);

    // Claim should fail when paused
    let result = client.try_claim(&user);
    assert!(result.is_err());
    // The error should be ContractError::Paused
    match result {
        Err(Ok(err)) => assert_eq!(err, ContractError::Paused),
        _ => panic!("Expected ContractError::Paused"),
    }
}
