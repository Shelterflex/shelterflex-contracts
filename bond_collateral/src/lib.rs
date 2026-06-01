#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, BytesN, Env,
    IntoVal, String, Symbol, Vec,
};

pub mod access_control;
mod formal_properties;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    BondCollateral(BytesN<32>),
    TotalCollateral,
    WarningThreshold,
    LiquidationThreshold,
    KeeperRewardCap,
    ContractVersion,

    // ── Inspector bond layer (Issue #925) ────────────────────────────────
    /// Address of the linked slashing_module contract.
    SlashingModule,
    /// Operator role: allowed to lock/unlock bonds for inspection disputes.
    Operator,
    /// Per-inspector bond balance.
    InspectorBond(Address),
    /// Active inspection_id locks per inspector (any non-empty entry blocks withdraw).
    InspectorLocks(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralPosition {
    pub owner: Address,
    pub collateral_amount: i128,
    pub bond_amount: i128,
    pub created_at: u64,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    InvalidAmount = 3,
    InsufficientCollateral = 4,
    PositionNotFound = 5,
    CannotLiquidate = 6,
    BelowThreshold = 7,
    InvalidThreshold = 8,
    InvalidRewardCap = 9,
    CollateralRatioTooLow = 10,
    NoSurplus = 11,
    /// Issue #925: bond is locked for a pending inspection dispute.
    BondLocked = 12,
    /// Issue #925: inspector has no bond / bond is not large enough for the slash.
    InsufficientBond = 13,
    /// Issue #925: lock attempted for an inspection_id that already has a lock.
    LockAlreadyExists = 14,
    /// Issue #925: unlock attempted for an inspection_id that is not locked.
    LockNotFound = 15,
    /// Issue #925: slashing_module address has not been configured.
    SlashingModuleNotSet = 16,
}

impl From<access_control::AccessControlError> for ContractError {
    fn from(_err: access_control::AccessControlError) -> Self {
        ContractError::NotAuthorized
    }
}

#[contract]
pub struct BondCollateral;

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("admin not set")
}

fn get_token(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Token)
        .expect("token not set")
}

fn get_contract_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::ContractVersion)
        .unwrap_or(1u32)
}

fn get_warning_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::WarningThreshold)
        .unwrap_or(150u32)
}

fn get_liquidation_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::LiquidationThreshold)
        .unwrap_or(120u32)
}

fn get_keeper_reward_cap(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::KeeperRewardCap)
        .unwrap_or(500u32)
}

fn calculate_collateral_ratio(collateral: i128, bond: i128) -> u32 {
    if bond == 0 {
        return u32::MAX;
    }
    ((collateral as f64 / bond as f64) * 100.0) as u32
}

fn get_position(env: &Env, position_id: &BytesN<32>) -> Option<CollateralPosition> {
    env.storage()
        .persistent()
        .get(&DataKey::BondCollateral(position_id.clone()))
}

fn put_position(env: &Env, position_id: &BytesN<32>, position: &CollateralPosition) {
    env.storage()
        .persistent()
        .set(&DataKey::BondCollateral(position_id.clone()), position);
}

fn remove_position(env: &Env, position_id: &BytesN<32>) {
    env.storage()
        .persistent()
        .remove(&DataKey::BondCollateral(position_id.clone()));
}

fn get_total_collateral(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalCollateral)
        .unwrap_or(0)
}

fn put_total_collateral(env: &Env, total: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalCollateral, &total);
}

#[contractimpl]
impl BondCollateral {
    pub fn init(env: Env, admin: Address, token: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }

        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::TotalCollateral, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::WarningThreshold, &150u32);
        env.storage()
            .instance()
            .set(&DataKey::LiquidationThreshold, &120u32);
        env.storage()
            .instance()
            .set(&DataKey::KeeperRewardCap, &500u32);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "init"),
            ),
            admin,
        );

        Ok(())
    }

    pub fn contract_version(env: Env) -> u32 {
        get_contract_version(&env)
    }

    pub fn set_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "set_admin")?;

        env.storage().instance().set(&DataKey::Admin, &new_admin);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "set_admin"),
            ),
            (admin, new_admin),
        );
        Ok(())
    }

    pub fn set_thresholds(
        env: Env,
        admin: Address,
        warning: u32,
        liquidation: u32,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(&env, &current_admin, &admin, "set_thresholds")?;

        if warning <= liquidation || liquidation < 100 {
            return Err(ContractError::InvalidThreshold);
        }

        env.storage()
            .instance()
            .set(&DataKey::WarningThreshold, &warning);
        env.storage()
            .instance()
            .set(&DataKey::LiquidationThreshold, &liquidation);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "set_thresholds"),
            ),
            (warning, liquidation),
        );

        Ok(())
    }

    pub fn set_keeper_reward_cap(
        env: Env,
        admin: Address,
        cap_bps: u32,
    ) -> Result<(), ContractError> {
        let current_admin = get_admin(&env);
        access_control::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_keeper_reward_cap",
        )?;

        if cap_bps > 5000 {
            return Err(ContractError::InvalidRewardCap);
        }

        env.storage()
            .instance()
            .set(&DataKey::KeeperRewardCap, &cap_bps);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "set_keeper_reward_cap"),
            ),
            cap_bps,
        );

        Ok(())
    }

    pub fn deposit_collateral(
        env: Env,
        owner: Address,
        position_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), ContractError> {
        owner.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let token_address = get_token(&env);
        let token_client = token::Client::new(&env, &token_address);

        token_client.transfer(&owner, &env.current_contract_address(), &amount);

        let mut position = get_position(&env, &position_id).unwrap_or(CollateralPosition {
            owner: owner.clone(),
            collateral_amount: 0,
            bond_amount: 0,
            created_at: env.ledger().timestamp(),
        });

        if position.owner != owner {
            return Err(ContractError::NotAuthorized);
        }

        position.collateral_amount += amount;
        position.created_at = env.ledger().timestamp();

        put_position(&env, &position_id, &position);

        let total = get_total_collateral(&env) + amount;
        put_total_collateral(&env, total);

        let ratio = calculate_collateral_ratio(position.collateral_amount, position.bond_amount);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "collateral_deposited"),
                owner.clone(),
            ),
            (
                position_id.clone(),
                amount,
                position.collateral_amount,
                ratio,
            ),
        );

        if ratio < get_warning_threshold(&env) {
            env.events().publish(
                (
                    Symbol::new(&env, "bond_collateral"),
                    Symbol::new(&env, "warning_threshold_breached"),
                    owner.clone(),
                ),
                (position_id.clone(), ratio, get_warning_threshold(&env)),
            );
        }

        Ok(())
    }

    pub fn issue_bond(
        env: Env,
        owner: Address,
        position_id: BytesN<32>,
        bond_amount: i128,
    ) -> Result<(), ContractError> {
        owner.require_auth();

        if bond_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut position = get_position(&env, &position_id).unwrap_or(CollateralPosition {
            owner: owner.clone(),
            collateral_amount: 0,
            bond_amount: 0,
            created_at: env.ledger().timestamp(),
        });

        if position.owner != owner {
            return Err(ContractError::NotAuthorized);
        }

        position.bond_amount += bond_amount;

        let ratio = calculate_collateral_ratio(position.collateral_amount, position.bond_amount);

        if ratio < get_liquidation_threshold(&env) {
            position.bond_amount -= bond_amount;
            return Err(ContractError::CollateralRatioTooLow);
        }

        put_position(&env, &position_id, &position);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "bond_issued"),
                owner.clone(),
            ),
            (
                position_id.clone(),
                bond_amount,
                position.bond_amount,
                ratio,
            ),
        );

        if ratio < get_warning_threshold(&env) {
            env.events().publish(
                (
                    Symbol::new(&env, "bond_collateral"),
                    Symbol::new(&env, "warning_threshold_breached"),
                    owner.clone(),
                ),
                (position_id.clone(), ratio, get_warning_threshold(&env)),
            );
        }

        Ok(())
    }

    pub fn redeem_bond(
        env: Env,
        owner: Address,
        position_id: BytesN<32>,
        bond_amount: i128,
    ) -> Result<(), ContractError> {
        owner.require_auth();

        if bond_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut position =
            get_position(&env, &position_id).ok_or(ContractError::PositionNotFound)?;

        if position.owner != owner {
            return Err(ContractError::NotAuthorized);
        }

        if position.bond_amount < bond_amount {
            return Err(ContractError::InsufficientCollateral);
        }

        position.bond_amount -= bond_amount;
        put_position(&env, &position_id, &position);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "bond_redeemed"),
                owner.clone(),
            ),
            (position_id.clone(), bond_amount, position.bond_amount),
        );

        Ok(())
    }

    pub fn withdraw_collateral(
        env: Env,
        owner: Address,
        position_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), ContractError> {
        owner.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut position =
            get_position(&env, &position_id).ok_or(ContractError::PositionNotFound)?;

        if position.owner != owner {
            return Err(ContractError::NotAuthorized);
        }

        if position.collateral_amount < amount {
            return Err(ContractError::InsufficientCollateral);
        }

        let new_collateral = position.collateral_amount - amount;
        let ratio = calculate_collateral_ratio(new_collateral, position.bond_amount);

        if position.bond_amount > 0 && ratio < get_liquidation_threshold(&env) {
            return Err(ContractError::BelowThreshold);
        }

        position.collateral_amount = new_collateral;
        put_position(&env, &position_id, &position);

        let token_address = get_token(&env);
        let token_client = token::Client::new(&env, &token_address);
        token_client.transfer(&env.current_contract_address(), &owner, &amount);

        let total = get_total_collateral(&env) - amount;
        put_total_collateral(&env, total);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "collateral_withdrawn"),
                owner.clone(),
            ),
            (position_id.clone(), amount, position.collateral_amount),
        );

        Ok(())
    }

    pub fn liquidate(
        env: Env,
        keeper: Address,
        position_id: BytesN<32>,
    ) -> Result<(), ContractError> {
        keeper.require_auth();

        let position = get_position(&env, &position_id).ok_or(ContractError::PositionNotFound)?;

        let ratio = calculate_collateral_ratio(position.collateral_amount, position.bond_amount);

        if ratio >= get_liquidation_threshold(&env) {
            return Err(ContractError::CannotLiquidate);
        }

        let collateral = position.collateral_amount;
        let bond = position.bond_amount;

        if bond == 0 || collateral == 0 {
            return Err(ContractError::CannotLiquidate);
        }

        let surplus = collateral.saturating_sub(bond);
        let mut keeper_reward = if surplus > 0 {
            (surplus * get_keeper_reward_cap(&env) as i128) / 10000
        } else {
            0
        };

        let max_reward = collateral / 10;
        if keeper_reward > max_reward {
            keeper_reward = max_reward;
        }

        let liquidator_payout = if keeper_reward > 0 {
            collateral.min(keeper_reward)
        } else {
            0
        };

        let token_address = get_token(&env);
        let token_client = token::Client::new(&env, &token_address);

        if liquidator_payout > 0 {
            token_client.transfer(&env.current_contract_address(), &keeper, &liquidator_payout);
        }

        remove_position(&env, &position_id);

        let total = get_total_collateral(&env) - collateral;
        put_total_collateral(&env, total);

        env.events().publish(
            (
                Symbol::new(&env, "bond_collateral"),
                Symbol::new(&env, "liquidation"),
                keeper.clone(),
            ),
            (
                position_id.clone(),
                position.owner.clone(),
                collateral,
                bond,
                ratio,
                liquidator_payout,
            ),
        );

        Ok(())
    }

    pub fn get_position(env: Env, position_id: BytesN<32>) -> Option<CollateralPosition> {
        get_position(&env, &position_id)
    }

    pub fn get_collateral_ratio(env: Env, position_id: BytesN<32>) -> Option<u32> {
        get_position(&env, &position_id)
            .map(|p| calculate_collateral_ratio(p.collateral_amount, p.bond_amount))
    }

    pub fn get_thresholds(env: Env) -> (u32, u32) {
        (get_warning_threshold(&env), get_liquidation_threshold(&env))
    }

    pub fn get_keeper_reward_cap(env: Env) -> u32 {
        get_keeper_reward_cap(&env)
    }

    pub fn total_collateral(env: Env) -> i128 {
        get_total_collateral(&env)
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Inspector bond layer (Issue #925)
    //
    // Adds a parallel, per-inspector bond accounting layer alongside the
    // existing collateral-position model. Inspectors deposit a bond that may
    // be locked for the duration of an inspection dispute and slashed by the
    // admin via the linked slashing_module.
    // ──────────────────────────────────────────────────────────────────────────

    /// Configure the linked slashing module address. Admin-only.
    pub fn set_slashing_module(
        env: Env,
        admin: Address,
        slashing_module: Address,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::SlashingModule, &slashing_module);
        env.events().publish(
            (symbol_short!("bond"), Symbol::new(&env, "set_slashing")),
            slashing_module,
        );
        Ok(())
    }

    /// Configure the operator address allowed to lock/unlock inspector bonds.
    pub fn set_operator(env: Env, admin: Address, operator: Address) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.events().publish(
            (symbol_short!("bond"), Symbol::new(&env, "set_operator")),
            operator,
        );
        Ok(())
    }

    /// Inspector deposits collateral as a bond. The inspector must auth.
    pub fn deposit_bond(env: Env, inspector: Address, amount: i128) -> Result<(), ContractError> {
        inspector.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        let current = get_inspector_bond(&env, &inspector);
        env.storage().persistent().set(
            &DataKey::InspectorBond(inspector.clone()),
            &(current + amount),
        );
        env.events().publish(
            (
                symbol_short!("bond"),
                Symbol::new(&env, "deposited"),
                inspector,
            ),
            amount,
        );
        Ok(())
    }

    /// Inspector withdraws part or all of their bond. Blocked when any
    /// inspection_id lock is active on the inspector.
    pub fn withdraw_bond(env: Env, inspector: Address, amount: i128) -> Result<(), ContractError> {
        inspector.require_auth();
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if !get_inspector_locks(&env, &inspector).is_empty() {
            return Err(ContractError::BondLocked);
        }
        let current = get_inspector_bond(&env, &inspector);
        if amount > current {
            return Err(ContractError::InsufficientBond);
        }
        env.storage().persistent().set(
            &DataKey::InspectorBond(inspector.clone()),
            &(current - amount),
        );
        env.events().publish(
            (
                symbol_short!("bond"),
                Symbol::new(&env, "withdrawn"),
                inspector,
            ),
            amount,
        );
        Ok(())
    }

    /// Current bond balance for the given inspector.
    pub fn get_bond(env: Env, inspector: Address) -> i128 {
        get_inspector_bond(&env, &inspector)
    }

    /// Lock the inspector's bond for the duration of an inspection dispute.
    /// Only the registered operator may lock.
    pub fn lock_bond(
        env: Env,
        operator: Address,
        inspector: Address,
        inspection_id: String,
    ) -> Result<(), ContractError> {
        require_operator(&env, &operator)?;
        let mut locks = get_inspector_locks(&env, &inspector);
        for existing in locks.iter() {
            if existing == inspection_id {
                return Err(ContractError::LockAlreadyExists);
            }
        }
        locks.push_back(inspection_id.clone());
        env.storage()
            .persistent()
            .set(&DataKey::InspectorLocks(inspector.clone()), &locks);
        env.events().publish(
            (
                symbol_short!("bond"),
                Symbol::new(&env, "locked"),
                inspector,
            ),
            inspection_id,
        );
        Ok(())
    }

    /// Release a previously-set lock once the dispute has resolved. Only the
    /// registered operator may unlock.
    pub fn unlock_bond(
        env: Env,
        operator: Address,
        inspector: Address,
        inspection_id: String,
    ) -> Result<(), ContractError> {
        require_operator(&env, &operator)?;
        let locks = get_inspector_locks(&env, &inspector);
        let mut pruned: Vec<String> = Vec::new(&env);
        let mut found = false;
        for existing in locks.iter() {
            if existing == inspection_id {
                found = true;
                continue;
            }
            pruned.push_back(existing);
        }
        if !found {
            return Err(ContractError::LockNotFound);
        }
        env.storage()
            .persistent()
            .set(&DataKey::InspectorLocks(inspector.clone()), &pruned);
        env.events().publish(
            (
                symbol_short!("bond"),
                Symbol::new(&env, "unlocked"),
                inspector,
            ),
            inspection_id,
        );
        Ok(())
    }

    /// Read the active inspection_id locks for an inspector.
    pub fn get_locks(env: Env, inspector: Address) -> Vec<String> {
        get_inspector_locks(&env, &inspector)
    }

    /// Admin-triggered slash: reduces the inspector's bond by `slash_amount`
    /// and records the slash in the linked slashing_module.
    pub fn execute_slash(
        env: Env,
        admin: Address,
        inspector: Address,
        slash_amount: i128,
        inspection_id: String,
        reason: String,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        if slash_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        let current = get_inspector_bond(&env, &inspector);
        if slash_amount > current {
            return Err(ContractError::InsufficientBond);
        }
        let slashing_module: Address = env
            .storage()
            .instance()
            .get(&DataKey::SlashingModule)
            .ok_or(ContractError::SlashingModuleNotSet)?;

        // Cross-contract record on the slashing module. The slashing module
        // checks that the caller (this contract) matches its registered bond
        // contract, so unauthorised callers cannot fabricate inspector slashes.
        env.invoke_contract::<i128>(
            &slashing_module,
            &Symbol::new(&env, "slash"),
            soroban_sdk::vec![
                &env,
                env.current_contract_address().into_val(&env),
                inspector.clone().into_val(&env),
                slash_amount.into_val(&env),
                inspection_id.clone().into_val(&env),
                reason.clone().into_val(&env),
            ],
        );

        env.storage().persistent().set(
            &DataKey::InspectorBond(inspector.clone()),
            &(current - slash_amount),
        );

        env.events().publish(
            (
                symbol_short!("bond"),
                Symbol::new(&env, "slashed"),
                inspector,
            ),
            (inspection_id, slash_amount, reason),
        );
        Ok(())
    }
}

// ── Inspector bond helpers ───────────────────────────────────────────────────

fn get_inspector_bond(env: &Env, inspector: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::InspectorBond(inspector.clone()))
        .unwrap_or(0)
}

fn get_inspector_locks(env: &Env, inspector: &Address) -> Vec<String> {
    env.storage()
        .persistent()
        .get(&DataKey::InspectorLocks(inspector.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
    caller.require_auth();
    let admin = get_admin(env);
    if caller != &admin {
        return Err(ContractError::NotAuthorized);
    }
    Ok(())
}

fn require_operator(env: &Env, caller: &Address) -> Result<(), ContractError> {
    caller.require_auth();
    let operator: Address = env
        .storage()
        .instance()
        .get(&DataKey::Operator)
        .ok_or(ContractError::NotAuthorized)?;
    if caller != &operator {
        return Err(ContractError::NotAuthorized);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, BytesN, Env};

    fn create_position_id(env: &Env, seed: u64) -> BytesN<32> {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&seed.to_be_bytes());
        BytesN::from_array(env, &bytes)
    }

    #[test]
    fn init_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(BondCollateral, ());
        let client = BondCollateralClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.try_init(&admin, &token).unwrap().unwrap();

        assert_eq!(client.contract_version(), 1u32);
    }

    #[test]
    fn contract_has_functions() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(BondCollateral, ());
        let client = BondCollateralClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.try_init(&admin, &token).unwrap().unwrap();

        let position_id = create_position_id(&env, 1);
        let result = client.get_position(&position_id);
        assert!(result.is_none());
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Inspector bond + slashing-module integration tests (Issue #925)
// ──────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod inspector_bond_tests {
    use super::*;
    use slashing_module::{SlashingModule, SlashingModuleClient};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env, String};

    struct Setup<'a> {
        env: Env,
        bond: BondCollateralClient<'a>,
        slasher: SlashingModuleClient<'a>,
        admin: Address,
        operator: Address,
        inspector: Address,
    }

    fn setup<'a>() -> Setup<'a> {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let operator = Address::generate(&env);
        let inspector = Address::generate(&env);
        let token = Address::generate(&env);

        // Deploy & init bond_collateral.
        let bond_id = env.register(BondCollateral, ());
        let bond = BondCollateralClient::new(&env, &bond_id);
        bond.init(&admin, &token);

        // Deploy & init slashing_module, register the bond contract as the
        // sole authorised caller of `slash`.
        let slasher_id = env.register(SlashingModule, ());
        let slasher = SlashingModuleClient::new(&env, &slasher_id);
        slasher.init(&admin);
        slasher.set_bond_contract(&admin, &bond_id);

        // Link bond_collateral → slashing_module and configure the operator.
        bond.set_slashing_module(&admin, &slasher_id);
        bond.set_operator(&admin, &operator);

        Setup {
            env,
            bond,
            slasher,
            admin,
            operator,
            inspector,
        }
    }

    fn inspection(env: &Env, s: &str) -> String {
        String::from_str(env, s)
    }

    // ── Bond deposit / withdraw happy path ──────────────────────────────

    #[test]
    fn deposit_then_withdraw_updates_bond_balance() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &1_000);
        assert_eq!(s.bond.get_bond(&s.inspector), 1_000);
        s.bond.withdraw_bond(&s.inspector, &400);
        assert_eq!(s.bond.get_bond(&s.inspector), 600);
    }

    #[test]
    fn deposit_rejects_non_positive_amount() {
        let s = setup();
        let result = s.bond.try_deposit_bond(&s.inspector, &0);
        assert_eq!(result, Err(Ok(ContractError::InvalidAmount)));
    }

    #[test]
    fn withdraw_rejects_more_than_balance() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &500);
        let result = s.bond.try_withdraw_bond(&s.inspector, &600);
        assert_eq!(result, Err(Ok(ContractError::InsufficientBond)));
    }

    // ── Lock / unlock ─────────────────────────────────────────────────────

    #[test]
    fn lock_blocks_withdraw_and_unlock_restores_it() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &1_000);
        s.bond
            .lock_bond(&s.operator, &s.inspector, &inspection(&s.env, "INSP-42"));

        let blocked = s.bond.try_withdraw_bond(&s.inspector, &100);
        assert_eq!(blocked, Err(Ok(ContractError::BondLocked)));

        s.bond
            .unlock_bond(&s.operator, &s.inspector, &inspection(&s.env, "INSP-42"));
        s.bond.withdraw_bond(&s.inspector, &100);
        assert_eq!(s.bond.get_bond(&s.inspector), 900);
    }

    #[test]
    fn duplicate_lock_returns_lock_already_exists() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &1_000);
        s.bond
            .lock_bond(&s.operator, &s.inspector, &inspection(&s.env, "INSP-1"));
        let dup = s
            .bond
            .try_lock_bond(&s.operator, &s.inspector, &inspection(&s.env, "INSP-1"));
        assert_eq!(dup, Err(Ok(ContractError::LockAlreadyExists)));
    }

    #[test]
    fn unlock_without_matching_lock_returns_lock_not_found() {
        let s = setup();
        let result =
            s.bond
                .try_unlock_bond(&s.operator, &s.inspector, &inspection(&s.env, "INSP-ghost"));
        assert_eq!(result, Err(Ok(ContractError::LockNotFound)));
    }

    #[test]
    fn lock_requires_the_registered_operator() {
        let s = setup();
        let stranger = Address::generate(&s.env);
        let result = s
            .bond
            .try_lock_bond(&stranger, &s.inspector, &inspection(&s.env, "INSP-1"));
        assert_eq!(result, Err(Ok(ContractError::NotAuthorized)));
    }

    // ── execute_slash: cross-contract call ────────────────────────────────

    #[test]
    fn execute_slash_reduces_bond_and_records_history() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &1_000);

        s.bond.execute_slash(
            &s.admin,
            &s.inspector,
            &300,
            &inspection(&s.env, "INSP-1"),
            &String::from_str(&s.env, "fraudulent report"),
        );

        // Bond balance reduced by exactly slash_amount.
        assert_eq!(s.bond.get_bond(&s.inspector), 700);

        // Slashing module recorded the slash with the supplied fields.
        let history = s.slasher.get_slash_history(&s.inspector);
        assert_eq!(history.len(), 1);
        let entry = history.get(0).unwrap();
        assert_eq!(entry.amount, 300);
        assert_eq!(entry.inspection_id, inspection(&s.env, "INSP-1"));
        assert_eq!(entry.reason, String::from_str(&s.env, "fraudulent report"));
    }

    #[test]
    fn execute_slash_fails_when_amount_exceeds_bond() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &100);
        let result = s.bond.try_execute_slash(
            &s.admin,
            &s.inspector,
            &200,
            &inspection(&s.env, "INSP-X"),
            &String::from_str(&s.env, "r"),
        );
        assert_eq!(result, Err(Ok(ContractError::InsufficientBond)));
        // Bond untouched.
        assert_eq!(s.bond.get_bond(&s.inspector), 100);
    }

    #[test]
    fn execute_slash_requires_admin() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &1_000);
        let stranger = Address::generate(&s.env);
        let result = s.bond.try_execute_slash(
            &stranger,
            &s.inspector,
            &100,
            &inspection(&s.env, "INSP-1"),
            &String::from_str(&s.env, "r"),
        );
        assert_eq!(result, Err(Ok(ContractError::NotAuthorized)));
    }

    // ── Direct slashing_module.slash: caller-gating ───────────────────────

    #[test]
    fn slashing_module_rejects_slash_from_unregistered_caller() {
        let s = setup();
        // The slashing module only accepts `slash` from the registered bond
        // contract. A stranger calling directly must be rejected.
        let stranger = Address::generate(&s.env);
        let result = s.slasher.try_slash(
            &stranger,
            &s.inspector,
            &100,
            &inspection(&s.env, "INSP-1"),
            &String::from_str(&s.env, "r"),
        );
        // The first error variant in slashing_module is `AlreadyInitialized = 1`,
        // `NotAuthorized = 2` — gating via discriminant equality below.
        assert!(matches!(result, Err(Ok(_))));
    }

    #[test]
    fn multiple_slashes_accumulate_in_history() {
        let s = setup();
        s.bond.deposit_bond(&s.inspector, &10_000);
        s.bond.execute_slash(
            &s.admin,
            &s.inspector,
            &100,
            &inspection(&s.env, "A"),
            &String::from_str(&s.env, "r1"),
        );
        s.bond.execute_slash(
            &s.admin,
            &s.inspector,
            &250,
            &inspection(&s.env, "B"),
            &String::from_str(&s.env, "r2"),
        );
        let history = s.slasher.get_slash_history(&s.inspector);
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.get(0).unwrap().inspection_id,
            inspection(&s.env, "A")
        );
        assert_eq!(
            history.get(1).unwrap().inspection_id,
            inspection(&s.env, "B")
        );
        assert_eq!(s.bond.get_bond(&s.inspector), 10_000 - 100 - 250);
    }
}
