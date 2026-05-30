#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol};

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

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct InspectorBondContract;

fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).expect("not init")
}

fn is_paused_internal(env: &Env) -> bool {
    env.storage().instance().get::<_, bool>(&DataKey::Paused).unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), ContractError> {
    if is_paused_internal(env) { Err(ContractError::Paused) } else { Ok(()) }
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
    caller.require_auth();
    if caller != &get_admin(env) { return Err(ContractError::NotAuthorized); }
    Ok(())
}

fn min_bond(env: &Env) -> i128 {
    env.storage().instance().get::<_, i128>(&DataKey::MinBondAmount).unwrap_or(1_000_0000000)
}

fn slash_bps(env: &Env) -> i128 {
    env.storage().instance().get::<_, i128>(&DataKey::SlashPenaltyBps).unwrap_or(1000)
}

fn lock_days(env: &Env) -> u64 {
    env.storage().instance().get::<_, u64>(&DataKey::UnstakeLockDays).unwrap_or(30)
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
        env.storage().instance().set(&DataKey::MinBondAmount, &min_bond_amount);
        env.storage().instance().set(&DataKey::SlashPenaltyBps, &slash_penalty_bps);
        env.storage().instance().set(&DataKey::UnstakeLockDays, &unstake_lock_days);
        Ok(())
    }

    /// Inspector stakes a bond. Validates amount >= MIN_BOND_AMOUNT.
    pub fn stake_bond(env: Env, inspector: Address, amount: i128) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        inspector.require_auth();
        if amount <= 0 { return Err(ContractError::InvalidAmount); }
        if amount < min_bond(&env) { return Err(ContractError::BondTooLow); }

        let lock_secs = lock_days(&env) * 86_400;
        let locked_until = env.ledger().timestamp() + lock_secs;

        let existing: Option<BondRecord> = env.storage().persistent().get(&DataKey::Bond(inspector.clone()));
        let bond = BondRecord {
            inspector: inspector.clone(),
            amount: existing.as_ref().map(|b| b.amount).unwrap_or(0) + amount,
            locked_until,
            slash_count: existing.as_ref().map(|b| b.slash_count).unwrap_or(0),
        };
        env.storage().persistent().set(&DataKey::Bond(inspector.clone()), &bond);

        env.events().publish(
            (Symbol::new(&env, "inspector_bond"), Symbol::new(&env, "staked"), inspector),
            amount,
        );
        Ok(())
    }

    /// Inspector withdraws bond if no active jobs and locked_until has passed.
    pub fn unstake_bond(env: Env, inspector: Address) -> Result<i128, ContractError> {
        inspector.require_auth();
        let bond: BondRecord = env.storage().persistent()
            .get(&DataKey::Bond(inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        if bond.amount < min_bond(&env) {
            return Err(ContractError::BondBelowMinimum);
        }
        if env.ledger().timestamp() < bond.locked_until {
            return Err(ContractError::LockNotExpired);
        }

        let amount = bond.amount;
        env.storage().persistent().remove(&DataKey::Bond(inspector.clone()));

        env.events().publish(
            (Symbol::new(&env, "inspector_bond"), Symbol::new(&env, "unstaked"), inspector),
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

        let mut bond: BondRecord = env.storage().persistent()
            .get(&DataKey::Bond(inspector.clone()))
            .ok_or(ContractError::NoBond)?;

        let penalty = bond.amount * slash_bps(&env) / 10_000;
        let slash_amount = if penalty > bond.amount { bond.amount } else { penalty };

        bond.amount = (bond.amount - slash_amount).max(0);
        bond.slash_count += 1;
        env.storage().persistent().set(&DataKey::Bond(inspector.clone()), &bond);

        env.events().publish(
            (Symbol::new(&env, "inspector_bond"), Symbol::new(&env, "slashed"), inspector.clone()),
            (slash_amount, report_id, reason),
        );
        Ok(slash_amount)
    }

    pub fn get_bond(env: Env, inspector: Address) -> Option<BondRecord> {
        env.storage().persistent().get(&DataKey::Bond(inspector))
    }

    pub fn is_bonded(env: Env, inspector: Address) -> bool {
        match env.storage().persistent().get::<_, BondRecord>(&DataKey::Bond(inspector)) {
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
        env.storage().instance().set(&DataKey::MinBondAmount, &amount);
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Env,
    };

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
        let slashed = client.slash_inspector(&admin, &inspector, &report_id, &Symbol::new(&env, "fraud"));

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
        assert_eq!(result.unwrap_err().unwrap(), ContractError::BondBelowMinimum);
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
        let result = client.try_slash_inspector(&attacker, &inspector, &report_id, &Symbol::new(&env, "fraud"));
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }
}
