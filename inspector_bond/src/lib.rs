#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, String, Symbol,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    MinBondAmount,
    SlashPenaltyBps,
    UnstakeLockDays,
    Bond(Address),
    /// Reentrancy lock for cross-contract call protection
    Reentrancy,
    /// Reentrancy lock for cross-contract call protection
    Reentrancy,

    // ── Two-Phase Inspector Slash (Issue #1082) ──────────────────────────────
    /// Challenge window for inspector slashes in seconds.
    ChallengeWindow,
    /// Monotonic slash-proposal counter.
    NextSlashId,
    /// Pending slash keyed by proposal ID.
    PendingInspectorSlash(u64),
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    Paused = 3,
    InvalidAmount = 4,
    BondTooLow = 5,
    LockNotExpired = 6,
    BondBelowMinimum = 7,
    NoBond = 8,
    /// Slash proposal not found
    SlashNotFound = 9,
    /// Challenge window has not yet elapsed
    ChallengeWindowNotElapsed = 10,
    /// Slash is already finalized or cancelled
    SlashAlreadyResolved = 11,
    /// Reentrancy detected — nested call rejected
    ReentrancyDetected = 12,
}

// ── Data Structures ───────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BondRecord {
    pub inspector: Address,
    pub amount: i128,
    pub locked_until: u64,
    pub slash_count: u32,
}

/// Status of a two-phase inspector slash proposal.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum InspectorSlashStatus {
    Pending = 0,
    Finalized = 1,
    Cancelled = 2,
}

/// A pending inspector slash proposal created by `propose_inspector_slash`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInspectorSlash {
    pub id: u64,
    pub inspector: Address,
    /// Pre-computed penalty amount (bps applied at proposal time).
    pub penalty_amount: i128,
    /// Ledger timestamp after which `finalize_inspector_slash` is allowed.
    pub deadline: u64,
    pub status: InspectorSlashStatus,
    /// Unique report identifier supplied by the caller.
    pub report_id: BytesN<32>,
    /// Human-readable reason string.
    pub reason: String,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct InspectorBondContract;

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("not init")
}

fn is_paused_internal(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), ContractError> {
    if is_paused_internal(env) {
        Err(ContractError::Paused)
    } else {
        Ok(())
    }
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
    caller.require_auth();
    if caller != &get_admin(env) {
        return Err(ContractError::NotAuthorized);
    }
    Ok(())
}

/// Reentrancy guard helpers
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

/// Scope guard that ensures reentrancy lock is released on drop
struct ReentrancyGuard<'a> {
    env: &'a Env,
}

impl<'a> ReentrancyGuard<'a> {
    fn new(env: &'a Env) -> Result<Self, ContractError> {
        enter_nonreentrant(env)?;
        Ok(ReentrancyGuard { env })
    }
}

impl<'a> Drop for ReentrancyGuard<'a> {
    fn drop(&mut self) {
        exit_nonreentrant(self.env);
    }
}

fn min_bond(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get::<_, i128>(&DataKey::MinBondAmount)
        .unwrap_or(1_000_0000000)
}

fn slash_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get::<_, i128>(&DataKey::SlashPenaltyBps)
        .unwrap_or(1000)
}

fn lock_days(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::UnstakeLockDays)
        .unwrap_or(30)
}

#[contractimpl]
impl InspectorBondContract {
    pub fn init(
        env: Env,
        admin: Address,
        min_bond_amount: i128,
        slash_penalty_bps: i128,
        unstake_lock_days: u64,
    ) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MinBondAmount, &min_bond_amount);
        env.storage()
            .instance()
            .set(&DataKey::SlashPenaltyBps, &slash_penalty_bps);
        env.storage()
            .instance()
            .set(&DataKey::UnstakeLockDays, &unstake_lock_days);
        Ok(())
    }

    /// Inspector stakes a bond. Validates amount >= MIN_BOND_AMOUNT.
    pub fn stake_bond(env: Env, inspector: Address, amount: i128) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        inspector.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if amount < min_bond(&env) {
            return Err(ContractError::BondTooLow);
        }

        let lock_secs = lock_days(&env) * 86_400;
        let locked_until = env.ledger().timestamp() + lock_secs;

        let existing: Option<BondRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::Bond(inspector.clone()));
        let bond = BondRecord {
            inspector: inspector.clone(),
            amount: existing.as_ref().map(|b| b.amount).unwrap_or(0) + amount,
            locked_until,
            slash_count: existing.as_ref().map(|b| b.slash_count).unwrap_or(0),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Bond(inspector.clone()), &bond);

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "staked"),
                inspector,
            ),
            amount,
        );
        Ok(())
    }

    /// Inspector withdraws bond if no active jobs and locked_until has passed.
    pub fn unstake_bond(env: Env, inspector: Address) -> Result<i128, ContractError> {
        inspector.require_auth();
        let bond: BondRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Bond(inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        if bond.amount < min_bond(&env) {
            return Err(ContractError::BondBelowMinimum);
        }
        if env.ledger().timestamp() < bond.locked_until {
            return Err(ContractError::LockNotExpired);
        }

        let amount = bond.amount;
        env.storage()
            .persistent()
            .remove(&DataKey::Bond(inspector.clone()));

        // Reentrancy guard before external operations - uses scope guard for automatic release
        let _guard = ReentrancyGuard::new(&env)?;

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "unstaked"),
                inspector,
            ),
            amount,
        );

        Ok(amount)
    }

    /// Admin slashes a percentage of the bond.
    pub fn slash_inspector(
        env: Env,
        admin: Address,
        inspector: Address,
        report_id: BytesN<32>,
        reason: Symbol,
    ) -> Result<i128, ContractError> {
        require_admin(&env, &admin)?;

        let mut bond: BondRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Bond(inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        let penalty = bond.amount * slash_bps(&env) / 10_000;
        let slash_amount = if penalty > bond.amount {
            bond.amount
        } else {
            penalty
        };

        bond.amount = (bond.amount - slash_amount).max(0);
        bond.slash_count += 1;
        env.storage()
            .persistent()
            .set(&DataKey::Bond(inspector.clone()), &bond);

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "slashed"),
                inspector.clone(),
            ),
            (slash_amount, report_id, reason),
        );
        Ok(slash_amount)
    }

    pub fn get_bond(env: Env, inspector: Address) -> Option<BondRecord> {
        env.storage().persistent().get(&DataKey::Bond(inspector))
    }

    pub fn is_bonded(env: Env, inspector: Address) -> bool {
        match env
            .storage()
            .persistent()
            .get::<_, BondRecord>(&DataKey::Bond(inspector))
        {
            Some(b) => b.amount >= min_bond(&env),
            None => false,
        }
    }

    pub fn pause(env: Env, admin: Address) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env, admin: Address) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused_internal(&env)
    }

    pub fn set_min_bond(env: Env, admin: Address, amount: i128) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::MinBondAmount, &amount);
        Ok(())
    }

    // ── Two-Phase Inspector Slash (Issue #1082) ───────────────────────────────

    /// Read the current challenge window (seconds). Default: 7 days.
    pub fn inspector_challenge_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::ChallengeWindow)
            .unwrap_or(604_800)
    }

    /// Admin sets the challenge window duration (seconds).
    pub fn set_inspector_challenge_window(
        env: Env,
        admin: Address,
        window_secs: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::ChallengeWindow, &window_secs);
        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "challenge_window_updated"),
            ),
            window_secs,
        );
        Ok(())
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

    /// Propose a two-phase inspector slash.
    ///
    /// Records the penalty amount (computed from the current bond and configured
    /// `slash_penalty_bps`) and sets a deadline equal to `now + challenge_window`.
    /// The bond is NOT reduced yet.  Call `finalize_inspector_slash` after the
    /// deadline, or `cancel_inspector_slash` to dismiss.
    ///
    /// Only the admin (arbiter) may propose.
    pub fn propose_inspector_slash(
        env: Env,
        admin: Address,
        inspector: Address,
        report_id: BytesN<32>,
        reason: String,
    ) -> Result<u64, ContractError> {
        require_admin(&env, &admin)?;

        let bond: BondRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Bond(inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        let penalty = bond.amount * slash_bps(&env) / 10_000;
        let penalty_amount = if penalty > bond.amount {
            bond.amount
        } else {
            penalty
        };

        let slash_id = Self::next_slash_id(&env);
        let deadline = env.ledger().timestamp() + Self::inspector_challenge_window(env.clone());

        let pending = PendingInspectorSlash {
            id: slash_id,
            inspector: inspector.clone(),
            penalty_amount,
            deadline,
            status: InspectorSlashStatus::Pending,
            report_id: report_id.clone(),
            reason: reason.clone(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::PendingInspectorSlash(slash_id), &pending);

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "slash_proposed"),
                inspector,
            ),
            (slash_id, penalty_amount, deadline),
        );

        Ok(slash_id)
    }

    /// Finalize a pending slash once the challenge window has elapsed.
    ///
    /// Applies the pre-computed penalty to the inspector's bond record.
    /// Only the admin may finalize.
    pub fn finalize_inspector_slash(
        env: Env,
        admin: Address,
        slash_id: u64,
    ) -> Result<i128, ContractError> {
        require_admin(&env, &admin)?;

        let mut pending: PendingInspectorSlash = env
            .storage()
            .persistent()
            .get(&DataKey::PendingInspectorSlash(slash_id))
            .ok_or(ContractError::SlashNotFound)?;

        if pending.status != InspectorSlashStatus::Pending {
            return Err(ContractError::SlashAlreadyResolved);
        }

        if env.ledger().timestamp() < pending.deadline {
            return Err(ContractError::ChallengeWindowNotElapsed);
        }

        // Apply bond reduction
        let mut bond: BondRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Bond(pending.inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        let actual_slash = pending.penalty_amount.min(bond.amount);
        bond.amount = (bond.amount - actual_slash).max(0);
        bond.slash_count += 1;
        env.storage()
            .persistent()
            .set(&DataKey::Bond(pending.inspector.clone()), &bond);

        // Mark as finalized
        pending.status = InspectorSlashStatus::Finalized;
        env.storage()
            .persistent()
            .set(&DataKey::PendingInspectorSlash(slash_id), &pending);

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "slash_finalized"),
                pending.inspector.clone(),
            ),
            (slash_id, actual_slash, pending.report_id, pending.reason),
        );

        Ok(actual_slash)
    }

    /// Cancel a pending slash during the challenge window.
    ///
    /// Only the admin (arbiter) may cancel.  The bond is left unchanged.
    pub fn cancel_inspector_slash(
        env: Env,
        admin: Address,
        slash_id: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;

        let mut pending: PendingInspectorSlash = env
            .storage()
            .persistent()
            .get(&DataKey::PendingInspectorSlash(slash_id))
            .ok_or(ContractError::SlashNotFound)?;

        if pending.status != InspectorSlashStatus::Pending {
            return Err(ContractError::SlashAlreadyResolved);
        }

        pending.status = InspectorSlashStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::PendingInspectorSlash(slash_id), &pending);

        env.events().publish(
            (
                Symbol::new(&env, "inspector_bond"),
                Symbol::new(&env, "slash_cancelled"),
                pending.inspector,
            ),
            slash_id,
        );

        Ok(())
    }

    /// Query a pending slash proposal by ID.
    pub fn get_pending_inspector_slash(env: Env, slash_id: u64) -> Option<PendingInspectorSlash> {
        env.storage()
            .persistent()
            .get(&DataKey::PendingInspectorSlash(slash_id))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{testutils::Address as _, testutils::Ledger, Env};

    fn setup(env: &Env) -> (Address, InspectorBondContractClient<'_>) {
        env.mock_all_auths();
        let id = env.register(InspectorBondContract, ());
        let client = InspectorBondContractClient::new(env, &id);
        let admin = Address::generate(env);
        // min_bond=1000, slash_bps=1000 (10%), lock_days=0 for easy testing
        client.init(&admin, &1_000, &1_000, &0);
        (admin, client)
    }

    #[test]
    fn stake_and_is_bonded() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        assert!(!client.is_bonded(&inspector));
        client.stake_bond(&inspector, &1_000);
        assert!(client.is_bonded(&inspector));

        let bond = client.get_bond(&inspector).unwrap();
        assert_eq!(bond.amount, 1_000);
        assert_eq!(bond.slash_count, 0);
    }

    #[test]
    fn stake_below_minimum_fails() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        let result = client.try_stake_bond(&inspector, &500);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::BondTooLow);
    }

    #[test]
    fn unstake_succeeds_when_lock_expired() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &1_000);
        // lock_days=0 so locked_until = now + 0 = now, already expired
        let amount = client.unstake_bond(&inspector);
        assert_eq!(amount, 1_000);
        assert!(client.get_bond(&inspector).is_none());
    }

    #[test]
    fn unstake_fails_when_lock_not_expired() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(InspectorBondContract, ());
        let client = InspectorBondContractClient::new(&env, &id);
        let admin = Address::generate(&env);
        // lock_days=30
        client.init(&admin, &1_000, &1_000, &30);

        let inspector = Address::generate(&env);
        client.stake_bond(&inspector, &1_000);

        let result = client.try_unstake_bond(&inspector);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::LockNotExpired);
    }

    #[test]
    fn slash_reduces_bond_by_penalty_bps() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &10_000);
        let report_id = BytesN::from_array(&env, &[1u8; 32]);
        let slashed =
            client.slash_inspector(&admin, &inspector, &report_id, &Symbol::new(&env, "fraud"));

        // 10% of 10_000 = 1_000
        assert_eq!(slashed, 1_000);
        let bond = client.get_bond(&inspector).unwrap();
        assert_eq!(bond.amount, 9_000);
        assert_eq!(bond.slash_count, 1);
    }

    #[test]
    fn slash_floors_at_zero() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &1_000);

        let report_id = BytesN::from_array(&env, &[2u8; 32]);
        for _ in 0..20 {
            client.slash_inspector(&admin, &inspector, &report_id, &Symbol::new(&env, "fraud"));
        }

        let bond = client.get_bond(&inspector).unwrap();
        assert!(bond.amount >= 0);
    }

    #[test]
    fn unstake_fails_when_bond_below_minimum_after_slash() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        // Stake exactly minimum
        client.stake_bond(&inspector, &1_000);
        // Slash reduces it below minimum (900 < 1000)
        let report_id = BytesN::from_array(&env, &[3u8; 32]);
        client.slash_inspector(&admin, &inspector, &report_id, &Symbol::new(&env, "fraud"));

        let result = client.try_unstake_bond(&inspector);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::BondBelowMinimum
        );
    }

    #[test]
    fn pause_blocks_stake_but_not_unstake() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        // Stake before pause
        client.stake_bond(&inspector, &1_000);
        client.pause(&admin);

        // stake_bond should fail
        let result = client.try_stake_bond(&inspector, &1_000);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

        // unstake_bond should still work (not paused-gated)
        let amount = client.unstake_bond(&inspector);
        assert_eq!(amount, 1_000);
    }

    #[test]
    fn non_admin_cannot_slash() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);
        let attacker = Address::generate(&env);

        client.stake_bond(&inspector, &1_000);
        let report_id = BytesN::from_array(&env, &[4u8; 32]);
        let result = client.try_slash_inspector(
            &attacker,
            &inspector,
            &report_id,
            &Symbol::new(&env, "fraud"),
        );
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    // ── Reentrancy guard tests ─────────────────────────────────────────────────────

    #[test]
    fn unstake_succeeds_and_releases_guard() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &1_000);
        let amount = client.unstake_bond(&inspector);
        assert_eq!(amount, 1_000);

        // Verify guard is released by trying again (should return NoBond, not ReentrancyDetected)
        let result = client.try_unstake_bond(&inspector);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NoBond);
    }

    #[test]
    fn unstake_releases_guard_on_lock_not_expired_error() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(InspectorBondContract, ());
        let client = InspectorBondContractClient::new(&env, &id);
        let admin = Address::generate(&env);
        // lock_days=30
        client.init(&admin, &1_000, &1_000, &30);

        let inspector = Address::generate(&env);
        client.stake_bond(&inspector, &1_000);

        // Try to unstake before lock expires (should fail)
        let result = client.try_unstake_bond(&inspector);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::LockNotExpired);

        // After error, guard should be released - advance time and try again
        env.ledger().with_mut(|li| li.timestamp += 30 * 86_400);
        let amount = client.unstake_bond(&inspector);
        assert_eq!(amount, 1_000);
    }

    #[test]
    fn unstake_releases_guard_on_no_bond_error() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        // Try to unstake without a bond (should fail)
        let result = client.try_unstake_bond(&inspector);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NoBond);

        // After error, guard should be released - stake and unstake should work
        client.stake_bond(&inspector, &1_000);
        let amount = client.unstake_bond(&inspector);
        assert_eq!(amount, 1_000);
    }

    // ── Two-Phase Inspector Slash (Issue #1082) ───────────────────────────────

    #[test]
    fn two_phase_inspector_slash_happy_path() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &10_000);

        let report_id = BytesN::from_array(&env, &[10u8; 32]);
        let reason = soroban_sdk::String::from_str(&env, "misconduct");
        let slash_id = client.propose_inspector_slash(&admin, &inspector, &report_id, &reason);

        // Bond must NOT be reduced yet
        let bond = client.get_bond(&inspector).unwrap();
        assert_eq!(bond.amount, 10_000);
        assert_eq!(bond.slash_count, 0);

        // Status is Pending
        let pending = client.get_pending_inspector_slash(&slash_id).unwrap();
        assert!(matches!(pending.status, InspectorSlashStatus::Pending));

        // Advance past the 7-day challenge window
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);

        let slashed = client.finalize_inspector_slash(&admin, &slash_id);
        assert_eq!(slashed, 1_000); // 10 % of 10_000

        let bond = client.get_bond(&inspector).unwrap();
        assert_eq!(bond.amount, 9_000);
        assert_eq!(bond.slash_count, 1);

        let pending = client.get_pending_inspector_slash(&slash_id).unwrap();
        assert!(matches!(pending.status, InspectorSlashStatus::Finalized));
    }

    #[test]
    fn two_phase_inspector_slash_cancel_preserves_bond() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &10_000);

        let report_id = BytesN::from_array(&env, &[11u8; 32]);
        let reason = soroban_sdk::String::from_str(&env, "disputed");
        let slash_id = client.propose_inspector_slash(&admin, &inspector, &report_id, &reason);

        // Cancel during the challenge window
        client.cancel_inspector_slash(&admin, &slash_id);

        // Bond unchanged
        let bond = client.get_bond(&inspector).unwrap();
        assert_eq!(bond.amount, 10_000);

        // Status is Cancelled
        let pending = client.get_pending_inspector_slash(&slash_id).unwrap();
        assert!(matches!(pending.status, InspectorSlashStatus::Cancelled));

        // Trying to finalize a cancelled slash must fail
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);
        let result = client.try_finalize_inspector_slash(&admin, &slash_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::SlashAlreadyResolved
        );
    }

    #[test]
    fn two_phase_inspector_slash_finalize_before_window_fails() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        client.stake_bond(&inspector, &10_000);

        let report_id = BytesN::from_array(&env, &[12u8; 32]);
        let reason = soroban_sdk::String::from_str(&env, "too soon");
        let slash_id = client.propose_inspector_slash(&admin, &inspector, &report_id, &reason);

        // Finalize immediately (before deadline) must be rejected
        let result = client.try_finalize_inspector_slash(&admin, &slash_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::ChallengeWindowNotElapsed
        );
    }

    #[test]
    fn two_phase_inspector_slash_non_admin_rejected() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);
        let attacker = Address::generate(&env);

        client.stake_bond(&inspector, &10_000);

        let report_id = BytesN::from_array(&env, &[13u8; 32]);
        let reason = soroban_sdk::String::from_str(&env, "fraud");

        // Non-admin cannot propose
        let result = client.try_propose_inspector_slash(&attacker, &inspector, &report_id, &reason);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);

        // Admin proposes, then attacker tries to finalize/cancel
        let slash_id = client.propose_inspector_slash(&admin, &inspector, &report_id, &reason);

        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 604_801);

        let result = client.try_finalize_inspector_slash(&attacker, &slash_id);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);

        let result = client.try_cancel_inspector_slash(&attacker, &slash_id);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn two_phase_inspector_slash_configurable_window() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let inspector = Address::generate(&env);

        // Default window is 7 days
        assert_eq!(client.inspector_challenge_window(), 604_800);

        // Override to 1 day
        client.set_inspector_challenge_window(&admin, &86_400);
        assert_eq!(client.inspector_challenge_window(), 86_400);

        client.stake_bond(&inspector, &10_000);
        let report_id = BytesN::from_array(&env, &[14u8; 32]);
        let reason = soroban_sdk::String::from_str(&env, "short window");
        let slash_id = client.propose_inspector_slash(&admin, &inspector, &report_id, &reason);

        // 20 hours not enough
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 72_000);
        let result = client.try_finalize_inspector_slash(&admin, &slash_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::ChallengeWindowNotElapsed
        );

        // 25 hours → past 1-day window
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 18_001); // 72_000 + 18_001 = 90_001 > 86_400
        let slashed = client.finalize_inspector_slash(&admin, &slash_id);
        assert_eq!(slashed, 1_000);
    }
}
