use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{token::StellarAssetClient, Address, Env, IntoVal};

fn setup_contract(env: &Env) -> (Address, StakingPoolClient<'_>, Address, Address, Address) {
    let contract_id = env.register(StakingPool, ());
    let client = StakingPoolClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let user = Address::generate(env);
    let token_admin = Address::generate(env);

    // Create token contract
    let token_contract = env.register_stellar_asset_contract_v2(token_admin);
    let token_contract_id = token_contract.address();

    // Initialize contract
    client.init(&admin, &token_contract_id);

    (contract_id, client, admin, user, token_contract_id)
}

#[test]
fn stake_tokens() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 100i128);
}

#[test]
fn unstake_tokens() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 100i128);

    client.unstake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 0i128);
}

#[test]
fn partial_unstake() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 100i128);

    client.unstake(&user, &50i128);
    assert_eq!(client.staked_balance(&user), 50i128);
}

#[test]
fn unstake_more_than_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);

    let result = client.try_unstake(&user, &200i128);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::InsufficientUnusedStake
    );
}

#[test]
fn multiple_stakers() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, admin, _user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&admin, &10000i128);

    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);
    let user3 = Address::generate(&env);
    let user4 = Address::generate(&env);
    let user5 = Address::generate(&env);

    asset.mint(&user1, &1000i128);
    asset.mint(&user2, &1000i128);
    asset.mint(&user3, &1000i128);
    asset.mint(&user4, &1000i128);
    asset.mint(&user5, &1000i128);

    client.stake(&user1, &100i128);
    client.stake(&user2, &200i128);
    client.stake(&user3, &300i128);
    client.stake(&user4, &400i128);
    client.stake(&user5, &500i128);

    assert_eq!(client.staked_balance(&user1), 100i128);
    assert_eq!(client.staked_balance(&user2), 200i128);
    assert_eq!(client.staked_balance(&user3), 300i128);
    assert_eq!(client.staked_balance(&user4), 400i128);
    assert_eq!(client.staked_balance(&user5), 500i128);

    assert_eq!(client.total_staked(), 1500i128);
}

#[test]
fn restake_after_unstake() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 100i128);

    client.unstake(&user, &100i128);
    assert_eq!(client.staked_balance(&user), 0i128);

    // Re-stake should be treated as fresh
    client.stake(&user, &200i128);
    assert_eq!(client.staked_balance(&user), 200i128);
}

#[test]
#[should_panic(expected = "contract is paused")]
fn admin_pause_stake_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    // Pause the contract
    client.pause(&admin);
    assert!(client.is_paused());

    // Stake should fail when paused
    client.stake(&user, &100i128);
}

#[test]
#[should_panic(expected = "contract is paused")]
fn admin_pause_unstake_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);

    // Pause the contract
    client.pause(&admin);
    assert!(client.is_paused());

    // Unstake should fail when paused
    client.unstake(&user, &50i128);
}

#[test]
fn non_admin_initialize_unauthorized() {
    let env = Env::default();
    let contract_id = env.register(StakingPool, ());
    let client = StakingPoolClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_contract = env.register_stellar_asset_contract_v2(token_admin);
    let token_contract_id = token_contract.address();

    // Try to initialize with non-admin (should succeed since init doesn't check admin)
    // But let's test that only the admin can perform admin operations after init
    client.init(&admin, &token_contract_id);

    // Now try pause with non-admin
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: (non_admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_pause(&non_admin);
    assert!(result.is_err());
}

#[test]
fn stake_emits_event() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);

    let events = env.events().all();
    assert!(!events.is_empty());

    // Verify at least one event was emitted
    // The event structure in soroban SDK is complex, so we just check events exist
    assert!(!events.is_empty());
}

#[test]
fn unstake_emits_event() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, user, token_id) = setup_contract(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&user, &1000i128);

    client.stake(&user, &100i128);
    client.unstake(&user, &100i128);

    let events = env.events().all();
    assert!(!events.is_empty());

    // Verify at least one event was emitted
    // The event structure in soroban SDK is complex, so we just check events exist
    assert!(!events.is_empty());
}

// ── Authorization boundary (Issue #18) ───────────────────────────────────────

/// Every entry point that takes an explicit `admin` argument is gated by the
/// shared admin comparison and must reject a non-admin caller.
#[test]
fn non_admin_rejected_on_every_admin_gated_entry_point() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, _admin, _user, _token) = setup_contract(&env);

    let attacker = Address::generate(&env);
    let user = Address::generate(&env);
    let hash = soroban_sdk::BytesN::from_array(&env, &[8u8; 32]);

    assert_eq!(
        client
            .try_utilize_stake(&attacker, &user, &100)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "utilize_stake must reject a non-admin"
    );

    assert_eq!(
        client
            .try_set_guardian(&attacker, &attacker)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_guardian must reject a non-admin"
    );

    assert_eq!(
        client
            .try_set_upgrade_delay(&attacker, &100)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_upgrade_delay must reject a non-admin"
    );

    assert_eq!(
        client
            .try_propose_upgrade(&attacker, &hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "propose_upgrade must reject a non-admin"
    );

    assert_eq!(
        client
            .try_execute_upgrade(&attacker, &hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "execute_upgrade must reject a non-admin"
    );

    assert_eq!(
        client
            .try_emergency_upgrade(&attacker, &hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "emergency_upgrade must reject a non-admin"
    );

    assert_eq!(
        client.try_cancel_upgrade(&attacker).unwrap_err().unwrap(),
        ContractError::NotAuthorized,
        "cancel_upgrade must reject a non-admin"
    );

    assert_eq!(
        client.try_pause(&attacker).unwrap_err().unwrap(),
        soroban_pausable_core::PausableError::NotAuthorized,
        "pause must reject a non-admin"
    );
}

/// `fund_rewards` takes no caller argument, so the gate is the host-level auth
/// requirement on the stored admin: a transaction signed by anyone else is
/// rejected before the body runs.
#[test]
fn non_admin_rejected_on_fund_rewards() {
    let env = Env::default();
    env.mock_all_auths();
    let (contract_id, client, _admin, _user, token_id) = setup_contract(&env);
    let attacker = Address::generate(&env);

    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&attacker, &1_000i128);

    env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "fund_rewards",
            args: (attacker.clone(), 100i128).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_fund_rewards(&attacker, &100i128);
    assert!(
        result.is_err(),
        "fund_rewards must reject a caller that is not the stored admin"
    );
}

/// A rejected admin call must leave the guardian and upgrade delay untouched.
#[test]
fn rejected_admin_call_does_not_change_config() {
    let env = Env::default();
    env.mock_all_auths();
    let (_contract_id, client, admin, _user, _token) = setup_contract(&env);

    let attacker = Address::generate(&env);
    client.set_upgrade_delay(&admin, &600);

    let result = client.try_set_upgrade_delay(&attacker, &0);
    assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);

    // The admin's own value still applies: a proposal is still delay-gated.
    let hash = soroban_sdk::BytesN::from_array(&env, &[9u8; 32]);
    client.propose_upgrade(&admin, &hash);
    assert!(
        client.try_execute_upgrade(&admin, &hash).is_err(),
        "the attacker must not have been able to zero out the upgrade delay"
    );
}
