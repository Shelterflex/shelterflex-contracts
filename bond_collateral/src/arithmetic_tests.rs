//! Arithmetic overflow/underflow tests: verify that monetary operations return
//! typed errors instead of trapping on overflow/underflow.

extern crate std;

use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

fn setup<'a>() -> (Env, Address, BondCollateralClient<'a>) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let token = Address::generate(&env);

    let bond_id = env.register(BondCollateral, ());
    let bond = BondCollateralClient::new(&env, &bond_id);
    bond.init(&admin, &token);

    (env, owner, bond)
}

fn position_id(env: &Env, seed: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    BytesN::from_array(env, &bytes)
}

#[test]
fn deposit_collateral_overflow_returns_amount_overflow() {
    let (env, owner, bond) = setup();
    let id = position_id(&env, 1);

    // Deposit maximum amount (creates position automatically)
    bond.deposit_collateral(&owner, &id, &i128::MAX);

    // Attempting to deposit more should return AmountOverflow
    let result = bond.try_deposit_collateral(&owner, &id, &1);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::AmountOverflow,
        "collateral deposit overflow must return AmountOverflow"
    );
}

#[test]
fn issue_bond_overflow_returns_amount_overflow() {
    let (env, owner, bond) = setup();
    let id = position_id(&env, 2);

    // Deposit collateral (creates position automatically)
    bond.deposit_collateral(&owner, &id, &10_000_000);

    // Issue maximum bond amount
    bond.issue_bond(&owner, &id, &i128::MAX);

    // Attempting to issue more should return AmountOverflow
    let result = bond.try_issue_bond(&owner, &id, &1);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::AmountOverflow,
        "bond issuance overflow must return AmountOverflow"
    );
}

#[test]
fn redeem_bond_underflow_returns_insufficient_collateral() {
    let (env, owner, bond) = setup();
    let id = position_id(&env, 3);

    // Create a position with small bond amount
    bond.deposit_collateral(&owner, &id, &10_000);
    bond.issue_bond(&owner, &id, &5_000);

    // Attempting to redeem more than issued should return InsufficientCollateral
    let result = bond.try_redeem_bond(&owner, &id, &10_000);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::InsufficientCollateral,
        "bond redemption underflow must return InsufficientCollateral"
    );
}

#[test]
fn withdraw_collateral_underflow_returns_insufficient_collateral() {
    let (env, owner, bond) = setup();
    let id = position_id(&env, 4);

    // Deposit small collateral (creates position automatically)
    bond.deposit_collateral(&owner, &id, &5_000);

    // Attempting to withdraw more than deposited should return InsufficientCollateral
    let result = bond.try_withdraw_collateral(&owner, &id, &10_000);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::InsufficientCollateral,
        "collateral withdrawal underflow must return InsufficientCollateral"
    );
}

#[test]
fn deposit_bond_overflow_returns_amount_overflow() {
    let (env, owner, bond) = setup();
    let inspector = Address::generate(&env);

    // Deposit maximum bond amount
    bond.deposit_bond(&inspector, &i128::MAX);

    // Attempting to deposit more should return AmountOverflow
    let result = bond.try_deposit_bond(&inspector, &1);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::AmountOverflow,
        "inspector bond deposit overflow must return AmountOverflow"
    );
}

#[test]
fn withdraw_bond_underflow_returns_insufficient_bond() {
    let (env, owner, bond) = setup();
    let inspector = Address::generate(&env);

    // Deposit small bond amount
    bond.deposit_bond(&inspector, &1_000);

    // Attempting to withdraw more than deposited should return InsufficientBond
    let result = bond.try_withdraw_bond(&inspector, &10_000);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::InsufficientBond,
        "inspector bond withdrawal underflow must return InsufficientBond"
    );
}

#[test]
fn liquidation_bond_reduction_overflow_handling() {
    let (env, owner, bond) = setup();
    let admin = Address::generate(&env);
    let keeper = Address::generate(&env);
    let id = position_id(&env, 5);

    // Deposit collateral and issue bond with very large amounts
    bond.deposit_collateral(&owner, &id, &i128::MAX);
    bond.issue_bond(&owner, &id, &i128::MAX);

    // Set thresholds to force liquidation
    bond.set_thresholds(&admin, &200, &150);
    bond.set_keeper_reward_cap(&admin, &1000);

    // Mock oracle with very high price
    let oracle = Address::generate(&env);
    bond.set_oracle_feed(&admin, &oracle, &600);

    // Liquidation should handle the arithmetic without trapping
    // The checked arithmetic ensures we get a proper error if overflow occurs
    let result = bond.try_liquidate(&keeper, &id);
    // This may fail due to ratio calculation, but should not trap
    assert!(
        result.is_ok()
            || matches!(
                result.unwrap_err().unwrap(),
                ContractError::OracleStale | ContractError::PositionNotFound
            )
    );
}
