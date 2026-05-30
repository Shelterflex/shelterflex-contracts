//! Comprehensive tests for `vesting_schedule` — Issue #924.
//!
//! Covers the seven categories enumerated in the issue:
//!   1. Initialisation idempotency
//!   2. Schedule creation (happy path + every validation error)
//!   3. Claim before cliff
//!   4. Partial / repeated claims between cliff and end
//!   5. Full claim at/after end (second claim returns NothingToClaim)
//!   6. Revoke flow (revocable, NotRevocable, AlreadyRevoked, claim-after-revoke)
//!   7. Pause flow (claim blocked, then succeeds after unpause)

use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

use crate::{
    calculate_claimable_amount, calculate_vested_amount, ContractError, VestingSchedule,
    VestingScheduleContract, VestingScheduleContractClient,
};

// ── Constants & fixture helpers ──────────────────────────────────────────────

const START: u64 = 1_000_000;
const CLIFF: u64 = 1_250_000; // start + 250_000
const END: u64 = 2_000_000; // start + 1_000_000
const TOTAL: i128 = 1_000_000;

struct Ctx<'a> {
    env: Env,
    contract: VestingScheduleContractClient<'a>,
    admin: Address,
    #[allow(dead_code)]
    token: Address,
    beneficiary: Address,
}

fn setup<'a>() -> Ctx<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let id = env.register(VestingScheduleContract, ());
    let contract = VestingScheduleContractClient::new(&env, &id);
    contract.initialize(&admin, &token);

    env.ledger().with_mut(|li| li.timestamp = START - 1);

    Ctx {
        env,
        contract,
        admin,
        token,
        beneficiary,
    }
}

fn create_default(ctx: &Ctx, revocable: bool) {
    ctx.contract.create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &END,
        &CLIFF,
        &revocable,
    );
}

fn set_time(env: &Env, t: u64) {
    env.ledger().with_mut(|li| li.timestamp = t);
}

// ── 1. Initialisation ────────────────────────────────────────────────────────

#[test]
fn initialize_succeeds_once_and_fails_on_second_call() {
    let ctx = setup();
    let result = ctx.contract.try_initialize(&ctx.admin, &ctx.token);
    assert_eq!(result, Err(Ok(ContractError::AlreadyInitialized)));
}

// ── 2. Create schedule ───────────────────────────────────────────────────────

#[test]
fn create_schedule_happy_path_stores_correct_fields() {
    let ctx = setup();
    create_default(&ctx, true);
    let stored = ctx.contract.get_vesting_schedule(&ctx.beneficiary);
    assert_eq!(stored.beneficiary, ctx.beneficiary);
    assert_eq!(stored.total_amount, TOTAL);
    assert_eq!(stored.claimed_amount, 0);
    assert_eq!(stored.start_time, START);
    assert_eq!(stored.end_time, END);
    assert_eq!(stored.cliff_time, CLIFF);
    assert!(stored.revocable);
    assert!(!stored.revoked);
}

#[test]
fn create_schedule_twice_returns_already_exists() {
    let ctx = setup();
    create_default(&ctx, true);
    let result = ctx.contract.try_create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &END,
        &CLIFF,
        &true,
    );
    assert_eq!(result, Err(Ok(ContractError::ScheduleAlreadyExists)));
}

#[test]
fn create_rejects_end_time_at_or_before_start() {
    let ctx = setup();
    let result = ctx.contract.try_create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &END,
        &START,
        &CLIFF,
        &true,
    );
    assert_eq!(result, Err(Ok(ContractError::InvalidTimeParameters)));

    let result_equal = ctx.contract.try_create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &START,
        &START,
        &true,
    );
    assert_eq!(result_equal, Err(Ok(ContractError::InvalidTimeParameters)));
}

#[test]
fn create_rejects_cliff_outside_start_end_window() {
    let ctx = setup();

    let before_start = ctx.contract.try_create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &END,
        &(START - 1),
        &true,
    );
    assert_eq!(before_start, Err(Ok(ContractError::InvalidCliffTime)));

    let after_end = ctx.contract.try_create_vesting_schedule(
        &ctx.admin,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &END,
        &(END + 1),
        &true,
    );
    assert_eq!(after_end, Err(Ok(ContractError::InvalidCliffTime)));
}

#[test]
fn create_rejects_non_positive_amount() {
    let ctx = setup();
    for bad in [0, -1, -1_000].into_iter() {
        let result = ctx.contract.try_create_vesting_schedule(
            &ctx.admin,
            &ctx.beneficiary,
            &bad,
            &START,
            &END,
            &CLIFF,
            &true,
        );
        assert_eq!(result, Err(Ok(ContractError::InvalidAmount)));
    }
}

// ── 3. Claim before cliff ────────────────────────────────────────────────────

#[test]
fn claim_before_cliff_returns_cliff_not_reached() {
    let ctx = setup();
    create_default(&ctx, true);
    set_time(&ctx.env, CLIFF - 1);
    let result = ctx.contract.try_claim(&ctx.beneficiary);
    assert_eq!(result, Err(Ok(ContractError::CliffNotReached)));
}

// ── 4. Claim after cliff, before end ─────────────────────────────────────────

#[test]
fn partial_claim_is_proportional_to_elapsed_time() {
    let ctx = setup();
    create_default(&ctx, true);
    // Halfway through the vesting window
    let halfway = START + (END - START) / 2;
    set_time(&ctx.env, halfway);
    let claimed = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(claimed, TOTAL / 2);
    let stored = ctx.contract.get_vesting_schedule(&ctx.beneficiary);
    assert_eq!(stored.claimed_amount, TOTAL / 2);
}

#[test]
fn repeated_partial_claims_accumulate_and_never_exceed_vested() {
    let ctx = setup();
    create_default(&ctx, true);

    // Quarter-way → claim 250k
    set_time(&ctx.env, START + (END - START) / 4);
    let first = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(first, TOTAL / 4);

    // Halfway → claim the next 250k (cumulative vested - already claimed)
    set_time(&ctx.env, START + (END - START) / 2);
    let second = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(second, TOTAL / 4);

    let stored = ctx.contract.get_vesting_schedule(&ctx.beneficiary);
    assert_eq!(stored.claimed_amount, TOTAL / 2);

    // Claiming again at the same instant must return NothingToClaim — the
    // schedule does not let the beneficiary claim more than is vested.
    let third = ctx.contract.try_claim(&ctx.beneficiary);
    assert_eq!(third, Err(Ok(ContractError::NothingToClaim)));
}

// ── 5. Claim at/after end ────────────────────────────────────────────────────

#[test]
fn full_claim_at_or_after_end_releases_total_then_returns_nothing_to_claim() {
    let ctx = setup();
    create_default(&ctx, true);
    set_time(&ctx.env, END);
    let claimed = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(claimed, TOTAL);

    // A second claim past end must report NothingToClaim, not double-pay.
    let second = ctx.contract.try_claim(&ctx.beneficiary);
    assert_eq!(second, Err(Ok(ContractError::NothingToClaim)));
}

// ── 6. Revoke ────────────────────────────────────────────────────────────────

#[test]
fn revocable_schedule_returns_unclaimed_to_admin_and_blocks_further_claims() {
    let ctx = setup();
    create_default(&ctx, true);

    // Claim a quarter, then revoke.
    set_time(&ctx.env, START + (END - START) / 4);
    let claimed = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(claimed, TOTAL / 4);

    let returned = ctx.contract.revoke(&ctx.admin, &ctx.beneficiary);
    assert_eq!(returned, TOTAL - TOTAL / 4);

    // Claim after revoke → Revoked error.
    set_time(&ctx.env, END);
    let attempt = ctx.contract.try_claim(&ctx.beneficiary);
    assert_eq!(attempt, Err(Ok(ContractError::Revoked)));
}

#[test]
fn non_revocable_schedule_cannot_be_revoked() {
    let ctx = setup();
    create_default(&ctx, false);
    let result = ctx.contract.try_revoke(&ctx.admin, &ctx.beneficiary);
    assert_eq!(result, Err(Ok(ContractError::NotRevocable)));
}

#[test]
fn revoking_twice_returns_already_revoked() {
    let ctx = setup();
    create_default(&ctx, true);
    ctx.contract.revoke(&ctx.admin, &ctx.beneficiary);
    let result = ctx.contract.try_revoke(&ctx.admin, &ctx.beneficiary);
    assert_eq!(result, Err(Ok(ContractError::AlreadyRevoked)));
}

// ── 7. Pause ─────────────────────────────────────────────────────────────────

#[test]
fn claim_is_blocked_while_paused_and_works_again_after_unpause() {
    let ctx = setup();
    create_default(&ctx, true);
    set_time(&ctx.env, END);

    ctx.contract.pause(&ctx.admin);
    assert!(ctx.contract.is_paused());

    let blocked = ctx.contract.try_claim(&ctx.beneficiary);
    assert_eq!(blocked, Err(Ok(ContractError::Paused)));

    ctx.contract.unpause(&ctx.admin);
    assert!(!ctx.contract.is_paused());

    let claimed = ctx.contract.claim(&ctx.beneficiary);
    assert_eq!(claimed, TOTAL);
}

// ── Auxiliary: not-found & auth paths ────────────────────────────────────────

#[test]
fn claim_with_no_schedule_returns_schedule_not_found() {
    let ctx = setup();
    let stranger = Address::generate(&ctx.env);
    let result = ctx.contract.try_claim(&stranger);
    assert_eq!(result, Err(Ok(ContractError::ScheduleNotFound)));
}

#[test]
fn non_admin_cannot_create_revoke_or_pause() {
    let ctx = setup();
    let stranger = Address::generate(&ctx.env);

    let create = ctx.contract.try_create_vesting_schedule(
        &stranger,
        &ctx.beneficiary,
        &TOTAL,
        &START,
        &END,
        &CLIFF,
        &true,
    );
    assert_eq!(create, Err(Ok(ContractError::NotAuthorized)));

    create_default(&ctx, true);

    let revoke = ctx.contract.try_revoke(&stranger, &ctx.beneficiary);
    assert_eq!(revoke, Err(Ok(ContractError::NotAuthorized)));

    let pause = ctx.contract.try_pause(&stranger);
    assert_eq!(pause, Err(Ok(ContractError::NotAuthorized)));
}

// ── Pure helpers (no Env required) ──────────────────────────────────────────

#[test]
fn pure_calculate_vested_amount_is_linear_between_start_and_end() {
    let env = Env::default();
    let schedule = VestingSchedule {
        beneficiary: Address::generate(&env),
        total_amount: 1_000,
        claimed_amount: 0,
        start_time: 1_000,
        end_time: 2_000,
        cliff_time: 1_200,
        revocable: true,
        revoked: false,
    };
    assert_eq!(calculate_vested_amount(&schedule, 900), 0);
    assert_eq!(calculate_vested_amount(&schedule, 1_000), 0);
    assert_eq!(calculate_vested_amount(&schedule, 1_500), 500);
    assert_eq!(calculate_vested_amount(&schedule, 2_500), 1_000);
}

#[test]
fn pure_calculate_claimable_amount_subtracts_already_claimed_and_honours_cliff() {
    let env = Env::default();
    let mut schedule = VestingSchedule {
        beneficiary: Address::generate(&env),
        total_amount: 1_000,
        claimed_amount: 200,
        start_time: 1_000,
        end_time: 2_000,
        cliff_time: 1_200,
        revocable: true,
        revoked: false,
    };
    // Before cliff: nothing claimable even though vesting has accrued.
    assert_eq!(calculate_claimable_amount(&schedule, 1_100), 0);
    // Halfway, already claimed 200 → 300 left to claim now.
    assert_eq!(calculate_claimable_amount(&schedule, 1_500), 300);

    // After the schedule is fully claimed, saturating subtraction returns 0.
    schedule.claimed_amount = 1_000;
    assert_eq!(calculate_claimable_amount(&schedule, 2_500), 0);
}
