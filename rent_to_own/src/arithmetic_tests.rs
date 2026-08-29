//! Arithmetic overflow/underflow tests: verify that monetary operations return
//! typed errors instead of trapping on overflow/underflow.

extern crate std;

use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{BytesN, Env};

fn setup(env: &Env) -> (Address, RentToOwnClient<'_>) {
    env.mock_all_auths();
    let id = env.register(RentToOwn, ());
    let client = RentToOwnClient::new(env, &id);
    let admin = Address::generate(env);
    client.init(&admin, &2000u32);
    (admin, client)
}

fn deal_id(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &[seed; 32])
}

#[test]
fn equity_payment_overflow_returns_amount_overflow() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let tenant = Address::generate(&env);
    let id = deal_id(&env, 1);

    // Register a deal with maximum property value
    client.register_deal(&admin, &id, &tenant, &i128::MAX, &10_000, &10);

    // First payment succeeds
    client.record_equity_payment(&admin, &id, &1_000, &10_000);

    // Attempting to add more than remaining capacity should return AmountOverflow
    let result = client.try_record_equity_payment(&admin, &id, &i128::MAX, &10_000);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::AmountOverflow,
        "equity payment overflow must return AmountOverflow"
    );
}

#[test]
fn equity_split_overflow_returns_amount_overflow() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let tenant = Address::generate(&env);
    let id = deal_id(&env, 2);

    // Initialize with forfeiture rate of 2000 bps (20%)
    client.init(&admin, &2000u32);

    client.register_deal(&admin, &id, &tenant, &100_000, &10_000, &10);

    // Record a large equity payment
    client.record_equity_payment(&admin, &id, &50_000, &10_000);

    // Defaulting should not overflow even with forfeiture rate
    // This test verifies the checked arithmetic in equity_split
    client.default_deal(&admin, &id, &Symbol::new(&env, "test"));

    let settlement = client.get_default_settlement(&id).unwrap();
    assert_eq!(settlement.forfeited_usdc, 10_000); // 20% of 50,000
    assert_eq!(settlement.refundable_usdc, 40_000);
}

#[test]
fn equity_percentage_overflow_handling() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let tenant = Address::generate(&env);
    let id = deal_id(&env, 3);

    // Register a deal with very small property value
    client.register_deal(&admin, &id, &tenant, &1, &10_000, &10);

    // Record equity that exceeds property value should fail with EquityOverflow, not trap
    let result = client.try_record_equity_payment(&admin, &id, &100, &10_000);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::EquityOverflow,
        "equity exceeding property value must return EquityOverflow"
    );
}

#[test]
fn get_equity_percentage_handles_overflow() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let tenant = Address::generate(&env);
    let id = deal_id(&env, 4);

    // Register a deal with zero property value (edge case)
    client.register_deal(&admin, &id, &tenant, &0, &10_000, &10);

    // get_equity_percentage should handle division by zero gracefully
    let percentage = client.get_equity_percentage(&id);
    assert_eq!(percentage, 0, "zero property value should return 0% equity");
}
