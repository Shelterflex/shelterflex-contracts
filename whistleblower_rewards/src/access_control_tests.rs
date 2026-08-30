//! Authorization-boundary tests: every gated entry point must reject a caller
//! that holds neither the admin nor the operator role.

extern crate std;

use crate::{ContractError, WhistleblowerRewards, WhistleblowerRewardsClient};
use soroban_pausable_core::PausableError;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, String as SString};

struct Setup<'a> {
    env: Env,
    client: WhistleblowerRewardsClient<'a>,
    admin: Address,
    operator: Address,
    attacker: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(WhistleblowerRewards, ());
    let client = WhistleblowerRewardsClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let attacker = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    client.init(&admin, &operator, &token_id, &0u64);

    Setup {
        env,
        client,
        admin,
        operator,
        attacker,
    }
}

#[test]
fn non_admin_rejected_on_every_admin_gated_entry_point() {
    let s = setup();
    let other = Address::generate(&s.env);
    let wasm_hash = BytesN::from_array(&s.env, &[7u8; 32]);

    assert_eq!(
        s.client
            .try_set_hold_window(&s.attacker, &600)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_hold_window must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_set_operator(&s.attacker, &other)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_operator must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_set_guardian(&s.attacker, &other)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_guardian must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_set_upgrade_delay(&s.attacker, &100)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "set_upgrade_delay must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_propose_upgrade(&s.attacker, &wasm_hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "propose_upgrade must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_execute_upgrade(&s.attacker, &wasm_hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "execute_upgrade must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_emergency_upgrade(&s.attacker, &wasm_hash)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "emergency_upgrade must reject a non-admin"
    );

    assert_eq!(
        s.client
            .try_cancel_upgrade(&s.attacker)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "cancel_upgrade must reject a non-admin"
    );

    assert_eq!(
        s.client.try_pause(&s.attacker).unwrap_err().unwrap(),
        PausableError::NotAuthorized,
        "pause must reject a non-admin"
    );

    s.client.pause(&s.admin);
    assert_eq!(
        s.client.try_unpause(&s.attacker).unwrap_err().unwrap(),
        PausableError::NotAuthorized,
        "unpause must reject a non-admin"
    );
}

/// The operator role is narrower than admin: it must not open the admin gate,
/// and a non-operator must not open the operator gate.
#[test]
fn operator_gate_is_distinct_from_admin_gate() {
    let s = setup();
    let whistleblower = Address::generate(&s.env);

    assert_eq!(
        s.client
            .try_set_hold_window(&s.operator, &600)
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "operator must not pass the admin gate"
    );

    assert_eq!(
        s.client
            .try_allocate(
                &s.attacker,
                &whistleblower,
                &SString::from_str(&s.env, "listing-1"),
                &SString::from_str(&s.env, "deal-1"),
                &1_000,
            )
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "allocate must reject a non-operator"
    );

    assert_eq!(
        s.client
            .try_allocate(
                &s.admin,
                &whistleblower,
                &SString::from_str(&s.env, "listing-1"),
                &SString::from_str(&s.env, "deal-1"),
                &1_000,
            )
            .unwrap_err()
            .unwrap(),
        ContractError::NotAuthorized,
        "admin must not pass the operator gate"
    );
}

/// A rejected call must leave the configuration untouched.
#[test]
fn rejected_call_does_not_change_state() {
    let s = setup();

    let before = s.client.get_hold_window();

    let result = s.client.try_set_hold_window(&s.attacker, &600);
    assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);

    assert_eq!(s.client.get_hold_window(), before);
}
