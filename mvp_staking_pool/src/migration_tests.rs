use super::migration_test_helpers::{
    seed_contract_with_data, setup_test_contract, test_scenarios, verify_state_integrity,
    verify_storage_integrity, ContractStateSnapshot, EXTREME_STAKE_VALUE, MAX_CAPACITY_USERS,
};
use soroban_sdk::{testutils::EnvTestConfig, Env};

fn migration_env() -> Env {
    Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: true,
    })
}

#[test]
fn empty_migration_scenario_is_stable() {
    let env = migration_env();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_empty_contract_scenario(&env);

    let report = verify_storage_integrity(&env, &contract.contract_id, &scenario);

    assert!(report.passed, "unexpected integrity issues for empty state");
    assert_eq!(report.verification_time, env.ledger().timestamp());
    assert_eq!(report.total_users_checked, 0);
    assert_eq!(report.total_stake_verified, 0);
    assert_eq!(contract.client.contract_version(), 2);
}

#[test]
fn migration_max_capacity_scenario_preserves_storage_invariants() {
    let env = migration_env();
    env.cost_estimate().budget().reset_unlimited();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_max_capacity_scenario(&env);

    seed_contract_with_data(&env, &contract, &scenario);

    let report = verify_storage_integrity(&env, &contract.contract_id, &scenario);

    assert!(report.passed, "max-capacity integrity report should pass");
    assert_eq!(report.total_users_checked, MAX_CAPACITY_USERS);
    assert_eq!(report.total_stake_verified, scenario.total_staked);
    assert_eq!(contract.client.total_staked(), scenario.total_staked);
}

#[test]
fn migration_edge_case_scenario_preserves_zero_and_extreme_stakes() {
    let env = migration_env();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_edge_case_scenario(&env);

    seed_contract_with_data(&env, &contract, &scenario);

    let report = verify_storage_integrity(&env, &contract.contract_id, &scenario);

    assert!(report.passed, "edge-case integrity report should pass");

    let mut saw_zero_balance = false;
    let mut saw_extreme_balance = false;

    for user in scenario.users.iter() {
        let expected_stake = scenario.stakes.get(user.clone()).unwrap_or(0);
        let actual_stake = contract.client.staked_balance(&user);

        if expected_stake == 0 {
            saw_zero_balance = true;
            assert_eq!(actual_stake, 0);
        }

        if expected_stake == EXTREME_STAKE_VALUE {
            saw_extreme_balance = true;
            assert_eq!(actual_stake, EXTREME_STAKE_VALUE);
        }
    }

    assert!(
        saw_zero_balance,
        "edge-case scenario must include a zero balance"
    );
    assert!(
        saw_extreme_balance,
        "edge-case scenario must include an extreme stake value"
    );
}

#[test]
fn storage_integrity_reports_total_mismatches_with_debug_details() {
    let env = migration_env();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_multi_user_scenario(&env, 4);

    seed_contract_with_data(&env, &contract, &scenario);

    let mut corrupted = scenario.clone();
    corrupted.total_staked += 1;

    let report = verify_storage_integrity(&env, &contract.contract_id, &corrupted);

    assert!(!report.passed);
    assert!(!report.issues.is_empty());
    assert_eq!(report.total_users_checked, 4);
}

#[test]
fn state_snapshot_captures_before_and_after_migration() {
    let env = migration_env();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_multi_user_scenario(&env, 3);

    let before = ContractStateSnapshot::capture(&env, &contract.contract_id, &scenario.users);
    assert_eq!(before.total_staked, 0);

    seed_contract_with_data(&env, &contract, &scenario);

    let after = ContractStateSnapshot::capture(&env, &contract.contract_id, &scenario.users);
    assert_eq!(after.total_staked, scenario.total_staked);

    let report = verify_state_integrity(&env, &contract.contract_id, &after);
    assert!(report.passed, "post-migration state integrity check failed");
    assert_eq!(report.total_users_checked, 3);
    assert_eq!(report.total_stake_verified, scenario.total_staked);
}

#[test]
fn migration_helper_is_reusable_across_seeded_scenarios() {
    let env = migration_env();
    let contract = setup_test_contract(&env);
    let scenario = test_scenarios::create_single_user_scenario(&env);

    seed_contract_with_data(&env, &contract, &scenario);

    let first_report = verify_storage_integrity(&env, &contract.contract_id, &scenario);
    let second_report = verify_storage_integrity(&env, &contract.contract_id, &scenario);

    assert!(first_report.passed);
    assert!(second_report.passed);
    assert_eq!(first_report.verification_time, env.ledger().timestamp());
    assert_eq!(second_report.verification_time, env.ledger().timestamp());
    assert_eq!(
        first_report.total_stake_verified,
        second_report.total_stake_verified
    );
    assert_eq!(
        first_report.total_users_checked,
        second_report.total_users_checked
    );
}
