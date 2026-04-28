#![no_std]

extern crate alloc;

use soroban_pausable::{Pausable, PausableError};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token::Client as TokenClient, Address,
    BytesN, Env, String, Symbol, Vec,
};

// ── Storage keys ─────────────────────────────────────────────────────────────
pub mod access_control;
pub mod validation;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    ContractVersion,
    StorageSchemaVersion,
    Admin,
    Operator,
    Resolver,
    ChallengeWindowSeconds,
    DisputeTimeoutSeconds,
    Token,
    ReceiptContract,
    Paused,
    /// Per-deal balance in persistent storage (#386 gas optimisation)
    DealBalance(String),
    DealDepositor(String),
    DealState(String),
    PendingRentRelease(String),
    RentDispute(String),
    LegacyLockedAmountV2(String),
    LegacyPendingPayoutV2(String),
    /// Reentrancy lock (#390)
    Reentrancy,
    // ── Upgrade governance (#392) ─────────────────────────────────────────
    Guardian,
    UpgradeDelay,
    PendingUpgradeHash,
    PendingUpgradeAt,
    // ── Circuit breaker (#393) ──────────────────────────────────────────
    CircuitBreakerState,
    PendingDrainHash,
    PendingDrainAt,
    RecoveryDelaySeconds,
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    Paused = 3,
    InvalidAmount = 4,
    InsufficientBalance = 5,
    EmptyString = 10,
    // Cross-contract communication errors (#390)
    /// Reentrancy detected
    ReentrancyDetected = 6,
    // Upgrade governance errors (#392)
    UpgradeAlreadyPending = 7,
    NoUpgradePending = 8,
    UpgradeDelayNotMet = 9,
    InvalidSchemaVersion = 11,
    MigrationInvariantViolation = 12,
    MigrationNotRequired = 13,
    PendingReleaseExists = 14,
    NoPendingRelease = 15,
    InvalidReleaseWindow = 16,
    NoOpenDispute = 17,
    DisputeNotAllowed = 18,
    InvalidEvidenceRef = 19,
    InvalidSettlement = 20,
    // Circuit breaker errors (#393)
    Frozen = 21,
    DrainRestricted = 22,
    InvalidGovernanceDrain = 23,
    RecoveryDelayNotMet = 24,
}

#[contracttype]
#[derive(Clone)]
pub struct DealState {
    pub total_balance: i128,
    pub locked_amount: i128,
    pub pending_payout: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct PendingRentRelease {
    pub to: Address,
    pub amount: i128,
    pub requested_by: Address,
    pub requested_at: u64,
    pub challenge_end_at: u64,
    pub external_ref_source: Symbol,
    pub external_ref: String,
}

#[contracttype]
#[derive(Clone)]
pub struct RentDispute {
    pub opened_by: Address,
    pub opened_at: u64,
    pub evidence_ref: String,
    pub challenge_evidence_ref: String,
    pub resolved: bool,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementOutcome {
    ReleaseToRecipient = 1,
    RefundToDepositor = 2,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u32)]
pub enum CircuitBreakerState {
    Unfrozen = 0,
    Frozen = 1,
    RecoveryAwaiting = 2,
}

const STORAGE_SCHEMA_V1: u32 = 1;
const STORAGE_SCHEMA_V2: u32 = 2;
const STORAGE_SCHEMA_V3: u32 = 3;

// ── Default values ───────────────────────────────────────────────────────

const DEFAULT_CIRCUIT_BREAKER_STATE: CircuitBreakerState = CircuitBreakerState::Unfrozen;

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct DealEscrow;

// ── Internal helpers ──────────────────────────────────────────────────────────

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Admin)
        .expect("admin not set")
}

fn get_storage_schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get::<_, u32>(&DataKey::StorageSchemaVersion)
        .unwrap_or(STORAGE_SCHEMA_V1)
}

fn get_operator(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Operator)
        .expect("operator not set")
}

fn get_resolver(env: &Env) -> Option<Address> {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Resolver)
}

fn get_challenge_window_seconds(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::ChallengeWindowSeconds)
        .unwrap_or(24 * 60 * 60)
}

fn get_dispute_timeout_seconds(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::DisputeTimeoutSeconds)
        .unwrap_or(48 * 60 * 60)
}

fn get_token(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Token)
        .expect("token not set")
}

fn get_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn get_circuit_breaker_state(env: &Env) -> CircuitBreakerState {
    env.storage()
        .instance()
        .get::<_, CircuitBreakerState>(&DataKey::CircuitBreakerState)
        .unwrap_or(CircuitBreakerState::Unfrozen)
}

fn get_recovery_delay_seconds(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::RecoveryDelaySeconds)
        .unwrap_or(24 * 60 * 60) // Default 24 hours
}

fn get_pending_drain_hash(env: &Env) -> Option<BytesN<32>> {
    env.storage()
        .instance()
        .get::<_, BytesN<32>>(&DataKey::PendingDrainHash)
}

fn get_pending_drain_at(env: &Env) -> Option<u64> {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::PendingDrainAt)
}

fn require_not_frozen(env: &Env, is_governance_drain: bool) -> Result<(), ContractError> {
    let state = get_circuit_breaker_state(env);
    match state {
        CircuitBreakerState::Unfrozen => Ok(()),
        CircuitBreakerState::Frozen => {
            if is_governance_drain {
                Ok(()) // Allow governance drain path
            } else {
                Err(ContractError::Frozen)
            }
        }
        CircuitBreakerState::RecoveryAwaiting => Err(ContractError::RecoveryDelayNotMet),
    }
}

fn require_not_paused(env: &Env) -> Result<(), ContractError> {
    if get_paused(env) {
        return Err(ContractError::Paused);
    }
    Ok(())
}

/// Per-deal balance from persistent storage (#386)
fn get_deal_balance(env: &Env, deal_id: &String) -> i128 {
    env.storage()
        .persistent()
        .get::<_, i128>(&DataKey::DealBalance(deal_id.clone()))
        .unwrap_or(0)
}

fn put_deal_balance(env: &Env, deal_id: &String, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::DealBalance(deal_id.clone()), &amount);
}

fn get_deal_depositor(env: &Env, deal_id: &String) -> Option<Address> {
    env.storage()
        .persistent()
        .get::<_, Address>(&DataKey::DealDepositor(deal_id.clone()))
}

fn set_deal_depositor_if_missing(env: &Env, deal_id: &String, depositor: &Address) {
    let key = DataKey::DealDepositor(deal_id.clone());
    if !env.storage().persistent().has(&key) {
        env.storage().persistent().set(&key, depositor);
    }
}

fn get_deal_state(env: &Env, deal_id: &String) -> DealState {
    if let Some(state) = env
        .storage()
        .persistent()
        .get::<_, DealState>(&DataKey::DealState(deal_id.clone()))
    {
        return state;
    }
    DealState {
        total_balance: get_deal_balance(env, deal_id),
        locked_amount: 0,
        pending_payout: 0,
    }
}

fn set_deal_state(env: &Env, deal_id: &String, state: &DealState) {
    env.storage()
        .persistent()
        .set(&DataKey::DealState(deal_id.clone()), state);
}

fn assert_deal_invariants(state: &DealState) -> Result<(), ContractError> {
    if state.total_balance < 0 || state.locked_amount < 0 || state.pending_payout < 0 {
        return Err(ContractError::MigrationInvariantViolation);
    }
    if state.locked_amount != state.pending_payout {
        return Err(ContractError::MigrationInvariantViolation);
    }
    if state.locked_amount > state.total_balance {
        return Err(ContractError::MigrationInvariantViolation);
    }
    Ok(())
}

fn get_pending_release(env: &Env, deal_id: &String) -> Option<PendingRentRelease> {
    env.storage()
        .persistent()
        .get::<_, PendingRentRelease>(&DataKey::PendingRentRelease(deal_id.clone()))
}

fn set_pending_release(env: &Env, deal_id: &String, release: &PendingRentRelease) {
    env.storage()
        .persistent()
        .set(&DataKey::PendingRentRelease(deal_id.clone()), release);
}

fn clear_pending_release(env: &Env, deal_id: &String) {
    env.storage()
        .persistent()
        .remove(&DataKey::PendingRentRelease(deal_id.clone()));
}

fn get_dispute(env: &Env, deal_id: &String) -> Option<RentDispute> {
    env.storage()
        .persistent()
        .get::<_, RentDispute>(&DataKey::RentDispute(deal_id.clone()))
}

fn set_dispute(env: &Env, deal_id: &String, dispute: &RentDispute) {
    env.storage()
        .persistent()
        .set(&DataKey::RentDispute(deal_id.clone()), dispute);
}

fn clear_dispute(env: &Env, deal_id: &String) {
    env.storage()
        .persistent()
        .remove(&DataKey::RentDispute(deal_id.clone()));
}

/// Reentrancy guard (#390)
fn enter_nonreentrant(env: &Env) -> Result<(), ContractError> {
    if env
        .storage()
        .instance()
        .get::<_, bool>(&DataKey::Reentrancy)
        .unwrap_or(false)
    {
        return Err(ContractError::ReentrancyDetected);
    }
    env.storage().instance().set(&DataKey::Reentrancy, &true);
    Ok(())
}

fn exit_nonreentrant(env: &Env) {
    env.storage().instance().set(&DataKey::Reentrancy, &false);
}

fn generate_tx_id(env: &Env, external_ref_source: &Symbol, external_ref: &String) -> BytesN<32> {
    use alloc::string::ToString;
    use soroban_sdk::Bytes;
    let source_str = external_ref_source.to_string();
    let source_trimmed = source_str.trim();
    let source_lower = {
        let mut s = alloc::string::String::new();
        for c in source_trimmed.chars() {
            for lower in c.to_lowercase() {
                s.push(lower);
            }
        }
        s
    };
    let ref_str = external_ref.to_string();
    let ref_trimmed = ref_str.trim();
    let canonical = {
        use alloc::format;
        format!("v1|source={}|ref={}", source_lower, ref_trimmed)
    };
    let canonical_bytes = Bytes::from_slice(env, canonical.as_bytes());
    let hash = env.crypto().sha256(&canonical_bytes);
    hash.into()
}

// ── Contract implementation ───────────────────────────────────────────────────

#[contractimpl]
impl DealEscrow {
    pub fn init(
        env: Env,
        admin: Address,
        operator: Address,
        token: Address,
        receipt_contract: Address,
    ) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::StorageSchemaVersion, &STORAGE_SCHEMA_V3);
        env.storage()
            .instance()
            .set(&DataKey::ReceiptContract, &receipt_contract);
        env.storage().instance().set(&DataKey::Paused, &false);
        // #389: consistent init event with version
        env.events().publish(
            (Symbol::new(&env, "deal_escrow"), Symbol::new(&env, "init")),
            (admin, operator, token, receipt_contract, 1u32),
        );
        Ok(())
    }

    pub fn contract_version(env: Env) -> u32 {
        env.storage()
            .instance()
            .get::<_, u32>(&DataKey::ContractVersion)
            .unwrap_or(0u32)
    }

    pub fn deposit(
        env: Env,
        from: Address,
        deal_id: String,
        amount: i128,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_valid_amount(amount)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        from.require_auth();
        let token_addr = get_token(&env);
        let token_client = TokenClient::new(&env, &token_addr);

        // #390: reentrancy guard before external token call
        enter_nonreentrant(&env)?;
        token_client.transfer(&from, &env.current_contract_address(), &amount);
        exit_nonreentrant(&env);

        // #386: per-key persistent storage
        let cur = get_deal_balance(&env, &deal_id);
        put_deal_balance(&env, &deal_id, cur + amount);
        set_deal_depositor_if_missing(&env, &deal_id, &from);
        let mut state = get_deal_state(&env, &deal_id);
        state.total_balance = cur + amount;
        assert_deal_invariants(&state)?;
        set_deal_state(&env, &deal_id, &state);

        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "deposit"),
            ),
            (deal_id, from, amount),
        );
        Ok(())
    }

    pub fn release(
        env: Env,
        caller: Address,
        deal_id: String,
        to: Address,
        external_ref_source: Symbol,
        external_ref: String,
    ) -> Result<i128, ContractError> {
        require_not_paused(&env)?;
        require_not_frozen(&env, false)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        validation::require_non_empty_string(&env, &external_ref)?;
        let admin = get_admin(&env);
        let operator = get_operator(&env);
        access_control::require_admin_or_operator_permission(
            &env, &admin, &operator, &caller, "release",
        )?;

        // #386: per-key persistent storage
        let cur = get_deal_balance(&env, &deal_id);
        if cur <= 0 {
            return Err(ContractError::InsufficientBalance);
        }
        let token_addr = get_token(&env);
        let token_client = TokenClient::new(&env, &token_addr);

        put_deal_balance(&env, &deal_id, 0);
        let mut state = get_deal_state(&env, &deal_id);
        state.total_balance = 0;
        state.locked_amount = 0;
        state.pending_payout = 0;
        assert_deal_invariants(&state)?;
        set_deal_state(&env, &deal_id, &state);

        let tx_id = generate_tx_id(&env, &external_ref_source, &external_ref);

        // #390: reentrancy guard before external token call
        enter_nonreentrant(&env)?;
        token_client.transfer(&env.current_contract_address(), &to, &cur);
        exit_nonreentrant(&env);

        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "release"),
            ),
            (deal_id, to, cur, external_ref_source, tx_id),
        );
        Ok(cur)
    }

    pub fn balance(env: Env, deal_id: String) -> i128 {
        get_deal_balance(&env, &deal_id)
    }

    pub fn storage_schema_version(env: Env) -> u32 {
        get_storage_schema_version(&env)
    }

    pub fn configure_dispute_windows(
        env: Env,
        admin: Address,
        challenge_window_seconds: u64,
        dispute_timeout_seconds: u64,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "configure_dispute_windows",
        )?;
        if challenge_window_seconds == 0 || dispute_timeout_seconds == 0 {
            return Err(ContractError::InvalidReleaseWindow);
        }
        env.storage()
            .instance()
            .set(&DataKey::ChallengeWindowSeconds, &challenge_window_seconds);
        env.storage()
            .instance()
            .set(&DataKey::DisputeTimeoutSeconds, &dispute_timeout_seconds);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "configure_dispute_windows"),
            ),
            (challenge_window_seconds, dispute_timeout_seconds),
        );
        Ok(())
    }

    pub fn set_resolver(env: Env, admin: Address, resolver: Address) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "set_resolver")?;
        env.storage().instance().set(&DataKey::Resolver, &resolver);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "set_resolver"),
            ),
            resolver,
        );
        Ok(())
    }

    pub fn migrate_storage_schema(
        env: Env,
        admin: Address,
        from_version: u32,
        deal_ids: Vec<String>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "migrate_storage_schema",
        )?;
        let current = get_storage_schema_version(&env);
        if current == STORAGE_SCHEMA_V3 {
            env.events().publish(
                (
                    Symbol::new(&env, "deal_escrow"),
                    Symbol::new(&env, "migration_noop"),
                ),
                (current, STORAGE_SCHEMA_V3),
            );
            return Ok(());
        }
        if current != from_version
            || !(from_version == STORAGE_SCHEMA_V1 || from_version == STORAGE_SCHEMA_V2)
        {
            return Err(ContractError::InvalidSchemaVersion);
        }
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "migration_started"),
            ),
            (from_version, STORAGE_SCHEMA_V3, deal_ids.len()),
        );

        for deal_id in deal_ids.iter() {
            let balance = get_deal_balance(&env, &deal_id);
            let mut locked = 0i128;
            let mut pending = 0i128;
            if from_version == STORAGE_SCHEMA_V2 {
                locked = env
                    .storage()
                    .persistent()
                    .get::<_, i128>(&DataKey::LegacyLockedAmountV2(deal_id.clone()))
                    .unwrap_or(0);
                pending = env
                    .storage()
                    .persistent()
                    .get::<_, i128>(&DataKey::LegacyPendingPayoutV2(deal_id.clone()))
                    .unwrap_or(0);
            }
            let state = DealState {
                total_balance: balance,
                locked_amount: locked,
                pending_payout: pending,
            };
            assert_deal_invariants(&state)?;
            set_deal_state(&env, &deal_id, &state);
        }

        env.storage()
            .instance()
            .set(&DataKey::StorageSchemaVersion, &STORAGE_SCHEMA_V3);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "migration_completed"),
            ),
            (from_version, STORAGE_SCHEMA_V3),
        );
        Ok(())
    }

    pub fn request_rent_release(
        env: Env,
        caller: Address,
        deal_id: String,
        to: Address,
        amount: i128,
        external_ref_source: Symbol,
        external_ref: String,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_valid_amount(amount)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        validation::require_non_empty_string(&env, &external_ref)?;
        let admin = get_admin(&env);
        let operator = get_operator(&env);
        access_control::require_admin_or_operator_permission(
            &env,
            &admin,
            &operator,
            &caller,
            "request_rent_release",
        )?;
        if get_pending_release(&env, &deal_id).is_some() {
            return Err(ContractError::PendingReleaseExists);
        }
        let mut state = get_deal_state(&env, &deal_id);
        if state.total_balance < amount {
            return Err(ContractError::InsufficientBalance);
        }
        state.locked_amount = amount;
        state.pending_payout = amount;
        assert_deal_invariants(&state)?;
        set_deal_state(&env, &deal_id, &state);
        let now = env.ledger().timestamp();
        let pending = PendingRentRelease {
            to,
            amount,
            requested_by: caller.clone(),
            requested_at: now,
            challenge_end_at: now + get_challenge_window_seconds(&env),
            external_ref_source,
            external_ref,
        };
        set_pending_release(&env, &deal_id, &pending);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "rent_release_requested"),
            ),
            (deal_id, caller, pending.amount, pending.challenge_end_at),
        );
        Ok(())
    }

    pub fn challenge_rent_release(
        env: Env,
        caller: Address,
        deal_id: String,
        challenge_evidence_ref: String,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        validation::require_non_empty_string(&env, &challenge_evidence_ref)?;
        caller.require_auth();
        let pending = get_pending_release(&env, &deal_id).ok_or(ContractError::NoPendingRelease)?;
        let now = env.ledger().timestamp();
        if now > pending.challenge_end_at {
            return Err(ContractError::DisputeNotAllowed);
        }
        let depositor =
            get_deal_depositor(&env, &deal_id).ok_or(ContractError::DisputeNotAllowed)?;
        if caller != depositor && caller != pending.to {
            return Err(ContractError::NotAuthorized);
        }
        let dispute = RentDispute {
            opened_by: caller.clone(),
            opened_at: now,
            evidence_ref: pending.external_ref.clone(),
            challenge_evidence_ref,
            resolved: false,
        };
        set_dispute(&env, &deal_id, &dispute);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "rent_release_challenged"),
            ),
            (deal_id, caller, now),
        );
        Ok(())
    }

    pub fn resolve_rent_dispute(
        env: Env,
        caller: Address,
        deal_id: String,
        outcome: SettlementOutcome,
        resolution_evidence_ref: String,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        validation::require_non_empty_string(&env, &resolution_evidence_ref)?;
        let resolver = get_resolver(&env).ok_or(ContractError::NotAuthorized)?;
        if caller != resolver {
            return Err(ContractError::NotAuthorized);
        }
        caller.require_auth();
        let dispute = get_dispute(&env, &deal_id).ok_or(ContractError::NoOpenDispute)?;
        if dispute.resolved {
            return Err(ContractError::InvalidSettlement);
        }
        Self::settle_release_inner(&env, &deal_id, outcome, &resolution_evidence_ref, true)
    }

    pub fn settle_rent_release_timeout(env: Env, deal_id: String) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        if get_dispute(&env, &deal_id).is_some() {
            return Err(ContractError::DisputeNotAllowed);
        }
        let pending = get_pending_release(&env, &deal_id).ok_or(ContractError::NoPendingRelease)?;
        if env.ledger().timestamp() < pending.challenge_end_at {
            return Err(ContractError::InvalidReleaseWindow);
        }
        Self::settle_release_inner(
            &env,
            &deal_id,
            SettlementOutcome::ReleaseToRecipient,
            &String::from_str(&env, "timeout_auto_release"),
            false,
        )
    }

    pub fn settle_dispute_timeout(env: Env, deal_id: String) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        validation::require_non_empty_string(&env, &deal_id)?;
        let dispute = get_dispute(&env, &deal_id).ok_or(ContractError::NoOpenDispute)?;
        if dispute.resolved {
            return Err(ContractError::InvalidSettlement);
        }
        if env.ledger().timestamp() < dispute.opened_at + get_dispute_timeout_seconds(&env) {
            return Err(ContractError::InvalidReleaseWindow);
        }
        Self::settle_release_inner(
            &env,
            &deal_id,
            SettlementOutcome::RefundToDepositor,
            &String::from_str(&env, "dispute_timeout_refund"),
            false,
        )
    }

    fn settle_release_inner(
        env: &Env,
        deal_id: &String,
        outcome: SettlementOutcome,
        resolution_evidence_ref: &String,
        resolved_by_resolver: bool,
    ) -> Result<(), ContractError> {
        let pending = get_pending_release(env, deal_id).ok_or(ContractError::NoPendingRelease)?;
        let depositor = get_deal_depositor(env, deal_id).ok_or(ContractError::InvalidSettlement)?;
        let mut state = get_deal_state(env, deal_id);
        if state.locked_amount != pending.amount || state.pending_payout != pending.amount {
            return Err(ContractError::MigrationInvariantViolation);
        }
        let recipient = match outcome {
            SettlementOutcome::ReleaseToRecipient => pending.to.clone(),
            SettlementOutcome::RefundToDepositor => depositor.clone(),
        };
        let token_addr = get_token(env);
        let token_client = TokenClient::new(env, &token_addr);
        enter_nonreentrant(env)?;
        token_client.transfer(&env.current_contract_address(), &recipient, &pending.amount);
        exit_nonreentrant(env);

        state.total_balance -= pending.amount;
        state.locked_amount = 0;
        state.pending_payout = 0;
        assert_deal_invariants(&state)?;
        set_deal_state(env, deal_id, &state);
        put_deal_balance(env, deal_id, state.total_balance);
        clear_pending_release(env, deal_id);
        if let Some(mut dispute) = get_dispute(env, deal_id) {
            dispute.resolved = true;
            set_dispute(env, deal_id, &dispute);
            clear_dispute(env, deal_id);
        }
        let tx_id = generate_tx_id(env, &pending.external_ref_source, resolution_evidence_ref);
        env.events().publish(
            (
                Symbol::new(env, "deal_escrow"),
                Symbol::new(env, "rent_release_settled"),
            ),
            (
                deal_id.clone(),
                recipient,
                pending.amount,
                outcome as u32,
                tx_id,
                resolved_by_resolver,
            ),
        );
        Ok(())
    }
}

#[contractimpl]
impl Pausable for DealEscrow {
    fn pause(env: Env, _admin: Address) -> Result<(), PausableError> {
        let stored: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("admin not set");
        if access_control::require_admin_permission(&env, &stored, &_admin, "pause").is_err() {
            return Err(PausableError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (Symbol::new(&env, "Pausable"), Symbol::new(&env, "pause")),
            (),
        );
        Ok(())
    }

    fn unpause(env: Env, _admin: Address) -> Result<(), PausableError> {
        let stored: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("admin not set");
        if access_control::require_admin_permission(&env, &stored, &_admin, "unpause").is_err() {
            return Err(PausableError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        // #389: emit admin address (was `()`)
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "unpause"),
            ),
            _admin,
        );
        Ok(())
    }

    fn is_paused(env: Env) -> bool {
        get_paused(&env)
    }
}

#[contractimpl]
impl DealEscrow {
    pub fn set_guardian(env: Env, admin: Address, guardian: Address) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "set_guardian")?;
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "set_guardian"),
            ),
            guardian,
        );
        Ok(())
    }

    pub fn set_upgrade_delay(
        env: Env,
        admin: Address,
        delay_seconds: u64,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_upgrade_delay",
        )?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeDelay, &delay_seconds);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "set_upgrade_delay"),
            ),
            delay_seconds,
        );
        Ok(())
    }

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "propose_upgrade")?;
        if env.storage().instance().has(&DataKey::PendingUpgradeHash) {
            return Err(ContractError::UpgradeAlreadyPending);
        }
        let now = env.ledger().timestamp();
        env.storage()
            .instance()
            .set(&DataKey::PendingUpgradeHash, &new_wasm_hash);
        env.storage()
            .instance()
            .set(&DataKey::PendingUpgradeAt, &now);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "propose_upgrade"),
            ),
            (new_wasm_hash, now),
        );
        Ok(())
    }

    pub fn execute_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "execute_upgrade")?;
        let pending: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::PendingUpgradeHash)
            .ok_or(ContractError::NoUpgradePending)?;
        if pending != new_wasm_hash {
            return Err(ContractError::NoUpgradePending);
        }
        let proposed_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PendingUpgradeAt)
            .unwrap_or(0);
        let delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeDelay)
            .unwrap_or(0);
        if delay > 0 && env.ledger().timestamp() < proposed_at + delay {
            return Err(ContractError::UpgradeDelayNotMet);
        }
        if let Some(guardian) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::Guardian)
        {
            guardian.require_auth();
        }
        env.storage()
            .instance()
            .remove(&DataKey::PendingUpgradeHash);
        env.storage().instance().remove(&DataKey::PendingUpgradeAt);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "execute_upgrade"),
            ),
            new_wasm_hash.clone(),
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn emergency_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "emergency_upgrade",
        )?;
        if let Some(guardian) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::Guardian)
        {
            guardian.require_auth();
        }
        env.storage()
            .instance()
            .remove(&DataKey::PendingUpgradeHash);
        env.storage().instance().remove(&DataKey::PendingUpgradeAt);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "emergency_upgrade"),
            ),
            (admin, new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn cancel_upgrade(env: Env, admin: Address) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "cancel_upgrade")?;
        let hash: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::PendingUpgradeHash)
            .ok_or(ContractError::NoUpgradePending)?;
        env.storage()
            .instance()
            .remove(&DataKey::PendingUpgradeHash);
        env.storage().instance().remove(&DataKey::PendingUpgradeAt);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "cancel_upgrade"),
            ),
            (admin, hash),
        );

        Ok(())
    }
}

// ── Circuit Breaker Functions (#393) ───────────────────────────────

#[contractimpl]
impl DealEscrow {
    pub fn freeze(env: Env, admin: Address) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "freeze")?;

        let state = get_circuit_breaker_state(&env);
        if state == CircuitBreakerState::Frozen {
            return Err(ContractError::Frozen);
        }

        env.storage()
            .instance()
            .set(&DataKey::CircuitBreakerState, &CircuitBreakerState::Frozen);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "freeze"),
            ),
            (admin, env.ledger().timestamp()),
        );
        Ok(())
    }

    pub fn is_frozen(env: Env) -> bool {
        get_circuit_breaker_state(&env) == CircuitBreakerState::Frozen
    }

    pub fn propose_drain(
        env: Env,
        admin: Address,
        drain_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "propose_drain")?;

        let state = get_circuit_breaker_state(&env);
        if state != CircuitBreakerState::Frozen {
            return Err(ContractError::InvalidGovernanceDrain); // Must be frozen to propose drain
        }

        let now = env.ledger().timestamp();
        env.storage()
            .instance()
            .set(&DataKey::PendingDrainHash, &drain_hash);
        env.storage().instance().set(&DataKey::PendingDrainAt, &now);

        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "propose_drain"),
            ),
            (drain_hash, now),
        );
        Ok(())
    }

    pub fn execute_drain(
        env: Env,
        admin: Address,
        drain_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "execute_drain")?;

        let pending_hash = get_pending_drain_hash(&env).ok_or(ContractError::NoPendingRelease)?;
        if pending_hash != drain_hash {
            return Err(ContractError::NoPendingRelease);
        }

        let proposed_at = get_pending_drain_at(&env).ok_or(ContractError::NoPendingRelease)?;
        let delay = get_recovery_delay_seconds(&env);
        if env.ledger().timestamp() < proposed_at + delay {
            return Err(ContractError::RecoveryDelayNotMet);
        }

        // Execute drain - only during governance-controlled recovery
        let state = get_circuit_breaker_state(&env);
        if state != CircuitBreakerState::Frozen {
            return Err(ContractError::InvalidGovernanceDrain);
        }

        // Clear pending drain
        env.storage().instance().remove(&DataKey::PendingDrainHash);
        env.storage().instance().remove(&DataKey::PendingDrainAt);

        // Allow recovery by unfreezing
        env.storage().instance().set(
            &DataKey::CircuitBreakerState,
            &CircuitBreakerState::Unfrozen,
        );

        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "execute_drain"),
            ),
            (admin, drain_hash, env.ledger().timestamp()),
        );
        Ok(())
    }

    pub fn set_recovery_delay(
        env: Env,
        admin: Address,
        delay_seconds: u64,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_recovery_delay",
        )?;

        env.storage()
            .instance()
            .set(&DataKey::RecoveryDelaySeconds, &delay_seconds);
        env.events().publish(
            (
                Symbol::new(&env, "deal_escrow"),
                Symbol::new(&env, "set_recovery_delay"),
            ),
            delay_seconds,
        );
        Ok(())
    }

    pub fn get_circuit_breaker_state(env: Env) -> u32 {
        get_circuit_breaker_state(&env) as u32
    }

    fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod test {
    extern crate std;
    use super::{
        ContractError, DataKey, DealEscrow, DealEscrowClient, SettlementOutcome, TokenClient,
    };
    use soroban_sdk::testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke};
    use soroban_sdk::token::StellarAssetClient;
    use soroban_sdk::{Address, BytesN, Env, IntoVal, String, Symbol};

    #[test]
    fn init_sets_version_to_one() {
        let env = Env::default();
        let contract_id = env.register(DealEscrow, ());
        let client = DealEscrowClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let operator = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_id = token_contract.address();
        let receipt = Address::generate(&env);
        client
            .try_init(&admin, &operator, &token_id, &receipt)
            .unwrap()
            .unwrap();
        assert_eq!(client.contract_version(), 1u32);
    }

    #[test]
    fn init_cannot_be_called_twice() {
        let env = Env::default();
        let contract_id = env.register(DealEscrow, ());
        let client = DealEscrowClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let operator = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_id = token_contract.address();
        let receipt = Address::generate(&env);
        client
            .try_init(&admin, &operator, &token_id, &receipt)
            .unwrap()
            .unwrap();
        let err = client
            .try_init(&admin, &operator, &token_id, &receipt)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::AlreadyInitialized);
    }

    fn setup(
        env: &Env,
    ) -> (
        Address,
        DealEscrowClient<'_>,
        Address,
        Address,
        Address,
        Address,
        Address,
    ) {
        let contract_id = env.register(DealEscrow, ());
        let client = DealEscrowClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let operator = Address::generate(env);
        let token_admin = Address::generate(env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_contract_id = token_contract.address();
        let receipt_contract = Address::generate(env);
        client
            .try_init(&admin, &operator, &token_contract_id, &receipt_contract)
            .unwrap()
            .unwrap();
        (
            contract_id,
            client,
            admin,
            operator,
            token_contract_id,
            token_admin,
            receipt_contract,
        )
    }

    #[test]
    fn deposit_transfers_tokens_in_and_updates_balance() {
        let env = Env::default();
        let (contract_id, client, _admin, _operator, token, token_admin, _rcpt) = setup(&env);
        let from = Address::generate(&env);
        let token_client = TokenClient::new(&env, &token);
        let token_sac = StellarAssetClient::new(&env, &token);
        let deal_id = String::from_str(&env, "deal-1");
        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (from.clone(), 500i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&from, &500i128);
        assert_eq!(token_client.balance(&from), 500i128);
        env.mock_auths(&[MockAuth {
            address: &from,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (from.clone(), deal_id.clone(), 200i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (from.clone(), contract_id.clone(), 200i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&from, &deal_id, &200i128)
            .unwrap()
            .unwrap();
        let contract_addr = contract_id.clone();
        assert_eq!(token_client.balance(&contract_addr), 200i128);
        assert_eq!(token_client.balance(&from), 300i128);
        assert_eq!(client.balance(&deal_id), 200i128);
    }

    #[test]
    fn release_transfers_out_full_balance_and_cannot_exceed() {
        let env = Env::default();
        let (contract_id, client, admin, operator, token, token_admin, _rcpt) = setup(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        let token_client = TokenClient::new(&env, &token);
        let token_sac = StellarAssetClient::new(&env, &token);
        let deal_id = String::from_str(&env, "deal-2");
        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (from.clone(), 300i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&from, &300i128);
        env.mock_auths(&[MockAuth {
            address: &from,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (from.clone(), deal_id.clone(), 250i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (from.clone(), contract_id.clone(), 250i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&from, &deal_id, &250i128)
            .unwrap()
            .unwrap();
        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "release",
                args: (
                    operator.clone(),
                    deal_id.clone(),
                    to.clone(),
                    Symbol::new(&env, "manual_admin"),
                    String::from_str(&env, "ext1"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let released = client
            .try_release(
                &operator,
                &deal_id,
                &to,
                &Symbol::new(&env, "manual_admin"),
                &String::from_str(&env, "ext1"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(released, 250i128);
        assert_eq!(token_client.balance(&to), 250i128);
        assert_eq!(client.balance(&deal_id), 0i128);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "release",
                args: (
                    admin.clone(),
                    deal_id.clone(),
                    to.clone(),
                    Symbol::new(&env, "manual_admin"),
                    String::from_str(&env, "ext2"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_release(
                &admin,
                &deal_id,
                &to,
                &Symbol::new(&env, "manual_admin"),
                &String::from_str(&env, "ext2"),
            )
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InsufficientBalance);
    }

    #[test]
    fn paused_blocks_operations() {
        let env = Env::default();
        let (contract_id, client, admin, _operator, token, token_admin, _rcpt) = setup(&env);
        let from = Address::generate(&env);
        let token_sac = StellarAssetClient::new(&env, &token);
        let deal_id = String::from_str(&env, "deal-3");
        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (from.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&from, &100i128);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_pause(&admin).unwrap().unwrap();
        env.mock_auths(&[MockAuth {
            address: &from,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (from.clone(), deal_id.clone(), 10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_deposit(&from, &deal_id, &10i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::Paused);
    }

    #[test]
    fn unauthorized_release_rejected() {
        let env = Env::default();
        let (contract_id, client, _admin, _operator, token, token_admin, _rcpt) = setup(&env);
        let from = Address::generate(&env);
        let non_auth = Address::generate(&env);
        let to = Address::generate(&env);
        let token_sac = StellarAssetClient::new(&env, &token);
        let deal_id = String::from_str(&env, "deal-4");
        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (from.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&from, &50i128);
        env.mock_auths(&[MockAuth {
            address: &from,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (from.clone(), deal_id.clone(), 50i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (from.clone(), contract_id.clone(), 50i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&from, &deal_id, &50i128)
            .unwrap()
            .unwrap();
        env.mock_auths(&[MockAuth {
            address: &non_auth,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "release",
                args: (
                    non_auth.clone(),
                    deal_id.clone(),
                    to.clone(),
                    Symbol::new(&env, "manual_admin"),
                    String::from_str(&env, "ext3"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_release(
                &non_auth,
                &deal_id,
                &to,
                &Symbol::new(&env, "manual_admin"),
                &String::from_str(&env, "ext3"),
            )
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::NotAuthorized);
    }

    // ── Upgrade governance tests (#392) ───────────────────────────────────────

    #[test]
    fn propose_upgrade_stores_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _operator, _token, _token_admin, _rcpt) = setup(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_propose_upgrade(&admin, &hash).unwrap().unwrap();
    }

    #[test]
    fn execute_upgrade_fails_when_delay_not_met() {
        let env = Env::default();
        let (contract_id, client, admin, _operator, _token, _token_admin, _rcpt) = setup(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_upgrade_delay",
                args: (admin.clone(), 3600u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_upgrade_delay(&admin, &3600u64)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_propose_upgrade(&admin, &hash).unwrap().unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "execute_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_execute_upgrade(&admin, &hash)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::UpgradeDelayNotMet);
    }

    #[test]
    fn cancel_upgrade_clears_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _operator, _token, _token_admin, _rcpt) = setup(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_propose_upgrade(&admin, &hash).unwrap().unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "cancel_upgrade",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_cancel_upgrade(&admin).unwrap().unwrap();
    }

    #[test]
    fn migrate_from_v1_to_v3_is_applied_once() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, client, admin, _operator, token, token_admin, _rcpt) = setup(&env);
        let from = Address::generate(&env);
        let deal_id = String::from_str(&env, "legacy-v1-deal");
        let token_sac = StellarAssetClient::new(&env, &token);
        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (from.clone(), 120i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&from, &120i128);
        env.mock_auths(&[MockAuth {
            address: &from,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (from.clone(), deal_id.clone(), 120i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (from.clone(), contract_id.clone(), 120i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&from, &deal_id, &120i128)
            .unwrap()
            .unwrap();

        env.as_contract(&contract_id, || {
            env.storage()
                .instance()
                .set(&DataKey::StorageSchemaVersion, &1u32);
        });

        let deal_ids = soroban_sdk::Vec::from_array(&env, [deal_id.clone()]);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "migrate_storage_schema",
                args: (admin.clone(), 1u32, deal_ids.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_migrate_storage_schema(&admin, &1u32, &deal_ids)
            .unwrap()
            .unwrap();
        assert_eq!(client.storage_schema_version(), 3u32);

        env.mock_all_auths();
        client
            .try_migrate_storage_schema(&admin, &1u32, &deal_ids)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn migration_invariant_violation_halts() {
        let env = Env::default();
        let (contract_id, client, admin, _operator, _token, _token_admin, _rcpt) = setup(&env);
        let deal_id = String::from_str(&env, "legacy-v2-bad");
        env.as_contract(&contract_id, || {
            env.storage()
                .instance()
                .set(&DataKey::StorageSchemaVersion, &2u32);
            env.storage()
                .persistent()
                .set(&DataKey::DealBalance(deal_id.clone()), &100i128);
            env.storage()
                .persistent()
                .set(&DataKey::LegacyLockedAmountV2(deal_id.clone()), &80i128);
            env.storage()
                .persistent()
                .set(&DataKey::LegacyPendingPayoutV2(deal_id.clone()), &10i128);
        });

        let deal_ids = soroban_sdk::Vec::from_array(&env, [deal_id.clone()]);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "migrate_storage_schema",
                args: (admin.clone(), 2u32, deal_ids.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_migrate_storage_schema(&admin, &2u32, &deal_ids)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::MigrationInvariantViolation);
        assert_eq!(client.storage_schema_version(), 2u32);
    }

    #[test]
    fn disputed_rent_release_resolves_once_without_double_settlement() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, client, admin, operator, token, token_admin, _rcpt) = setup(&env);
        let depositor = Address::generate(&env);
        let landlord = Address::generate(&env);
        let resolver = Address::generate(&env);
        let deal_id = String::from_str(&env, "rent-dispute-1");
        let token_sac = StellarAssetClient::new(&env, &token);
        let token_client = TokenClient::new(&env, &token);

        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (depositor.clone(), 200i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&depositor, &200i128);

        env.mock_auths(&[MockAuth {
            address: &depositor,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (depositor.clone(), deal_id.clone(), 150i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (depositor.clone(), contract_id.clone(), 150i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&depositor, &deal_id, &150i128)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_resolver",
                args: (admin.clone(), resolver.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_set_resolver(&admin, &resolver).unwrap().unwrap();

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "request_rent_release",
                args: (
                    operator.clone(),
                    deal_id.clone(),
                    landlord.clone(),
                    100i128,
                    Symbol::new(&env, "rent_cycle"),
                    String::from_str(&env, "invoice-1"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_request_rent_release(
                &operator,
                &deal_id,
                &landlord,
                &100i128,
                &Symbol::new(&env, "rent_cycle"),
                &String::from_str(&env, "invoice-1"),
            )
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &depositor,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "challenge_rent_release",
                args: (
                    depositor.clone(),
                    deal_id.clone(),
                    String::from_str(&env, "bank_statement_mismatch"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_challenge_rent_release(
                &depositor,
                &deal_id,
                &String::from_str(&env, "bank_statement_mismatch"),
            )
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &resolver,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "resolve_rent_dispute",
                args: (
                    resolver.clone(),
                    deal_id.clone(),
                    SettlementOutcome::RefundToDepositor,
                    String::from_str(&env, "resolver-ruling-1"),
                )
                    .into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (contract_id.clone(), depositor.clone(), 100i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_resolve_rent_dispute(
                &resolver,
                &deal_id,
                &SettlementOutcome::RefundToDepositor,
                &String::from_str(&env, "resolver-ruling-1"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(token_client.balance(&depositor), 150i128);
        let second_attempt = client.try_resolve_rent_dispute(
            &resolver,
            &deal_id,
            &SettlementOutcome::RefundToDepositor,
            &String::from_str(&env, "resolver-ruling-2"),
        );
        assert!(second_attempt.is_err());
    }

    #[test]
    fn uncontested_timeout_release_is_deterministic() {
        let env = Env::default();
        let (contract_id, client, admin, operator, token, token_admin, _rcpt) = setup(&env);
        let depositor = Address::generate(&env);
        let landlord = Address::generate(&env);
        let deal_id = String::from_str(&env, "rent-timeout-1");
        let token_sac = StellarAssetClient::new(&env, &token);
        let token_client = TokenClient::new(&env, &token);

        env.mock_auths(&[MockAuth {
            address: &token_admin,
            invoke: &MockAuthInvoke {
                contract: &token,
                fn_name: "mint",
                args: (depositor.clone(), 120i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        token_sac.mint(&depositor, &120i128);
        env.mock_auths(&[MockAuth {
            address: &depositor,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "deposit",
                args: (depositor.clone(), deal_id.clone(), 120i128).into_val(&env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &token,
                    fn_name: "transfer",
                    args: (depositor.clone(), contract_id.clone(), 120i128).into_val(&env),
                    sub_invokes: &[],
                }],
            },
        }]);
        client
            .try_deposit(&depositor, &deal_id, &120i128)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "configure_dispute_windows",
                args: (admin.clone(), 1u64, 10u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_configure_dispute_windows(&admin, &1u64, &10u64)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "request_rent_release",
                args: (
                    operator.clone(),
                    deal_id.clone(),
                    landlord.clone(),
                    90i128,
                    Symbol::new(&env, "rent_cycle"),
                    String::from_str(&env, "invoice-timeout"),
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_request_rent_release(
                &operator,
                &deal_id,
                &landlord,
                &90i128,
                &Symbol::new(&env, "rent_cycle"),
                &String::from_str(&env, "invoice-timeout"),
            )
            .unwrap()
            .unwrap();

        env.ledger().set_timestamp(env.ledger().timestamp() + 2);
        client
            .try_settle_rent_release_timeout(&deal_id)
            .unwrap()
            .unwrap();
        assert_eq!(token_client.balance(&landlord), 90i128);
        let err = client
            .try_settle_rent_release_timeout(&deal_id)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::NoPendingRelease);
    }
}
