#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Bytes, Env, String, Symbol, Vec,
};

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    /// Authorized evidence submitters
    Submitter(Address),
    /// Slash record keyed by evidence hash (duplicate detection)
    SlashRecord(Bytes),
    /// Per-actor slash count (for indexing / audit)
    SlashCount(Address),
    /// Per-actor slash balance reduction total
    SlashedAmount(Address),
    /// Jailed actors → jailed = true
    Jailed(Address),
    /// Per-actor staked balance (managed externally; tracked here for slashing)
    StakedBalance(Address),
    /// Governance-approved unjail flag (set by admin; consumed on unjail)
    UnjailApproval(Address),

    // ── Inspector bond slashing (Issue #925) ─────────────────────────────
    /// Registered bond_collateral contract; only this address may call `slash`.
    BondContract,
    /// Slash history per inspector (append-only).
    InspectorSlashHistory(Address),

    // ── Two-Phase Slashing (Issue #1082) ──────────────────────────────────
    ChallengeWindow,
    NextSlashId,
    PendingSlash(u64),
}

/// Single slash entry recorded against an inspector by the bond contract
/// (Issue #925). Distinct from `SlashEvidence`, which lives in the
/// validator-style evidence flow.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectorSlashRecord {
    pub inspection_id: String,
    pub amount: i128,
    pub reason: String,
    pub slashed_at: u64,
}

// ── Two-Phase Slashing Status and Struct ──────────────────────────────────────

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SlashStatus {
    Pending = 0,
    Finalized = 1,
    Cancelled = 2,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSlash {
    pub id: u64,
    pub actor: Address,
    pub amount: i128,
    pub deadline: u64,
    pub status: SlashStatus,
    pub is_validator: bool,
    pub evidence_hash: Option<Bytes>,
    pub offence: Offence,
    pub submitter: Option<Address>,
    pub penalty_bps: u32,
    pub inspection_id: Option<String>,
    pub reason: Option<String>,
}

// ── Slashable Offence Types ───────────────────────────────────────────────────

/// Penalty ratios expressed as basis points (1 bp = 0.01%).
/// Max stake reduction capped at 10_000 bp (100%).
pub const OFFENCE_DOUBLE_SIGN_BPS: u32 = 1_000; // 10 %
pub const OFFENCE_DOWNTIME_BPS: u32 = 100; // 1 %
pub const OFFENCE_INVALID_BLOCK_BPS: u32 = 500; // 5 %
pub const MAX_BPS: u32 = 10_000;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    /// Contract already initialized
    AlreadyInitialized = 1,
    /// Caller is not authorized
    NotAuthorized = 2,
    /// Contract is paused
    Paused = 3,
    /// Evidence was already processed (duplicate)
    DuplicateEvidence = 4,
    /// Slashed actor has no staked balance
    ZeroBalance = 5,
    /// Actor is already jailed
    AlreadyJailed = 6,
    /// Actor is not jailed (unjail attempted on unjailed actor)
    NotJailed = 7,
    /// No governance approval for unjail
    UnjailNotApproved = 8,
    /// Unknown offence type
    UnknownOffence = 9,
    /// Amount overflow or underflow
    ArithmeticError = 9,
    /// Slash ID not found
    SlashNotFound = 10,
    /// Try to finalize before the challenge window has elapsed
    ChallengeWindowNotElapsed = 11,
    /// Try to resolve or finalize an already resolved/finalized slash
    SlashAlreadyResolved = 12,
    /// Invalid slash amount (e.g. <= 0)
    InvalidAmount = 13,
    ArithmeticError = 10,
}

// ── Data Structures ───────────────────────────────────────────────────────────

/// Offence classification submitted with evidence
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offence {
    DoubleSign,
    Downtime,
    InvalidBlock,
    None,
}

/// Full evidence record stored on-chain (keyed by hash)
#[contracttype]
#[derive(Clone)]
pub struct SlashEvidence {
    /// The penalized actor
    pub actor: Address,
    /// Offence type
    pub offence: Offence,
    /// Evidence submitter
    pub submitter: Address,
    /// Ledger timestamp of submission
    pub submitted_at: u64,
    /// Basis-point penalty that was applied
    pub penalty_bps: u32,
    /// Token amount slashed
    pub slashed_amount: i128,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct SlashingModule;

#[contractimpl]
impl SlashingModule {
    // ── Initialization ────────────────────────────────────────────────────────

    pub fn init(env: Env, admin: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.events().publish(
            (Symbol::new(&env, "slashing"), Symbol::new(&env, "init")),
            admin,
        );
        Ok(())
    }

    // ── Admin helpers ─────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotAuthorized)?;
        caller.require_auth();
        if caller != &admin {
            return Err(ContractError::NotAuthorized);
        }
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), ContractError> {
        if env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            return Err(ContractError::Paused);
        }
        Ok(())
    }

    /// Register an authorized evidence submitter.
    pub fn set_submitter(
        env: Env,
        admin: Address,
        submitter: Address,
        enabled: bool,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::Submitter(submitter.clone()), &enabled);
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "set_submitter"),
                submitter,
            ),
            enabled,
        );
        Ok(())
    }

    fn require_submitter(env: &Env, caller: &Address) -> Result<(), ContractError> {
        caller.require_auth();
        let enabled: bool = env
            .storage()
            .instance()
            .get(&DataKey::Submitter(caller.clone()))
            .unwrap_or(false);
        if !enabled {
            return Err(ContractError::NotAuthorized);
        }
        Ok(())
    }

    // ── Balance management ────────────────────────────────────────────────────

    /// Deposit / update an actor's staked balance (called by staking contracts or admin).
    pub fn set_staked_balance(
        env: Env,
        admin: Address,
        actor: Address,
        balance: i128,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::StakedBalance(actor), &balance);
        Ok(())
    }

    pub fn staked_balance(env: Env, actor: Address) -> i128 {
        env.storage()
            .persistent()
            .get::<_, i128>(&DataKey::StakedBalance(actor))
            .unwrap_or(0)
    }

    // ── Two-Phase Slashing Settings ──────────────────────────────────────────

    /// Configure the challenge window duration. Admin-only.
    pub fn set_challenge_window(
        env: Env,
        admin: Address,
        window_seconds: u64,
    ) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::ChallengeWindow, &window_seconds);
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "challenge_window_updated"),
            ),
            window_seconds,
        );
        Ok(())
    }

    /// Read the current challenge window duration.
    pub fn challenge_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::ChallengeWindow)
            .unwrap_or(604_800) // 7 days default
    }

    fn next_slash_id(env: &Env) -> u64 {
        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextSlashId)
            .unwrap_or(0);
        let next = id + 1;
        env.storage().instance().set(&DataKey::NextSlashId, &next);
        next
    }

    // ── Core: submit evidence & slash ─────────────────────────────────────────

    /// Submit evidence of misbehavior (proposes a validator slash).
    ///
    /// * `evidence_hash` – unique fingerprint of the raw evidence bytes (duplicate guard).
    /// * `actor`         – address being slashed.
    /// * `offence`       – classification used to look up the penalty ratio.
    pub fn submit_evidence(
        env: Env,
        submitter: Address,
        evidence_hash: Bytes,
        actor: Address,
        offence: Offence,
    ) -> Result<u64, ContractError> {
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_submitter(&env, &submitter)?;

        // Duplicate evidence check
        if env
            .storage()
            .persistent()
            .has(&DataKey::SlashRecord(evidence_hash.clone()))
        {
            return Err(ContractError::DuplicateEvidence);
        }

        // Actor must not already be jailed (cannot re-slash a jailed actor for a
        // new offence until governance unjails them; prevents double-jailing races).
        let already_jailed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Jailed(actor.clone()))
            .unwrap_or(false);
        if already_jailed {
            return Err(ContractError::AlreadyJailed);
        }

        // Determine penalty ratio
        let penalty_bps: u32 = match offence {
            Offence::DoubleSign => OFFENCE_DOUBLE_SIGN_BPS,
            Offence::Downtime => OFFENCE_DOWNTIME_BPS,
            Offence::InvalidBlock => OFFENCE_INVALID_BLOCK_BPS,
            Offence::None => return Err(ContractError::UnknownOffence),
        };

        // Load current staked balance
        let balance: i128 = env
            .storage()
            .persistent()
            .get::<_, i128>(&DataKey::StakedBalance(actor.clone()))
            .unwrap_or(0);

        if balance == 0 {
            return Err(ContractError::ZeroBalance);
        }

        // Proportional slash – saturating at full balance (no over-slash)
        let slash_amount = (balance * penalty_bps as i128) / MAX_BPS as i128;
        let slash_amount = slash_amount.min(balance); // cap at balance

        // Generate slash ID
        let slash_id = Self::next_slash_id(&env);
        let deadline = env.ledger().timestamp() + Self::challenge_window(env.clone());

        // Create PendingSlash
        let pending = PendingSlash {
            id: slash_id,
            actor: actor.clone(),
            amount: slash_amount,
            deadline,
            status: SlashStatus::Pending,
            is_validator: true,
            evidence_hash: Some(evidence_hash.clone()),
            offence: offence.clone(),
            submitter: Some(submitter.clone()),
            penalty_bps,
            inspection_id: None,
            reason: None,
        };

        // Save PendingSlash
        env.storage()
            .persistent()
            .set(&DataKey::PendingSlash(slash_id), &pending);

        // Mark evidence hash as used/pending to block duplicate proposals with the same hash
        let evidence_record = SlashEvidence {
            actor: actor.clone(),
            offence: offence.clone(),
            submitter: submitter.clone(),
            submitted_at: env.ledger().timestamp(),
            penalty_bps,
            slashed_amount: slash_amount,
        };
        env.storage().persistent().set(
            &DataKey::SlashRecord(evidence_hash.clone()),
            &evidence_record,
        );

        // Emit proposed event
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "proposed"),
                actor,
            ),
            (slash_id, slash_amount, deadline),
        );

        Ok(slash_id)
    }

    /// Propose a slash for an actor.
    /// Callable by authorized submitter or registered bond contract or admin.
    pub fn propose_slash(
        env: Env,
        submitter: Address,
        actor: Address,
        amount: i128,
    ) -> Result<u64, ContractError> {
        // Enforce authorization: caller must be Admin, authorized Submitter, or BondContract
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotAuthorized)?;

        let mut is_authorized = false;
        if submitter == admin {
            is_authorized = true;
        } else {
            let is_sub: bool = env
                .storage()
                .instance()
                .get(&DataKey::Submitter(submitter.clone()))
                .unwrap_or(false);
            let is_bond = Some(submitter.clone()) == Self::bond_contract(env.clone());
            if is_sub || is_bond {
                is_authorized = true;
            }
        }

        if !is_authorized {
            return Err(ContractError::NotAuthorized);
        }
        submitter.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let slash_id = Self::next_slash_id(&env);
        let deadline = env.ledger().timestamp() + Self::challenge_window(env.clone());

        let pending = PendingSlash {
            id: slash_id,
            actor: actor.clone(),
            amount,
            deadline,
            status: SlashStatus::Pending,
            is_validator: false,
            evidence_hash: None,
            offence: Offence::None,
            submitter: Some(submitter),
            penalty_bps: 0,
            inspection_id: None,
            reason: None,
        };

        env.storage()
            .persistent()
            .set(&DataKey::PendingSlash(slash_id), &pending);

        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "proposed"),
                actor,
            ),
            (slash_id, amount, deadline),
        );

        Ok(slash_id)
    }

    /// Finalize a pending slash after the challenge window has elapsed.
    pub fn finalize_slash(env: Env, caller: Address, slash_id: u64) -> Result<(), ContractError> {
        // Enforce authorization: caller must be Admin, authorized Submitter, or BondContract
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotAuthorized)?;

        let mut is_authorized = false;
        if caller == admin {
            is_authorized = true;
        } else {
            let is_sub: bool = env
                .storage()
                .instance()
                .get(&DataKey::Submitter(caller.clone()))
                .unwrap_or(false);
            let is_bond = Some(caller.clone()) == Self::bond_contract(env.clone());
            if is_sub || is_bond {
                is_authorized = true;
            }
        }

        if !is_authorized {
            return Err(ContractError::NotAuthorized);
        }
        caller.require_auth();

        let mut pending: PendingSlash = env
            .storage()
            .persistent()
            .get(&DataKey::PendingSlash(slash_id))
            .ok_or(ContractError::SlashNotFound)?;

        if pending.status != SlashStatus::Pending {
            return Err(ContractError::SlashAlreadyResolved);
        }

        if env.ledger().timestamp() < pending.deadline {
            return Err(ContractError::ChallengeWindowNotElapsed);
        }

        pending.status = SlashStatus::Finalized;
        env.storage()
            .persistent()
            .set(&DataKey::PendingSlash(slash_id), &pending);

        if pending.is_validator {
            // Apply validator slashing logic
            let actor = pending.actor.clone();
            let slash_amount = pending.amount;

            // Load current staked balance
            let balance: i128 = env
                .storage()
                .persistent()
                .get::<_, i128>(&DataKey::StakedBalance(actor.clone()))
                .unwrap_or(0);

            let actual_slash = slash_amount.min(balance);
            let new_balance = balance - actual_slash;

            // Apply balance reduction atomically
            env.storage()
                .persistent()
                .set(&DataKey::StakedBalance(actor.clone()), &new_balance);

            // Track cumulative slash amount per actor
            let prev_total: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::SlashedAmount(actor.clone()))
                .unwrap_or(0);
            env.storage().persistent().set(
                &DataKey::SlashedAmount(actor.clone()),
                &(prev_total + actual_slash),
            );

            // Increment slash count
            let prev_count: u32 = env
                .storage()
                .persistent()
                .get(&DataKey::SlashCount(actor.clone()))
                .unwrap_or(0);
            env.storage()
                .persistent()
                .set(&DataKey::SlashCount(actor.clone()), &(prev_count + 1));

            // Publish finalized event
            let evidence_record = SlashEvidence {
                actor: actor.clone(),
                offence: pending.offence.clone(),
                submitter: pending.submitter.clone().unwrap_or(caller.clone()),
                submitted_at: env.ledger().timestamp(),
                penalty_bps: pending.penalty_bps,
                slashed_amount: actual_slash,
            };

            // Overwrite slash record with finalized amount
            if let Some(hash) = pending.evidence_hash.clone() {
                env.storage()
                    .persistent()
                    .set(&DataKey::SlashRecord(hash), &evidence_record);
            }

            // Emit slash event
            env.events().publish(
                (
                    Symbol::new(&env, "slashing"),
                    Symbol::new(&env, "slashed"),
                    actor.clone(),
                ),
                evidence_record.clone(),
            );

            // Jail the actor
            env.storage()
                .persistent()
                .set(&DataKey::Jailed(actor.clone()), &true);

            env.events().publish(
                (
                    Symbol::new(&env, "slashing"),
                    Symbol::new(&env, "jailed"),
                    actor.clone(),
                ),
                evidence_record,
            );
        }

        // Emit general finalized event
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "finalized"),
                pending.actor.clone(),
            ),
            slash_id,
        );

        Ok(())
    }

    /// Cancel a pending slash during the challenge window.
    /// Only callable by the admin (arbiter).
    pub fn cancel_slash(env: Env, admin: Address, slash_id: u64) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;

        let mut pending: PendingSlash = env
            .storage()
            .persistent()
            .get(&DataKey::PendingSlash(slash_id))
            .ok_or(ContractError::SlashNotFound)?;

        if pending.status != SlashStatus::Pending {
            return Err(ContractError::SlashAlreadyResolved);
        }

        pending.status = SlashStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::PendingSlash(slash_id), &pending);

        // If it was a validator slash, remove the duplicate evidence guard
        if pending.is_validator {
            if let Some(hash) = pending.evidence_hash {
                env.storage()
                    .persistent()
                    .remove(&DataKey::SlashRecord(hash));
            }
        }

        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "cancelled"),
                pending.actor,
            ),
            slash_id,
        );

        Ok(())
    }

    /// Query the pending slash details.
    pub fn get_pending_slash(env: Env, slash_id: u64) -> Option<PendingSlash> {
        env.storage()
            .persistent()
            .get(&DataKey::PendingSlash(slash_id))
    }

    // ── Jailing queries ───────────────────────────────────────────────────────

    pub fn is_jailed(env: Env, actor: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Jailed(actor))
            .unwrap_or(false)
    }

    pub fn slash_count(env: Env, actor: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::SlashCount(actor))
            .unwrap_or(0)
    }

    pub fn total_slashed(env: Env, actor: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::SlashedAmount(actor))
            .unwrap_or(0)
    }

    // ── Governance unjail ─────────────────────────────────────────────────────

    /// Admin pre-approves unjail for an actor (governance step).
    pub fn approve_unjail(env: Env, admin: Address, actor: Address) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        let jailed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Jailed(actor.clone()))
            .unwrap_or(false);
        if !jailed {
            return Err(ContractError::NotJailed);
        }
        env.storage()
            .instance()
            .set(&DataKey::UnjailApproval(actor.clone()), &true);
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "unjail_approved"),
            ),
            (admin, actor),
        );
        Ok(())
    }

    /// Actor claims their governance-approved unjail.
    pub fn unjail(env: Env, actor: Address) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        actor.require_auth();

        let jailed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Jailed(actor.clone()))
            .unwrap_or(false);
        if !jailed {
            return Err(ContractError::NotJailed);
        }

        let approved: bool = env
            .storage()
            .instance()
            .get(&DataKey::UnjailApproval(actor.clone()))
            .unwrap_or(false);
        if !approved {
            return Err(ContractError::UnjailNotApproved);
        }

        // Consume approval and lift jail
        env.storage()
            .instance()
            .remove(&DataKey::UnjailApproval(actor.clone()));
        env.storage()
            .persistent()
            .set(&DataKey::Jailed(actor.clone()), &false);

        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "unjailed"),
                actor.clone(),
            ),
            env.ledger().timestamp(),
        );
        Ok(())
    }

    // ── Inspector bond slashing (Issue #925) ──────────────────────────────────
    //
    // Companion flow to the validator slashing above: the `bond_collateral`
    // contract calls `slash` here when an admin decides an inspector's
    // collateral should be reduced for a specific inspection. We gate `slash`
    // to the one registered bond contract so no other caller can record a
    // slash against an inspector.

    /// Register the bond_collateral contract address authorised to call `slash`.
    pub fn set_bond_contract(
        env: Env,
        admin: Address,
        bond_contract: Address,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::BondContract, &bond_contract);
        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "set_bond_contract"),
            ),
            bond_contract,
        );
        Ok(())
    }

    /// Currently-registered bond contract, if any.
    pub fn bond_contract(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::BondContract)
    }

    /// Record a slash against an inspector for a specific inspection. Only
    /// callable by the registered bond contract; the caller is checked against
    /// both Soroban auth and the registered address. Returns the amount slashed.
    pub fn slash(
        env: Env,
        caller: Address,
        inspector: Address,
        amount: i128,
        inspection_id: String,
        reason: String,
    ) -> Result<i128, ContractError> {
        Self::require_not_paused(&env)?;
        let registered: Address = env
            .storage()
            .instance()
            .get(&DataKey::BondContract)
            .ok_or(ContractError::NotAuthorized)?;
        if caller != registered {
            return Err(ContractError::NotAuthorized);
        }
        caller.require_auth();

        if amount <= 0 {
            return Err(ContractError::ArithmeticError);
        }

        let record = InspectorSlashRecord {
            inspection_id: inspection_id.clone(),
            amount,
            reason: reason.clone(),
            slashed_at: env.ledger().timestamp(),
        };

        let key = DataKey::InspectorSlashHistory(inspector.clone());
        let mut history: Vec<InspectorSlashRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        history.push_back(record.clone());
        env.storage().persistent().set(&key, &history);

        env.events().publish(
            (
                Symbol::new(&env, "slashing"),
                Symbol::new(&env, "inspector_slashed"),
                inspector,
            ),
            (inspection_id, amount, reason),
        );

        Ok(amount)
    }

    /// Read the inspector's slash history (oldest first).
    pub fn get_slash_history(env: Env, inspector: Address) -> Vec<InspectorSlashRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::InspectorSlashHistory(inspector))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Pause the contract. Admin-only.
    pub fn pause(env: Env, admin: Address) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (Symbol::new(&env, "slashing"), Symbol::new(&env, "paused")),
            admin,
        );
        Ok(())
    }

    /// Unpause the contract. Admin-only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (Symbol::new(&env, "slashing"), Symbol::new(&env, "unpaused")),
            admin,
        );
        Ok(())
    }

    /// True iff the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{testutils::Address as _, testutils::Ledger, Bytes, Env};

    fn evidence(env: &Env, tag: &str) -> Bytes {
        Bytes::from_slice(env, tag.as_bytes())
    }

    fn setup(env: &Env) -> (Address, Address, SlashingModuleClient<'_>) {
        env.mock_all_auths();
        let id = env.register(SlashingModule, ());
        let client = SlashingModuleClient::new(env, &id);
        let admin = Address::generate(env);
        let submitter = Address::generate(env);
        client.init(&admin);
        client.set_submitter(&admin, &submitter, &true);
        (admin, submitter, client)
    }

    // Seed the actor's staked balance directly via admin helper.
    fn seed_balance(
        client: &SlashingModuleClient<'_>,
        admin: &Address,
        actor: &Address,
        amount: i128,
    ) {
        client.set_staked_balance(admin, actor, &amount);
    }

    // ── happy-path slash ──────────────────────────────────────────────────────

    #[test]
    fn valid_slash_reduces_balance_and_jails() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev1"),
            &actor,
            &Offence::DoubleSign,
        );

        // Funds not moved immediately
        assert_eq!(client.staked_balance(&actor), 10_000);
        assert_eq!(client.total_slashed(&actor), 0);
        assert!(!client.is_jailed(&actor));

        // Advance past challenge window (default 604_800 seconds)
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);

        // Finalize
        client.finalize_slash(&submitter, &slash_id);

        // 10 % of 10_000 = 1_000 slashed → 9_000 remaining
        assert_eq!(client.staked_balance(&actor), 9_000);
        assert_eq!(client.total_slashed(&actor), 1_000);
        assert_eq!(client.slash_count(&actor), 1);
        assert!(client.is_jailed(&actor));
    }

    #[test]
    fn downtime_slash_correct_ratio() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 100_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev2"),
            &actor,
            &Offence::Downtime,
        );

        // Advance time
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        client.finalize_slash(&submitter, &slash_id);

        // 1 % of 100_000 = 1_000
        assert_eq!(client.staked_balance(&actor), 99_000);
    }

    // ── duplicate evidence ────────────────────────────────────────────────────

    #[test]
    fn duplicate_evidence_rejected() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);
        let actor2 = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);
        seed_balance(&client, &admin, &actor2, 10_000);

        let ev = evidence(&env, "same_hash");
        client.submit_evidence(&submitter, &ev, &actor, &Offence::Downtime);

        // Second submission with same hash must fail immediately even if not finalized
        let result = client.try_submit_evidence(&submitter, &ev, &actor2, &Offence::Downtime);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::DuplicateEvidence
        );
    }

    // ── over-slash boundary ───────────────────────────────────────────────────

    #[test]
    fn slash_never_exceeds_balance() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        // Even if penalty ratio would exceed 100 %, balance must not go negative.
        seed_balance(&client, &admin, &actor, 1); // tiny balance

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "tiny"),
            &actor,
            &Offence::DoubleSign,
        );

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        client.finalize_slash(&submitter, &slash_id);

        assert!(client.staked_balance(&actor) >= 0);
    }

    // ── jailed actor cannot be re-slashed ────────────────────────────────────

    #[test]
    fn jailed_actor_cannot_be_slashed_again() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_j1"),
            &actor,
            &Offence::Downtime,
        );

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        client.finalize_slash(&submitter, &slash_id);
        assert!(client.is_jailed(&actor));

        let result = client.try_submit_evidence(
            &submitter,
            &evidence(&env, "ev_j2"),
            &actor,
            &Offence::InvalidBlock,
        );
        assert_eq!(result.unwrap_err().unwrap(), ContractError::AlreadyJailed);
    }

    // ── unjail path ───────────────────────────────────────────────────────────

    #[test]
    fn unjail_requires_governance_approval() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_u1"),
            &actor,
            &Offence::Downtime,
        );

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        client.finalize_slash(&submitter, &slash_id);
        assert!(client.is_jailed(&actor));

        // Unjail without approval must fail
        let result = client.try_unjail(&actor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::UnjailNotApproved
        );

        // Admin approves
        client.approve_unjail(&admin, &actor);

        // Now actor can unjail themselves
        client.unjail(&actor);
        assert!(!client.is_jailed(&actor));
    }

    #[test]
    fn unjail_cannot_be_applied_twice() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);
        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_u2"),
            &actor,
            &Offence::Downtime,
        );

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        client.finalize_slash(&submitter, &slash_id);

        client.approve_unjail(&admin, &actor);
        client.unjail(&actor);

        // Second unjail must fail – not jailed
        let result = client.try_unjail(&actor);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotJailed);
    }

    // ── unauthorized submitter ────────────────────────────────────────────────

    #[test]
    fn unauthorized_submitter_rejected() {
        let env = Env::default();
        let (admin, _submitter, client) = setup(&env);
        let actor = Address::generate(&env);
        let stranger = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let result = client.try_submit_evidence(
            &stranger,
            &evidence(&env, "ev_s1"),
            &actor,
            &Offence::Downtime,
        );
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    // ── two-phase slash lifecycle, timing & auth ──────────────────────────────

    #[test]
    fn two_phase_lifecycle_finalize_before_window_fails() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_t1"),
            &actor,
            &Offence::DoubleSign,
        );

        // Finalizing immediately (before 604,800 seconds) must fail
        let result = client.try_finalize_slash(&submitter, &slash_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::ChallengeWindowNotElapsed
        );
    }

    #[test]
    fn two_phase_lifecycle_cancel_clears_evidence_and_does_not_move_funds() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let ev = evidence(&env, "ev_t2");
        let slash_id = client.submit_evidence(&submitter, &ev, &actor, &Offence::DoubleSign);

        // Cancel during challenge window
        client.cancel_slash(&admin, &slash_id);

        // Verify status is cancelled
        let pending = client.get_pending_slash(&slash_id).unwrap();
        assert!(matches!(pending.status, SlashStatus::Cancelled));

        // Staked balance remains unchanged
        assert_eq!(client.staked_balance(&actor), 10_000);

        // Evidence can be submitted again since cancellation cleared the SlashRecord
        let new_id = client.submit_evidence(&submitter, &ev, &actor, &Offence::DoubleSign);
        assert_ne!(slash_id, new_id);
    }

    #[test]
    fn two_phase_lifecycle_auth_enforced_on_transitions() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);
        let stranger = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_t3"),
            &actor,
            &Offence::DoubleSign,
        );

        // Stranger cannot finalize
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        let res_fin = client.try_finalize_slash(&stranger, &slash_id);
        assert_eq!(res_fin.unwrap_err().unwrap(), ContractError::NotAuthorized);

        // Stranger cannot cancel
        let res_can = client.try_cancel_slash(&stranger, &slash_id);
        assert_eq!(res_can.unwrap_err().unwrap(), ContractError::NotAuthorized);

        // Admin is authorized to cancel or finalize
        client.finalize_slash(&admin, &slash_id);
    }

    #[test]
    fn configurable_challenge_window() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);

        // Check default
        assert_eq!(client.challenge_window(), 604_800);

        // Update challenge window to 1 day (86,400 seconds)
        client.set_challenge_window(&admin, &86_400);
        assert_eq!(client.challenge_window(), 86_400);

        let slash_id = client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_t4"),
            &actor,
            &Offence::DoubleSign,
        );

        // Should fail after 20 hours
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 72_000);
        let res1 = client.try_finalize_slash(&submitter, &slash_id);
        assert_eq!(
            res1.unwrap_err().unwrap(),
            ContractError::ChallengeWindowNotElapsed
        );

        // Should succeed after 25 hours
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 20_000); // 72_000 + 20_000 = 92_000 (which is > 86_400)
        client.finalize_slash(&submitter, &slash_id);
        assert_eq!(client.staked_balance(&actor), 9_000);
    }

    // ── Inspector bond slashing surface (Issue #925) ─────────────────────

    #[test]
    fn bond_contract_is_set_and_read_back() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SlashingModule, ());
        let client = SlashingModuleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.init(&admin);

        assert!(client.bond_contract().is_none());

        let bond_contract = Address::generate(&env);
        client.set_bond_contract(&admin, &bond_contract);
        assert_eq!(client.bond_contract(), Some(bond_contract));
    }

    #[test]
    fn slash_rejects_unregistered_caller_with_not_authorized() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SlashingModule, ());
        let client = SlashingModuleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.init(&admin);

        let registered = Address::generate(&env);
        client.set_bond_contract(&admin, &registered);

        let stranger = Address::generate(&env);
        let inspector = Address::generate(&env);
        let result = client.try_slash(
            &stranger,
            &inspector,
            &100,
            &soroban_sdk::String::from_str(&env, "INSP-1"),
            &soroban_sdk::String::from_str(&env, "r"),
        );
        assert_eq!(result, Err(Ok(ContractError::NotAuthorized)));
    }

    #[test]
    fn slash_rejects_non_positive_amount() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SlashingModule, ());
        let client = SlashingModuleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.init(&admin);
        let bond_contract = Address::generate(&env);
        client.set_bond_contract(&admin, &bond_contract);

        let inspector = Address::generate(&env);
        let result = client.try_slash(
            &bond_contract,
            &inspector,
            &0,
            &soroban_sdk::String::from_str(&env, "INSP-1"),
            &soroban_sdk::String::from_str(&env, "r"),
        );
        assert_eq!(result, Err(Ok(ContractError::ArithmeticError)));
    }

    #[test]
    fn slash_records_history_when_called_by_registered_bond_contract() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SlashingModule, ());
        let client = SlashingModuleClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.init(&admin);
        let bond_contract = Address::generate(&env);
        client.set_bond_contract(&admin, &bond_contract);

        let inspector = Address::generate(&env);
        let inspection = soroban_sdk::String::from_str(&env, "INSP-7");
        let reason = soroban_sdk::String::from_str(&env, "fraud");
        let slashed = client.slash(&bond_contract, &inspector, &500, &inspection, &reason);
        assert_eq!(slashed, 500);

        let history = client.get_slash_history(&inspector);
        assert_eq!(history.len(), 1);
        let entry = history.get(0).unwrap();
        assert_eq!(entry.amount, 500);
        assert_eq!(entry.inspection_id, inspection);
        assert_eq!(entry.reason, reason);
    }

    // ── Pausable tests ───────────────────────────────────────────────────────

    #[test]
    fn pause_blocks_mutating_calls() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);
        client.pause(&admin);

        // submit_evidence should fail
        let result = client.try_submit_evidence(
            &submitter,
            &evidence(&env, "ev_pause"),
            &actor,
            &Offence::Downtime,
        );
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // set_staked_balance should fail
        let result = client.try_set_staked_balance(&admin, &actor, &5_000);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // approve_unjail should fail
        let result = client.try_approve_unjail(&admin, &actor);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // set_bond_contract should fail
        let bond_contract = Address::generate(&env);
        let result = client.try_set_bond_contract(&admin, &bond_contract);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);
    }

    #[test]
    fn unpause_allows_mutating_calls() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);
        client.pause(&admin);
        client.unpause(&admin);

        // submit_evidence should succeed after unpause
        client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_unpause"),
            &actor,
            &Offence::Downtime,
        );
        assert_eq!(client.staked_balance(&actor), 9_900);
    }

    #[test]
    fn pause_requires_admin() {
        let env = Env::default();
        let (_admin, _submitter, client) = setup(&env);
        let attacker = Address::generate(&env);

        let result = client.try_pause(&attacker);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn unpause_requires_admin() {
        let env = Env::default();
        let (admin, _submitter, client) = setup(&env);
        let attacker = Address::generate(&env);

        client.pause(&admin);
        let result = client.try_unpause(&attacker);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn getters_work_while_paused() {
        let env = Env::default();
        let (admin, submitter, client) = setup(&env);
        let actor = Address::generate(&env);

        seed_balance(&client, &admin, &actor, 10_000);
        client.submit_evidence(
            &submitter,
            &evidence(&env, "ev_getter"),
            &actor,
            &Offence::Downtime,
        );
        client.pause(&admin);

        // Read-only getters should still work
        assert_eq!(client.staked_balance(&actor), 9_900);
        assert_eq!(client.total_slashed(&actor), 100);
        assert_eq!(client.slash_count(&actor), 1);
        assert!(client.is_jailed(&actor));
        assert!(client.is_paused());
    }
}
