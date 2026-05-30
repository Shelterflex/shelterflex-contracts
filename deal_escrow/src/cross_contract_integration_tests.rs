//! Cross-contract integration tests: rent_wallet + deal_escrow + rent_payments

extern crate std;

use crate::{
    ContractError as DealEscrowError, DealEscrow, DealEscrowClient, TokenClient as EscrowTokenClient,
};
use rent_payments::{RentPayments, RentPaymentsClient};
use rent_wallet::{RentWallet, RentWalletClient};
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, Env, IntoVal, String, Symbol};
use std::format;

/// Deployed contracts and role addresses for cross-contract payment flows.
struct TestContracts<'a> {
    rent_wallet_id: Address,
    rent_wallet: RentWalletClient<'a>,
    deal_escrow_id: Address,
    deal_escrow: DealEscrowClient<'a>,
    rent_payments_id: Address,
    rent_payments: RentPaymentsClient<'a>,
    token: Address,
    token_admin: Address,
    admin: Address,
    operator: Address,
    tenant: Address,
    landlord: Address,
    platform: Address,
    reporter: Address,
}

fn setup_full_stack(env: &Env) -> TestContracts<'_> {
    let admin = Address::generate(env);
    let operator = Address::generate(env);
    let tenant = Address::generate(env);
    let landlord = Address::generate(env);
    let platform = Address::generate(env);
    let reporter = Address::generate(env);

    let token_admin = Address::generate(env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = token_contract.address();

    let rent_wallet_id = env.register(RentWallet, ());
    let rent_wallet = RentWalletClient::new(env, &rent_wallet_id);
    rent_wallet.try_init(&admin).unwrap().unwrap();

    let rent_payments_id = env.register(RentPayments, ());
    let rent_payments = RentPaymentsClient::new(env, &rent_payments_id);
    rent_payments.try_init(&admin).unwrap().unwrap();

    let deal_escrow_id = env.register(DealEscrow, ());
    let deal_escrow = DealEscrowClient::new(env, &deal_escrow_id);
    deal_escrow
        .try_init(&admin, &operator, &token, &rent_payments_id)
        .unwrap()
        .unwrap();

    TestContracts {
        rent_wallet_id,
        rent_wallet,
        deal_escrow_id,
        deal_escrow,
        rent_payments_id,
        rent_payments,
        token,
        token_admin,
        admin,
        operator,
        tenant,
        landlord,
        platform,
        reporter,
    }
}

fn deal_id_str(env: &Env, deal_id: u64) -> String {
    String::from_str(env, &format!("{deal_id}"))
}

fn mint_to(env: &Env, token: &Address, token_admin: &Address, to: &Address, amount: i128) {
    let sac = StellarAssetClient::new(env, token);
    env.mock_auths(&[MockAuth {
        address: token_admin,
        invoke: &MockAuthInvoke {
            contract: token,
            fn_name: "mint",
            args: (to.clone(), amount).into_val(env),
            sub_invokes: &[],
        },
    }]);
    sac.mint(to, &amount);
}

fn wallet_credit_and_escrow_deposit(
    env: &Env,
    stack: &TestContracts<'_>,
    deal_id: u64,
    amount: i128,
) {
    let deal_str = deal_id_str(env, deal_id);
    let token_client = TokenClient::new(env, &stack.token);

    env.mock_auths(&[MockAuth {
        address: &stack.admin,
        invoke: &MockAuthInvoke {
            contract: &stack.rent_wallet_id,
            fn_name: "credit",
            args: (stack.admin.clone(), stack.tenant.clone(), amount).into_val(env),
            sub_invokes: &[],
        },
    }]);
    stack
        .rent_wallet
        .try_credit(&stack.admin, &stack.tenant, &amount)
        .unwrap()
        .unwrap();
    assert_eq!(stack.rent_wallet.balance(&stack.tenant), amount);

    env.mock_auths(&[MockAuth {
        address: &stack.admin,
        invoke: &MockAuthInvoke {
            contract: &stack.rent_wallet_id,
            fn_name: "debit",
            args: (stack.admin.clone(), stack.tenant.clone(), amount).into_val(env),
            sub_invokes: &[],
        },
    }]);
    stack
        .rent_wallet
        .try_debit(&stack.admin, &stack.tenant, &amount)
        .unwrap()
        .unwrap();
    assert_eq!(stack.rent_wallet.balance(&stack.tenant), 0);

    let tenant_balance_before = token_client.balance(&stack.tenant);
    mint_to(env, &stack.token, &stack.token_admin, &stack.tenant, amount);

    env.mock_auths(&[MockAuth {
        address: &stack.tenant,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "deposit",
            args: (stack.tenant.clone(), deal_str.clone(), amount).into_val(env),
            sub_invokes: &[MockAuthInvoke {
                contract: &stack.token,
                fn_name: "transfer",
                args: (stack.tenant.clone(), stack.deal_escrow_id.clone(), amount).into_val(env),
                sub_invokes: &[],
            }],
        },
    }]);
    stack
        .deal_escrow
        .try_deposit(&stack.tenant, &deal_str, &amount)
        .unwrap()
        .unwrap();

    assert_eq!(
        token_client.balance(&stack.tenant),
        tenant_balance_before + amount - amount
    );
    assert_eq!(stack.deal_escrow.balance(&deal_str), amount);
}

fn release_escrow_and_record_receipt(
    env: &Env,
    stack: &TestContracts<'_>,
    deal_id: u64,
    principal: i128,
    platform_amount: i128,
    reporter_amount: i128,
    receipt_amount: i128,
) {
    let deal_str = deal_id_str(env, deal_id);
    let token_client = EscrowTokenClient::new(env, &stack.token);
    let escrow_balance = stack.deal_escrow.balance(&deal_str);

    let landlord_before = token_client.balance(&stack.landlord);
    let platform_before = token_client.balance(&stack.platform);
    let reporter_before = token_client.balance(&stack.reporter);

    env.mock_auths(&[MockAuth {
        address: &stack.operator,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "release",
            args: (
                stack.operator.clone(),
                deal_str.clone(),
                stack.landlord.clone(),
                principal,
                stack.platform.clone(),
                platform_amount,
                stack.reporter.clone(),
                reporter_amount,
                Symbol::new(env, "manual_admin"),
                String::from_str(env, "payment"),
            )
                .into_val(env),
            sub_invokes: &[],
        },
    }]);
    let released = stack
        .deal_escrow
        .try_release(
            &stack.operator,
            &deal_str,
            &stack.landlord,
            &principal,
            &stack.platform,
            &platform_amount,
            &stack.reporter,
            &reporter_amount,
            &Symbol::new(env, "manual_admin"),
            &String::from_str(env, "payment"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(released, escrow_balance);
    assert_eq!(stack.deal_escrow.balance(&deal_str), 0);

    assert_eq!(
        token_client.balance(&stack.landlord),
        landlord_before + principal
    );
    assert_eq!(
        token_client.balance(&stack.platform),
        platform_before + platform_amount
    );
    assert_eq!(
        token_client.balance(&stack.reporter),
        reporter_before + reporter_amount
    );

    env.mock_auths(&[MockAuth {
        address: &stack.admin,
        invoke: &MockAuthInvoke {
            contract: &stack.rent_payments_id,
            fn_name: "create_receipt",
            args: (deal_id, receipt_amount, stack.tenant.clone()).into_val(env),
            sub_invokes: &[],
        },
    }]);
    let receipt = stack
        .rent_payments
        .try_create_receipt(&deal_id, &receipt_amount, &stack.tenant)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.deal_id, deal_id);
    assert_eq!(receipt.amount, receipt_amount);
    assert_eq!(receipt.payer, stack.tenant);
}

#[test]
fn scenario_1_full_deal_payment_flow() {
    let env = Env::default();
    let stack = setup_full_stack(&env);
    let deal_id = 1u64;
    let amount = 1_000i128;
    let principal = 850i128;
    let platform_fee = 100i128;
    let reporter_fee = 50i128;

    wallet_credit_and_escrow_deposit(&env, &stack, deal_id, amount);
    release_escrow_and_record_receipt(
        &env,
        &stack,
        deal_id,
        principal,
        platform_fee,
        reporter_fee,
        amount,
    );

    assert_eq!(stack.rent_payments.receipt_count(&deal_id), 1);
    let page = stack
        .rent_payments
        .list_receipts_by_deal(&deal_id, &10u32, &None);
    assert_eq!(page.receipts.len(), 1);
    assert_eq!(page.receipts.get(0).unwrap().amount, amount);
    assert_eq!(page.receipts.get(0).unwrap().deal_id, deal_id);
}

#[test]
fn scenario_2_partial_instalment_flow_three_payments() {
    let env = Env::default();
    let stack = setup_full_stack(&env);
    let deal_id = 2u64;
    let instalment = 1_000i128;
    let principal = 850i128;
    let platform_fee = 100i128;
    let reporter_fee = 50i128;
    let mut cumulative = 0i128;

    for _ in 0..3 {
        wallet_credit_and_escrow_deposit(&env, &stack, deal_id, instalment);
        release_escrow_and_record_receipt(
            &env,
            &stack,
            deal_id,
            principal,
            platform_fee,
            reporter_fee,
            instalment,
        );
        cumulative += instalment;
    }

    assert_eq!(stack.rent_payments.receipt_count(&deal_id), 3);
    let page = stack
        .rent_payments
        .list_receipts_by_deal(&deal_id, &10u32, &None);
    assert_eq!(page.receipts.len(), 3);

    let mut receipt_total = 0i128;
    for i in 0..page.receipts.len() {
        receipt_total += page.receipts.get(i).unwrap().amount;
    }
    assert_eq!(receipt_total, cumulative);
    assert_eq!(cumulative, 3_000i128);
}

#[test]
fn scenario_3_paused_deal_escrow_blocks_deposit_until_unpaused() {
    let env = Env::default();
    let stack = setup_full_stack(&env);
    let deal_id = 3u64;
    let deal_str = deal_id_str(&env, deal_id);
    let amount = 500i128;

    mint_to(
        &env,
        &stack.token,
        &stack.token_admin,
        &stack.tenant,
        amount,
    );

    env.mock_auths(&[MockAuth {
        address: &stack.admin,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "pause",
            args: (stack.admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    stack.deal_escrow.try_pause(&stack.admin).unwrap().unwrap();

    env.mock_auths(&[MockAuth {
        address: &stack.tenant,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "deposit",
            args: (stack.tenant.clone(), deal_str.clone(), amount).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let paused_err = stack
        .deal_escrow
        .try_deposit(&stack.tenant, &deal_str, &amount)
        .unwrap_err()
        .unwrap();
    assert_eq!(paused_err, DealEscrowError::Paused);

    env.mock_auths(&[MockAuth {
        address: &stack.admin,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "unpause",
            args: (stack.admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    stack
        .deal_escrow
        .try_unpause(&stack.admin)
        .unwrap()
        .unwrap();

    env.mock_auths(&[MockAuth {
        address: &stack.tenant,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "deposit",
            args: (stack.tenant.clone(), deal_str.clone(), amount).into_val(&env),
            sub_invokes: &[MockAuthInvoke {
                contract: &stack.token,
                fn_name: "transfer",
                args: (stack.tenant.clone(), stack.deal_escrow_id.clone(), amount).into_val(&env),
                sub_invokes: &[],
            }],
        },
    }]);
    stack
        .deal_escrow
        .try_deposit(&stack.tenant, &deal_str, &amount)
        .unwrap()
        .unwrap();
    assert_eq!(stack.deal_escrow.balance(&deal_str), amount);
}

#[test]
fn scenario_4_release_more_than_escrow_balance_fails() {
    let env = Env::default();
    let stack = setup_full_stack(&env);
    let deal_id = 4u64;
    let deal_str = deal_id_str(&env, deal_id);
    let deposited = 100i128;

    wallet_credit_and_escrow_deposit(&env, &stack, deal_id, deposited);

    env.mock_auths(&[MockAuth {
        address: &stack.operator,
        invoke: &MockAuthInvoke {
            contract: &stack.deal_escrow_id,
            fn_name: "release",
            args: (
                stack.operator.clone(),
                deal_str.clone(),
                stack.landlord.clone(),
                200i128,
                stack.platform.clone(),
                50i128,
                stack.reporter.clone(),
                50i128,
                Symbol::new(&env, "manual_admin"),
                String::from_str(&env, "over-release"),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let err = stack
        .deal_escrow
        .try_release(
            &stack.operator,
            &deal_str,
            &stack.landlord,
            &200i128,
            &stack.platform,
            &50i128,
            &stack.reporter,
            &50i128,
            &Symbol::new(&env, "manual_admin"),
            &String::from_str(&env, "over-release"),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, DealEscrowError::InvalidSplit);
    assert_eq!(stack.deal_escrow.balance(&deal_str), deposited);
}
