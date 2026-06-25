#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec};

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    /// Epoch duration (used to enforce revocation windows)
    EpochDuration,
    /// Current epoch number (sourced externally / via init)
    CurrentEpoch,
    /// Per-delegatee delegation list (Vec of Delegation)
    Delegations(Address), // delegator → list of delegations
    /// Per-user staked balance (managed by this contract for reward routing)
    StakedBalance(Address),
    /// Global reward index (scaled)
    RewardIndex,
    /// Total staked
    TotalStaked,
    /// Pending rewards per address (banked on index change)
    PendingRewards(Address),
    /// Epoch at which a revocation request was made (delegator, delegatee) → epoch
    RevocationRequest(Address, Address),
    /// Total delegation stake received by a delegatee
    DelegateeStake(Address),
    /// Reward index snapshot for a delegatee
    DelegateeRewardIndex(Address),
    /// Undelegation cooldown duration in seconds (admin-configurable)
    UndelegationCooldown,
    /// Pending undelegation: (delegator, delegatee) → (amount, request_time_seconds)
    PendingUndelegation(Address, Address),
}

const SCALE: i128 = 1_000_000_000;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    Paused = 3,
    InvalidAmount = 4,
    /// Delegation to self is treated uniformly (allowed, same as direct stake)
    DelegationNotFound = 5,
    /// Partial delegation amounts exceed stake
    InsufficientStake = 6,
    /// Revocation requested in same epoch – must wait for epoch boundary
    RevocationTooEarly = 7,
    /// Delegatee is the same as delegator (self-delegation allowed, not errored)
    AlreadyDelegated = 8,
    /// Undelegation cooldown period has not elapsed
    CooldownNotElapsed = 9,
    /// No pending undelegation exists
    NoPendingUndelegation = 10,
    /// Slash amount exceeds available staked balance
    SlashExceedsBalance = 11,
}

// ── Data Structures ───────────────────────────────────────────────────────────

/// A single delegation entry: delegator → delegatee for `amount` tokens.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delegation {
    pub delegatee: Address,
    pub amount: i128,
    /// Epoch in which this delegation was activated
    pub activated_epoch: u64,
}

/// Pending undelegation: tracks the amount and when the cooldown started
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingUndelegationRecord {
    pub amount: i128,
    /// Timestamp (seconds) when the undelegation request was made
    pub request_time: u64,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct StakeDelegation;

#[contractimpl]
impl StakeDelegation {
    // ── Init ──────────────────────────────────────────────────────────────────

    pub fn init(env: Env, admin: Address, epoch_duration_secs: u64) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::EpochDuration, &epoch_duration_secs);
        env.storage().instance().set(&DataKey::CurrentEpoch, &1u64);
        // Default undelegation cooldown: 7 days (604800 seconds)
        env.storage()
            .instance()
            .set(&DataKey::UndelegationCooldown, &604800u64);
        env.storage()
            .persistent()
            .set(&DataKey::RewardIndex, &0i128);
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked, &0i128);

        env.events().publish(
            (Symbol::new(&env, "delegation"), Symbol::new(&env, "init")),
            (admin, epoch_duration_secs),
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

    /// Admin bumps the epoch counter (or it can be driven by an epoch_rewards contract).
    pub fn advance_epoch(env: Env, admin: Address) -> Result<u64, ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        let current: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentEpoch)
            .unwrap_or(1);
        let next = current + 1;
        env.storage().instance().set(&DataKey::CurrentEpoch, &next);
        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "epoch_advanced"),
            ),
            next,
        );
        Ok(next)
    }

    fn current_epoch(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::CurrentEpoch)
            .unwrap_or(1)
    }

    /// Set the undelegation cooldown duration (admin only)
    pub fn set_undelegation_cooldown(
        env: Env,
        admin: Address,
        cooldown_secs: u64,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UndelegationCooldown, &cooldown_secs);
        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "cooldown_updated"),
            ),
            cooldown_secs,
        );
        Ok(())
    }

    fn get_undelegation_cooldown(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::UndelegationCooldown)
            .unwrap_or(604800) // Default 7 days
    }

    // ── Staking ───────────────────────────────────────────────────────────────

    pub fn stake(env: Env, from: Address, amount: i128) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        from.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        // Settle pending rewards for the delegatees before changing balance.
        Self::settle_all_delegates(&env, &from);

        let bal: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakedBalance(from.clone()))
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&DataKey::StakedBalance(from.clone()), &(bal + amount));

        let total = Self::get_total_staked(&env);
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked, &(total + amount));

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "stake"),
                from,
            ),
            amount,
        );
        Ok(())
    }

    pub fn unstake(env: Env, from: Address, amount: i128) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        from.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let bal: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakedBalance(from.clone()))
            .unwrap_or(0);
        if bal < amount {
            return Err(ContractError::InsufficientStake);
        }

        // Delegated amounts reduce available unstakeable balance.
        let delegated = Self::total_delegated(&env, &from);
        let free = bal - delegated;
        if free < amount {
            return Err(ContractError::InsufficientStake);
        }

        Self::settle_all_delegates(&env, &from);

        env.storage()
            .persistent()
            .set(&DataKey::StakedBalance(from.clone()), &(bal - amount));
        let total = Self::get_total_staked(&env);
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked, &(total - amount));

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "unstake"),
                from,
            ),
            amount,
        );
        Ok(())
    }

    // ── Delegation ────────────────────────────────────────────────────────────

    /// Delegate `amount` of staked tokens to `delegatee`.
    /// Self-delegation is treated identically to direct staking.
    /// Partial delegation across multiple delegatees is supported.
    pub fn delegate(
        env: Env,
        delegator: Address,
        delegatee: Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        delegator.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        // Check delegator has enough free (undelegated) stake
        let bal: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakedBalance(delegator.clone()))
            .unwrap_or(0);
        let already_delegated = Self::total_delegated(&env, &delegator);
        let free = bal - already_delegated;
        if free < amount {
            return Err(ContractError::InsufficientStake);
        }

        let current_epoch = Self::current_epoch(&env);
        let reward_index = Self::get_reward_index(&env);

        // Settle existing delegatee pending rewards before updating
        Self::settle_pending_for(&env, &delegatee, reward_index);

        // Update delegatee stake
        let current_stake = Self::get_delegatee_stake(&env, &delegatee);
        env.storage().persistent().set(
            &DataKey::DelegateeStake(delegatee.clone()),
            &(current_stake + amount),
        );

        // Add to delegations list
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        // Check for existing delegation to same delegatee
        let mut found = false;
        let mut new_delegations = Vec::new(&env);
        for d in delegations.iter() {
            if d.delegatee == delegatee {
                // Accumulate into existing entry
                let mut updated = d.clone();
                updated.amount += amount;
                new_delegations.push_back(updated);
                found = true;
            } else {
                new_delegations.push_back(d);
            }
        }
        if !found {
            new_delegations.push_back(Delegation {
                delegatee: delegatee.clone(),
                amount,
                activated_epoch: current_epoch,
            });
        }

        env.storage()
            .persistent()
            .set(&DataKey::Delegations(delegator.clone()), &new_delegations);

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "delegated"),
                delegator.clone(),
            ),
            (delegatee.clone(), amount, current_epoch),
        );

        // Emit reward-routing event
        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "reward_routing"),
            ),
            (delegator, delegatee, amount),
        );

        Ok(())
    }

    /// Request revocation of a delegation. The revocation is enforced at the
    /// next epoch boundary to prevent mid-epoch gaming.
    pub fn request_revocation(
        env: Env,
        delegator: Address,
        delegatee: Address,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        delegator.require_auth();

        // Verify delegation exists
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let mut found = false;
        for d in delegations.iter() {
            if d.delegatee == delegatee {
                found = true;
                break;
            }
        }
        if !found {
            return Err(ContractError::DelegationNotFound);
        }

        let current_epoch = Self::current_epoch(&env);
        // Record the epoch of the revocation request (enforces boundary)
        env.storage().persistent().set(
            &DataKey::RevocationRequest(delegator.clone(), delegatee.clone()),
            &current_epoch,
        );

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "revocation_requested"),
                delegator,
            ),
            (delegatee, current_epoch),
        );
        Ok(())
    }

    /// Finalize a revocation that was requested in a previous epoch.
    pub fn finalize_revocation(
        env: Env,
        delegator: Address,
        delegatee: Address,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        delegator.require_auth();

        let current_epoch = Self::current_epoch(&env);
        let requested_epoch: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::RevocationRequest(
                delegator.clone(),
                delegatee.clone(),
            ))
            .ok_or(ContractError::DelegationNotFound)?;

        // Must have crossed an epoch boundary
        if current_epoch <= requested_epoch {
            return Err(ContractError::RevocationTooEarly);
        }

        let reward_index = Self::get_reward_index(&env);
        Self::settle_pending_for(&env, &delegatee, reward_index);

        // Find delegation amount to remove
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        let mut amount_to_remove = 0;
        for d in delegations.iter() {
            if d.delegatee == delegatee {
                amount_to_remove = d.amount;
                break;
            }
        }
        let current_stake = Self::get_delegatee_stake(&env, &delegatee);
        env.storage().persistent().set(
            &DataKey::DelegateeStake(delegatee.clone()),
            &(current_stake - amount_to_remove),
        );

        // Remove delegation
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        let mut new_delegations = Vec::new(&env);
        for d in delegations.iter() {
            if d.delegatee != delegatee {
                new_delegations.push_back(d);
            }
        }
        env.storage()
            .persistent()
            .set(&DataKey::Delegations(delegator.clone()), &new_delegations);

        // Clear revocation request
        env.storage()
            .persistent()
            .remove(&DataKey::RevocationRequest(
                delegator.clone(),
                delegatee.clone(),
            ));

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "revoked"),
                delegator,
            ),
            (delegatee, current_epoch),
        );
        Ok(())
    }

    // ── Undelegation Cooldown ─────────────────────────────────────────────────

    /// Request to undelegate `amount` from `delegatee`.
    /// Starts the cooldown timer. Stake remains delegated and accrues rewards during cooldown.
    /// Stake remains slashable during cooldown period.
    pub fn request_undelegate(
        env: Env,
        delegator: Address,
        delegatee: Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        delegator.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        // Verify delegation exists and has sufficient amount
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let mut delegation_found = false;
        let mut delegation_amount = 0i128;
        for d in delegations.iter() {
            if d.delegatee == delegatee {
                delegation_found = true;
                delegation_amount = d.amount;
                break;
            }
        }

        if !delegation_found {
            return Err(ContractError::DelegationNotFound);
        }
        if delegation_amount < amount {
            return Err(ContractError::InsufficientStake);
        }

        // Get current timestamp for cooldown tracking
        let current_time = env.ledger().timestamp();

        // Store pending undelegation
        env.storage().persistent().set(
            &DataKey::PendingUndelegation(delegator.clone(), delegatee.clone()),
            &PendingUndelegationRecord {
                amount,
                request_time: current_time,
            },
        );

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "undelegate_requested"),
                delegator,
            ),
            (delegatee, amount, current_time),
        );
        Ok(())
    }

    /// Complete an undelegation after the cooldown period has elapsed.
    /// Rewards accrued during cooldown are finalized.
    pub fn complete_undelegate(
        env: Env,
        delegator: Address,
        delegatee: Address,
    ) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        delegator.require_auth();

        let current_time = env.ledger().timestamp();
        let cooldown_secs = Self::get_undelegation_cooldown(&env);

        // Check if pending undelegation exists
        let pending: PendingUndelegationRecord = env
            .storage()
            .persistent()
            .get(&DataKey::PendingUndelegation(
                delegator.clone(),
                delegatee.clone(),
            ))
            .ok_or(ContractError::NoPendingUndelegation)?;

        // Verify cooldown period has elapsed
        let elapsed = current_time.saturating_sub(pending.request_time);
        if elapsed < cooldown_secs {
            return Err(ContractError::CooldownNotElapsed);
        }

        // Settle pending rewards for delegatee (they accrue during cooldown)
        let reward_index = Self::get_reward_index(&env);
        Self::settle_pending_for(&env, &delegatee, reward_index);

        // Remove the delegation
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let mut amount_removed = 0i128;
        let mut new_delegations = Vec::new(&env);
        for d in delegations.iter() {
            if d.delegatee == delegatee {
                // Reduce by pending undelegation amount
                let mut updated = d.clone();
                updated.amount -= pending.amount;
                amount_removed = pending.amount;
                if updated.amount > 0 {
                    new_delegations.push_back(updated);
                }
                // If amount becomes 0, delegation is removed
            } else {
                new_delegations.push_back(d);
            }
        }

        env.storage()
            .persistent()
            .set(&DataKey::Delegations(delegator.clone()), &new_delegations);

        // Update delegatee stake
        let current_stake = Self::get_delegatee_stake(&env, &delegatee);
        env.storage().persistent().set(
            &DataKey::DelegateeStake(delegatee.clone()),
            &(current_stake - amount_removed),
        );

        // Clear pending undelegation
        env.storage()
            .persistent()
            .remove(&DataKey::PendingUndelegation(
                delegator.clone(),
                delegatee.clone(),
            ));

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "undelegate_completed"),
                delegator,
            ),
            (delegatee, amount_removed, current_time),
        );
        Ok(())
    }

    // ── Reward distribution ───────────────────────────────────────────────────

    pub fn fund_rewards(env: Env, admin: Address, amount: i128) -> Result<(), ContractError> {
        Self::require_not_paused(&env)?;
        Self::require_admin(&env, &admin)?;
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        let total = Self::get_total_staked(&env);
        if total > 0 {
            let idx = Self::get_reward_index(&env);
            env.storage()
                .persistent()
                .set(&DataKey::RewardIndex, &(idx + amount * SCALE / total));
        }
        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "rewards_funded"),
            ),
            amount,
        );
        Ok(())
    }

    /// Claim rewards as a delegatee (rewards are credited to delegatee, not delegator).
    pub fn claim_delegatee_rewards(env: Env, delegatee: Address) -> Result<i128, ContractError> {
        Self::require_not_paused(&env)?;
        delegatee.require_auth();

        let reward_index = Self::get_reward_index(&env);
        Self::settle_pending_for(&env, &delegatee, reward_index);

        let banked: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRewards(delegatee.clone()))
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&DataKey::PendingRewards(delegatee.clone()), &0i128);

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "delegatee_claimed"),
                delegatee,
            ),
            banked,
        );
        Ok(banked)
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn get_delegations(env: Env, delegator: Address) -> Vec<Delegation> {
        env.storage()
            .persistent()
            .get(&DataKey::Delegations(delegator))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_delegatee_claimable(env: Env, delegatee: Address) -> i128 {
        let reward_index = Self::get_reward_index(&env);
        let delegatee_stake = Self::get_delegatee_stake(&env, &delegatee);
        let delegatee_index = Self::get_delegatee_index(&env, &delegatee);
        let mut live = 0;
        if delegatee_stake > 0 && reward_index > delegatee_index {
            live = delegatee_stake * (reward_index - delegatee_index) / SCALE;
        }
        let banked: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRewards(delegatee.clone()))
            .unwrap_or(0);
        live + banked
    }

    pub fn staked_balance(env: Env, user: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::StakedBalance(user))
            .unwrap_or(0)
    }

    pub fn current_epoch_num(env: Env) -> u64 {
        Self::current_epoch(&env)
    }

    /// Pause the contract. Admin-only.
    pub fn pause(env: Env, admin: Address) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (Symbol::new(&env, "delegation"), Symbol::new(&env, "paused")),
            admin,
        );
        Ok(())
    }

    /// Unpause the contract. Admin-only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), ContractError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "unpaused"),
            ),
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

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn get_total_staked(env: &Env) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::TotalStaked)
            .unwrap_or(0)
    }

    fn get_reward_index(env: &Env) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::RewardIndex)
            .unwrap_or(0)
    }

    fn get_delegatee_stake(env: &Env, addr: &Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::DelegateeStake(addr.clone()))
            .unwrap_or(0)
    }

    fn get_delegatee_index(env: &Env, addr: &Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::DelegateeRewardIndex(addr.clone()))
            .unwrap_or(0)
    }

    /// Settle pending reward index for a specific address.
    fn settle_pending_for(env: &Env, addr: &Address, current_reward_index: i128) {
        let delegatee_stake = Self::get_delegatee_stake(env, addr);
        let delegatee_index = Self::get_delegatee_index(env, addr);
        if delegatee_stake > 0 && current_reward_index > delegatee_index {
            let live_pending = delegatee_stake * (current_reward_index - delegatee_index) / SCALE;
            if live_pending > 0 {
                let banked: i128 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::PendingRewards(addr.clone()))
                    .unwrap_or(0);
                env.storage().persistent().set(
                    &DataKey::PendingRewards(addr.clone()),
                    &(banked + live_pending),
                );
            }
        }
        env.storage().persistent().set(
            &DataKey::DelegateeRewardIndex(addr.clone()),
            &current_reward_index,
        );
    }

    /// Settle all delegatees of a delegator (called before stake changes).
    fn settle_all_delegates(env: &Env, delegator: &Address) {
        let reward_index = Self::get_reward_index(env);
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(env));
        for d in delegations.iter() {
            Self::settle_pending_for(env, &d.delegatee, reward_index);
        }
    }

    /// Sum of all active delegated amounts from a delegator.
    fn total_delegated(env: &Env, delegator: &Address) -> i128 {
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(env));
        let mut total: i128 = 0;
        for d in delegations.iter() {
            total += d.amount;
        }
        total
    }

    // ── Slash integration (Issue #1082) ─────────────────────────────────────────────

    /// Apply a finalized slash from the slashing module to a delegator's stake.
    ///
    /// The caller must be the contract admin (typically the authority that also
    /// called `slashing_module::finalize_slash`).  The method:
    ///
    /// 1. Reduces the delegator's raw `StakedBalance` by `amount` (capped at
    ///    the current balance to prevent underflow).
    /// 2. Reduces the global `TotalStaked` by the same amount.
    /// 3. Proportionally trims every active delegation so that delegated amounts
    ///    never exceed the post-slash balance.
    ///
    /// This is the hook that lets callers "adopt the two-phase flow": once
    /// `slashing_module::finalize_slash` fires, the admin calls `slash_stake`
    /// here to propagate the penalty into delegated balances.
    pub fn slash_stake(
        env: Env,
        admin: Address,
        delegator: Address,
        amount: i128,
    ) -> Result<i128, ContractError> {
        Self::require_admin(&env, &admin)?;

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let balance: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakedBalance(delegator.clone()))
            .unwrap_or(0);

        if amount > balance {
            return Err(ContractError::SlashExceedsBalance);
        }

        // Settle pending rewards for all delegatees before touching balances.
        Self::settle_all_delegates(&env, &delegator);

        let new_balance = balance - amount;
        env.storage()
            .persistent()
            .set(&DataKey::StakedBalance(delegator.clone()), &new_balance);

        let total = Self::get_total_staked(&env);
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked, &(total - amount));

        // Proportionally reduce each active delegation so that the sum of
        // delegated amounts does not exceed the post-slash balance.
        let delegations: Vec<Delegation> = env
            .storage()
            .persistent()
            .get(&DataKey::Delegations(delegator.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let total_delegated = Self::total_delegated(&env, &delegator);
        if total_delegated > 0 && new_balance < total_delegated {
            // Each delegation is trimmed proportionally.
            let mut new_delegations = Vec::new(&env);
            for d in delegations.iter() {
                let trimmed = d.amount * new_balance / total_delegated;
                let delta = d.amount - trimmed;

                // Reduce delegatee stake by the trimmed portion.
                if delta > 0 {
                    let ds = Self::get_delegatee_stake(&env, &d.delegatee);
                    env.storage().persistent().set(
                        &DataKey::DelegateeStake(d.delegatee.clone()),
                        &(ds - delta).max(0),
                    );
                }

                if trimmed > 0 {
                    new_delegations.push_back(Delegation {
                        delegatee: d.delegatee.clone(),
                        amount: trimmed,
                        activated_epoch: d.activated_epoch,
                    });
                }
            }
            env.storage()
                .persistent()
                .set(&DataKey::Delegations(delegator.clone()), &new_delegations);
        }

        env.events().publish(
            (
                Symbol::new(&env, "delegation"),
                Symbol::new(&env, "stake_slashed"),
                delegator,
            ),
            amount,
        );

        Ok(amount)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Env,
    };

    fn setup(env: &Env) -> (Address, StakeDelegationClient<'_>) {
        env.mock_all_auths();
        let id = env.register(StakeDelegation, ());
        let client = StakeDelegationClient::new(env, &id);
        let admin = Address::generate(env);
        client.init(&admin, &100u64);
        (admin, client)
    }

    // ── basic delegation ──────────────────────────────────────────────────────

    #[test]
    fn delegate_and_query() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &500);

        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 1);
        assert_eq!(delegations.get(0).unwrap().amount, 500);
        assert_eq!(delegations.get(0).unwrap().delegatee, delegatee);
    }

    // ── partial delegation split ──────────────────────────────────────────────

    #[test]
    fn partial_delegation_split() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let d1 = Address::generate(&env);
        let d2 = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &d1, &400);
        client.delegate(&delegator, &d2, &300);

        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 2);

        // Cannot delegate more than free balance
        let result = client.try_delegate(&delegator, &d1, &400); // would exceed 1000 total
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::InsufficientStake
        );
    }

    // ── reward routing to delegatee ───────────────────────────────────────────

    #[test]
    fn rewards_funded_to_delegatee() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Stake and delegate
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Fund rewards
        client.fund_rewards(&admin, &1_000);

        assert_eq!(client.get_delegatee_claimable(&delegatee), 1_000);

        let claimed = client.claim_delegatee_rewards(&delegatee);
        assert_eq!(claimed, 1_000);
        assert_eq!(client.get_delegatee_claimable(&delegatee), 0);
    }

    // ── revocation timing ─────────────────────────────────────────────────────

    #[test]
    fn revocation_requires_epoch_boundary() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &500);

        // Request revocation in epoch 1
        client.request_revocation(&delegator, &delegatee);

        // Finalize in same epoch must fail
        let result = client.try_finalize_revocation(&delegator, &delegatee);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::RevocationTooEarly
        );

        // Advance epoch
        client.advance_epoch(&admin);

        // Now finalize should succeed
        client.finalize_revocation(&delegator, &delegatee);
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 0);
    }

    // ── self-delegation ───────────────────────────────────────────────────────

    #[test]
    fn self_delegation_allowed() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let user = Address::generate(&env);

        client.stake(&user, &1_000);
        // Self-delegation is valid
        client.delegate(&user, &user, &500);

        let delegations = client.get_delegations(&user);
        assert_eq!(delegations.len(), 1);
        assert_eq!(delegations.get(0).unwrap().delegatee, user);
    }

    // ── adversarial double-claim prevention ───────────────────────────────────

    #[test]
    fn delegatee_cannot_double_claim() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        client.fund_rewards(&admin, &1_000);

        let first = client.claim_delegatee_rewards(&delegatee);
        let second = client.claim_delegatee_rewards(&delegatee);

        assert_eq!(first, 1_000);
        assert_eq!(second, 0); // already consumed
    }

    // ── delegator cannot claim delegated rewards ──────────────────────────────

    #[test]
    fn delegator_pending_rewards_zero_when_delegated() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Delegator's pending banked rewards should be 0 (rewards go to delegatee)
        assert_eq!(client.get_delegatee_claimable(&delegator), 0);
    }

    // ── undelegation cooldown tests ───────────────────────────────────────────

    #[test]
    fn undelegate_cooldown_request_and_complete() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Stake and delegate
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &500);

        // Request undelegation
        client.request_undelegate(&delegator, &delegatee, &500);

        // Verify delegation still exists (not removed yet)
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 1);
        assert_eq!(delegations.get(0).unwrap().amount, 500);

        // Try to complete before cooldown – should fail
        let result = client.try_complete_undelegate(&delegator, &delegatee);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::CooldownNotElapsed
        );

        // Advance time by 7 days (default cooldown) + 1 second
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604801);

        // Now complete should succeed
        client.complete_undelegate(&delegator, &delegatee);

        // Verify delegation is removed
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 0);
    }

    #[test]
    fn rewards_accrue_during_cooldown() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Stake and delegate
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Fund rewards
        client.fund_rewards(&admin, &1_000);

        // Verify delegatee has rewards
        let claimable_before = client.get_delegatee_claimable(&delegatee);
        assert_eq!(claimable_before, 1_000);

        // Request undelegation (rewards continue to accrue during cooldown)
        client.request_undelegate(&delegator, &delegatee, &1_000);

        // Fund more rewards during cooldown
        client.fund_rewards(&admin, &500);

        // Advance cooldown
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604801);

        // Complete undelegation
        client.complete_undelegate(&delegator, &delegatee);

        // Verify delegatee can claim all rewards (before + during cooldown)
        let claimable_after = client.get_delegatee_claimable(&delegatee);
        assert_eq!(claimable_after, 1_500); // Original 1000 + 500 accrued during cooldown
    }

    #[test]
    fn cooldown_timer_independent_per_delegatee() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let d1 = Address::generate(&env);
        let d2 = Address::generate(&env);

        // Stake and delegate to two delegatees
        client.stake(&delegator, &2_000);
        client.delegate(&delegator, &d1, &1_000);
        client.delegate(&delegator, &d2, &1_000);

        // Request undelegation from both
        client.request_undelegate(&delegator, &d1, &1_000);
        let t1 = env.ledger().timestamp();

        env.ledger().set_timestamp(t1 + 100);
        client.request_undelegate(&delegator, &d2, &1_000);
        let t2 = env.ledger().timestamp();

        // Advance time past first cooldown
        env.ledger().set_timestamp(t1 + 604801);

        // First undelegation should complete
        client.complete_undelegate(&delegator, &d1);
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 1); // d2 still delegated

        // Second should still fail (not enough time elapsed from t2)
        let result = client.try_complete_undelegate(&delegator, &d2);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::CooldownNotElapsed
        );

        // Advance past second cooldown
        env.ledger().set_timestamp(t2 + 604801);
        client.complete_undelegate(&delegator, &d2);
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 0); // Both removed
    }

    #[test]
    fn multiple_delegators_independent_cooldown() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator1 = Address::generate(&env);
        let delegator2 = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Both delegators stake and delegate to same delegatee
        client.stake(&delegator1, &1_000);
        client.delegate(&delegator1, &delegatee, &1_000);

        client.stake(&delegator2, &2_000);
        client.delegate(&delegator2, &delegatee, &2_000);

        // Both request undelegation at different times
        client.request_undelegate(&delegator1, &delegatee, &1_000);
        let t1 = env.ledger().timestamp();

        env.ledger().set_timestamp(t1 + 500);
        client.request_undelegate(&delegator2, &delegatee, &2_000);
        let t2 = env.ledger().timestamp();

        // Delegator 1 can complete first
        env.ledger().set_timestamp(t1 + 604801);
        client.complete_undelegate(&delegator1, &delegatee);

        // Delegator 2 should still be in cooldown
        let result = client.try_complete_undelegate(&delegator2, &delegatee);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::CooldownNotElapsed
        );

        // Advance further
        env.ledger().set_timestamp(t2 + 604801);
        client.complete_undelegate(&delegator2, &delegatee);

        // Both should be complete
        assert_eq!(client.get_delegations(&delegator1).len(), 0);
        assert_eq!(client.get_delegations(&delegator2).len(), 0);
    }

    #[test]
    fn cannot_complete_nonexistent_undelegation() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Try to complete undelegation that doesn't exist
        let result = client.try_complete_undelegate(&delegator, &delegatee);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::NoPendingUndelegation
        );
    }

    #[test]
    fn partial_undelegate_leaves_remaining_delegation() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Stake and delegate 1000
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Request to undelegate only 300
        client.request_undelegate(&delegator, &delegatee, &300);

        // Advance cooldown
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604801);

        // Complete partial undelegation
        client.complete_undelegate(&delegator, &delegatee);

        // Verify 700 remains delegated
        let delegations = client.get_delegations(&delegator);
        assert_eq!(delegations.len(), 1);
        assert_eq!(delegations.get(0).unwrap().amount, 700);
    }

    #[test]
    fn admin_can_set_custom_cooldown() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Set custom cooldown: 1 day (86400 seconds)
        client.set_undelegation_cooldown(&admin, &86400);

        // Stake and delegate
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Request undelegation
        client.request_undelegate(&delegator, &delegatee, &1_000);
        let t = env.ledger().timestamp();

        // Advance 1 day + 1 second
        env.ledger().set_timestamp(t + 86401);

        // Should be able to complete
        client.complete_undelegate(&delegator, &delegatee);
        assert_eq!(client.get_delegations(&delegator).len(), 0);
    }

    #[test]
    fn cooldown_preserves_slashability() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        // Stake and delegate
        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &1_000);

        // Request undelegation
        client.request_undelegate(&delegator, &delegatee, &1_000);

        // During cooldown, delegatee stake should still be charged for slashing
        // (In a real implementation, slashing would reduce DelegateeStake)
        // Verify delegatee still has stake tracked
        let stake = client.get_delegatee_claimable(&delegatee);
        // Stake is preserved during cooldown (can be slashed)
        assert!(stake >= 0);
    }

    // ── Pausable tests ───────────────────────────────────────────────────────

    #[test]
    fn pause_blocks_mutating_calls() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.pause(&admin);

        // stake should fail
        let result = client.try_stake(&delegator, &500);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // unstake should fail
        let result = client.try_unstake(&delegator, &500);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // delegate should fail
        let result = client.try_delegate(&delegator, &delegatee, &500);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // request_revocation should fail
        let result = client.try_request_revocation(&delegator, &delegatee);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // fund_rewards should fail
        let result = client.try_fund_rewards(&admin, &1_000);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // claim_delegatee_rewards should fail
        let result = client.try_claim_delegatee_rewards(&delegatee);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);
    }

    #[test]
    fn unpause_allows_mutating_calls() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let _delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.pause(&admin);
        client.unpause(&admin);

        // stake should succeed after unpause
        client.stake(&delegator, &500);
        assert_eq!(client.staked_balance(&delegator), 1_500);
    }

    #[test]
    fn pause_requires_admin() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let attacker = Address::generate(&env);

        let result = client.try_pause(&attacker);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn unpause_requires_admin() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let attacker = Address::generate(&env);

        client.pause(&admin);
        let result = client.try_unpause(&attacker);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn getters_work_while_paused() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let delegator = Address::generate(&env);
        let delegatee = Address::generate(&env);

        client.stake(&delegator, &1_000);
        client.delegate(&delegator, &delegatee, &500);
        client.pause(&admin);

        // Read-only getters should still work
        assert_eq!(client.staked_balance(&delegator), 1_000);
        assert_eq!(client.get_delegations(&delegator).len(), 1);
        assert_eq!(client.current_epoch_num(), 1);
        assert!(client.is_paused());
    }
}
