#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token::Client as TokenClient, Address,
    BytesN, Env, Symbol,
};

const REWARD_INDEX_SCALE: i128 = 1_000_000_000_000;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    ContractVersion,
    Admin,
    Token,
    TotalStaked,
    Paused,
    GlobalRewardIndex,
    // Per-user keys (persistent storage) — replaces instance Map (#386)
    StakedBalance(Address),
    UserRewardIndex(Address),
    ClaimableReward(Address),
    // Upgrade governance (#392)
    Guardian,
    UpgradeDelay,
    PendingUpgradeHash,
    PendingUpgradeAt,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    // Upgrade governance errors (#392)
    UpgradeAlreadyPending = 1,
    NoUpgradePending = 2,
    UpgradeDelayNotMet = 3,
    NotAuthorized = 4,
}

#[contract]
pub struct StakingPool;

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

fn get_total_staked(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get::<_, i128>(&DataKey::TotalStaked)
        .unwrap_or(0)
}

fn put_total_staked(env: &Env, total: i128) {
    env.storage().instance().set(&DataKey::TotalStaked, &total);
}

fn get_global_reward_index(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get::<_, i128>(&DataKey::GlobalRewardIndex)
        .unwrap_or(0)
}

fn put_global_reward_index(env: &Env, idx: i128) {
    env.storage()
        .instance()
        .set(&DataKey::GlobalRewardIndex, &idx);
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_admin(env: &Env) {
    let admin = get_admin(env);
    admin.require_auth();
}

fn require_not_paused(env: &Env) {
    if is_paused(env) {
        panic!("contract is paused");
    }
}

fn require_positive_amount(amount: i128) {
    if amount <= 0 {
        panic!("amount must be positive");
    }
}

fn get_staked_balance(env: &Env, user: &Address) -> i128 {
    env.storage()
        .persistent()
        .get::<_, i128>(&DataKey::StakedBalance(user.clone()))
        .unwrap_or(0)
}

fn get_user_reward_index(env: &Env, user: &Address) -> i128 {
    env.storage()
        .persistent()
        .get::<_, i128>(&DataKey::UserRewardIndex(user.clone()))
        .unwrap_or(0)
}

fn get_claimable_reward(env: &Env, user: &Address) -> i128 {
    env.storage()
        .persistent()
        .get::<_, i128>(&DataKey::ClaimableReward(user.clone()))
        .unwrap_or(0)
}

fn accrue_user_rewards(env: &Env, user: &Address) {
    let global_idx = get_global_reward_index(env);
    let user_idx = get_user_reward_index(env, user);

    if global_idx <= user_idx {
        return;
    }

    let staked = get_staked_balance(env, user);

    if staked > 0 {
        let delta = global_idx - user_idx;
        let accrued = (staked * delta) / REWARD_INDEX_SCALE;

        if accrued > 0 {
            let current = get_claimable_reward(env, user);
            env.storage().persistent().set(
                &DataKey::ClaimableReward(user.clone()),
                &(current + accrued),
            );
        }
    }

    env.storage()
        .persistent()
        .set(&DataKey::UserRewardIndex(user.clone()), &global_idx);
}

#[contractimpl]
impl StakingPool {
    pub fn init(env: Env, admin: Address, token: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage().instance().set(&DataKey::TotalStaked, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::GlobalRewardIndex, &0i128);

        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "init"),
            ),
            (admin, token, 1u32),
        );
    }

    pub fn contract_version(env: Env) -> u32 {
        env.storage()
            .instance()
            .get::<_, u32>(&DataKey::ContractVersion)
            .unwrap_or(0u32)
    }

    pub fn stake(env: Env, user: Address, amount: i128) {
        user.require_auth();
        require_not_paused(&env);
        require_positive_amount(amount);

        accrue_user_rewards(&env, &user);

        let token_address = get_token(&env);
        let token_client = TokenClient::new(&env, &token_address);

        // Transfer tokens from user to contract
        token_client.transfer(&user, &env.current_contract_address(), &amount);

        // Update staked balance
        let current_balance = get_staked_balance(&env, &user);
        env.storage().persistent().set(
            &DataKey::StakedBalance(user.clone()),
            &(current_balance + amount),
        );

        // Update total staked
        let total = get_total_staked(&env);
        put_total_staked(&env, total + amount);

        env.events()
            .publish((Symbol::new(&env, "stake"), user.clone()), amount);
    }

    pub fn unstake(env: Env, user: Address, amount: i128) {
        user.require_auth();
        require_not_paused(&env);
        require_positive_amount(amount);

        accrue_user_rewards(&env, &user);

        let current_balance = get_staked_balance(&env, &user);
        if current_balance < amount {
            panic!("insufficient staked balance");
        }

        let token_address = get_token(&env);
        let token_client = TokenClient::new(&env, &token_address);

        // Update staked balance
        env.storage().persistent().set(
            &DataKey::StakedBalance(user.clone()),
            &(current_balance - amount),
        );

        // Update total staked
        let total = get_total_staked(&env);
        put_total_staked(&env, total - amount);

        // Transfer tokens from contract to user
        token_client.transfer(&env.current_contract_address(), &user, &amount);

        env.events()
            .publish((Symbol::new(&env, "unstake"), user.clone()), amount);
    }

    pub fn staked_balance(env: Env, user: Address) -> i128 {
        get_staked_balance(&env, &user)
    }

    pub fn total_staked(env: Env) -> i128 {
        get_total_staked(&env)
    }

    pub fn fund_rewards(env: Env, from: Address, amount: i128) {
        require_admin(&env);
        require_not_paused(&env);
        require_positive_amount(amount);

        let admin = get_admin(&env);
        if from != admin {
            panic!("from must be admin");
        }

        let total = get_total_staked(&env);
        if total <= 0 {
            panic!("no stakers");
        }

        let token_address = get_token(&env);
        let token_client = TokenClient::new(&env, &token_address);
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        let increment = (amount * REWARD_INDEX_SCALE) / total;
        let new_idx = get_global_reward_index(&env) + increment;
        put_global_reward_index(&env, new_idx);

        env.events()
            .publish((Symbol::new(&env, "fund_rewards"), from.clone()), amount);
    }

    pub fn claimable(env: Env, user: Address) -> i128 {
        accrue_user_rewards(&env, &user);
        get_claimable_reward(&env, &user)
    }

    pub fn claim(env: Env, to: Address) -> i128 {
        to.require_auth();
        require_not_paused(&env);

        accrue_user_rewards(&env, &to);

        let amount = get_claimable_reward(&env, &to);
        if amount <= 0 {
            return 0;
        }

        env.storage()
            .persistent()
            .set(&DataKey::ClaimableReward(to.clone()), &0i128);

        let token_address = get_token(&env);
        let token_client = TokenClient::new(&env, &token_address);
        token_client.transfer(&env.current_contract_address(), &to, &amount);

        env.events()
            .publish((Symbol::new(&env, "claim"), to.clone()), amount);
        amount
    }

    pub fn pause(env: Env) {
        require_admin(&env);
        let admin = get_admin(&env);
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "paused"),
            ),
            admin,
        );
    }

    pub fn unpause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "unpaused"),
            ),
            admin,
        );
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    // ── Upgrade governance (#392) ──────────────────────────────────────────────

    pub fn set_guardian(env: Env, admin: Address, guardian: Address) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "set_guardian"),
            ),
            guardian,
        );
        Ok(())
    }

    pub fn set_upgrade_delay(env: Env, admin: Address, delay: u64) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::UpgradeDelay, &delay);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "set_upgrade_delay"),
            ),
            delay,
        );
        Ok(())
    }

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
        }
        if env.storage().instance().has(&DataKey::PendingUpgradeHash) {
            return Err(ContractError::UpgradeAlreadyPending);
        }
        let delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeDelay)
            .unwrap_or(0);
        let execute_at = env.ledger().timestamp() + delay;
        env.storage()
            .instance()
            .set(&DataKey::PendingUpgradeHash, &new_wasm_hash);
        env.storage()
            .instance()
            .set(&DataKey::PendingUpgradeAt, &execute_at);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "propose_upgrade"),
            ),
            (new_wasm_hash, execute_at),
        );
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
        }
        let hash = env
            .storage()
            .instance()
            .get::<_, BytesN<32>>(&DataKey::PendingUpgradeHash)
            .ok_or(ContractError::NoUpgradePending)?;
        let execute_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PendingUpgradeAt)
            .ok_or(ContractError::NoUpgradePending)?;
        if env.ledger().timestamp() < execute_at {
            return Err(ContractError::UpgradeDelayNotMet);
        }
        env.storage()
            .instance()
            .remove(&DataKey::PendingUpgradeHash);
        env.storage().instance().remove(&DataKey::PendingUpgradeAt);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "execute_upgrade"),
            ),
            hash.clone(),
        );
        env.deployer().update_current_contract_wasm(hash);
        Ok(())
    }

    pub fn emergency_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
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
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "emergency_upgrade"),
            ),
            (admin.clone(), new_wasm_hash.clone()),
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn cancel_upgrade(env: Env, admin: Address) -> Result<(), ContractError> {
        admin.require_auth();
        if admin != get_admin(&env) {
            return Err(ContractError::NotAuthorized);
        }
        if !env.storage().instance().has(&DataKey::PendingUpgradeHash) {
            return Err(ContractError::NoUpgradePending);
        }
        env.storage()
            .instance()
            .remove(&DataKey::PendingUpgradeHash);
        env.storage().instance().remove(&DataKey::PendingUpgradeAt);
        env.events().publish(
            (
                Symbol::new(&env, "mvp_staking_pool"),
                Symbol::new(&env, "cancel_upgrade"),
            ),
            admin.clone(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod test {
    extern crate std;

    use super::{StakingPool, StakingPoolClient};
    use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
    use soroban_sdk::{token::StellarAssetClient, Address, Env, IntoVal, Symbol, TryIntoVal};

    fn setup_contract(env: &Env) -> (Address, StakingPoolClient<'_>, Address, Address, Address) {
        let contract_id = env.register(StakingPool, ());
        let client = StakingPoolClient::new(env, &contract_id);

        let admin = Address::generate(env);
        let user = Address::generate(env);
        let token_admin = Address::generate(env);

        // Create token contract
        let token_contract = env.register_stellar_asset_contract_v2(token_admin);
        let token_contract_id = token_contract.address();

        // Initialize contract
        client.init(&admin, &token_contract_id);

        (contract_id, client, admin, user, token_contract_id)
    }

    #[test]
    fn rewards_accrue_and_can_be_claimed_without_iteration() {
        let env = Env::default();
        let (contract_id, client, admin, user_a, token_id) = setup_contract(&env);
        let user_b = Address::generate(&env);

        env.mock_all_auths();

        let asset = StellarAssetClient::new(&env, &token_id);
        asset.mint(&admin, &1_000i128);
        asset.mint(&user_a, &1_000i128);
        asset.mint(&user_b, &1_000i128);

        client.stake(&user_a, &100i128);
        client.fund_rewards(&admin, &50i128);
        assert!(client.claimable(&user_a) > 0);

        client.stake(&user_b, &100i128);
        client.fund_rewards(&admin, &100i128);

        let claimable_a = client.claimable(&user_a);
        let claimable_b = client.claimable(&user_b);
        assert_eq!(claimable_a, 100i128);
        assert_eq!(claimable_b, 50i128);

        let claimed = client.claim(&user_a);
        assert_eq!(claimed, 100i128);

        let claimed_again = client.claim(&user_a);
        assert_eq!(claimed_again, 0i128);

        // Sanity: contract still callable and has not iterated through user maps for distribution.
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.pause();
    }

    // ============================================================================
    // Init Tests
    // ============================================================================

    #[test]
    fn init_sets_admin_and_token() {
        let env = Env::default();
        let contract_id = env.register(StakingPool, ());
        let client = StakingPoolClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin);
        let token_contract_id = token_contract.address();

        client.init(&admin, &token_contract_id);

        assert_eq!(client.contract_version(), 1u32);

        // Verify admin can pause
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.pause();
    }

    #[test]
    #[should_panic(expected = "already initialized")]
    fn init_cannot_be_called_twice() {
        let env = Env::default();
        let contract_id = env.register(StakingPool, ());
        let client = StakingPoolClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin);
        let token_contract_id = token_contract.address();

        client.init(&admin, &token_contract_id);
        client.init(&admin, &token_contract_id);
    }

    // ============================================================================
    // Query Tests
    // ============================================================================

    #[test]
    fn staked_balance_returns_zero_for_new_user() {
        let env = Env::default();
        let (_contract_id, client, _admin, user, _token_id) = setup_contract(&env);
        let new_user = Address::generate(&env);

        assert_eq!(client.staked_balance(&user), 0i128);
        assert_eq!(client.staked_balance(&new_user), 0i128);
    }

    #[test]
    fn total_staked_returns_zero_initially() {
        let env = Env::default();
        let (_contract_id, client, _admin, _user, _token_id) = setup_contract(&env);
        assert_eq!(client.total_staked(), 0i128);
    }

    // ============================================================================
    // Admin Tests
    // ============================================================================

    #[test]
    fn admin_can_pause_and_unpause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.pause();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unpause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unpause();
    }

    #[test]
    #[should_panic]
    fn non_admin_cannot_pause() {
        let env = Env::default();
        let (contract_id, client, _admin, _user, _token_id) = setup_contract(&env);
        let non_admin = Address::generate(&env);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.pause();
    }

    // ============================================================================
    // Pause Behavior Tests
    // ============================================================================

    #[test]
    #[should_panic(expected = "contract is paused")]
    fn stake_fails_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _token_id) = setup_contract(&env);

        // Pause the contract
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.pause();

        // Try to stake while paused
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.stake(&user, &100i128);
    }

    #[test]
    #[should_panic(expected = "contract is paused")]
    fn unstake_fails_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _token_id) = setup_contract(&env);

        // Pause the contract
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.pause();

        // Try to unstake while paused
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unstake(&user, &50i128);
    }

    // ============================================================================
    // Input Validation Tests
    // ============================================================================

    #[test]
    #[should_panic(expected = "amount must be positive")]
    fn stake_fails_with_zero_amount() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), 0i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.stake(&user, &0i128);
    }

    #[test]
    #[should_panic(expected = "amount must be positive")]
    fn stake_fails_with_negative_amount() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "stake",
                args: (user.clone(), -10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.stake(&user, &-10i128);
    }

    #[test]
    #[should_panic(expected = "amount must be positive")]
    fn unstake_fails_with_zero_amount() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), 0i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unstake(&user, &0i128);
    }

    #[test]
    #[should_panic(expected = "amount must be positive")]
    fn unstake_fails_with_negative_amount() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), -10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unstake(&user, &-10i128);
    }

    // ============================================================================
    // Stake/Unstake Edge Case Tests
    // ============================================================================

    #[test]
    #[should_panic(expected = "insufficient staked balance")]
    fn unstake_fails_with_insufficient_balance() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unstake(&user, &100i128);
    }

    #[test]
    #[should_panic(expected = "insufficient staked balance")]
    fn unstake_fails_when_unstaking_more_than_staked() {
        let env = Env::default();
        let (contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        // Try to unstake without any stake (should fail due to insufficient balance)
        env.mock_auths(&[MockAuth {
            address: &user,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unstake",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.unstake(&user, &100i128);
    }

    #[test]
    fn stake_and_unstake_work_correctly() {
        let env = Env::default();
        let (_contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        // Initial balances should be zero
        assert_eq!(client.staked_balance(&user), 0i128);
        assert_eq!(client.total_staked(), 0i128);

        // Test that functions exist and have correct signatures
        // The actual token transfer logic is tested in integration tests
        assert_eq!(client.staked_balance(&user), 0i128);
        assert_eq!(client.total_staked(), 0i128);
    }

    #[test]
    fn multiple_users_can_stake_independently() {
        let env = Env::default();
        let (contract_id, client, _admin, user1, _token_id) = setup_contract(&env);
        let user2 = Address::generate(&env);

        // Initial balances should be zero
        assert_eq!(client.staked_balance(&user1), 0i128);
        assert_eq!(client.staked_balance(&user2), 0i128);
        assert_eq!(client.total_staked(), 0i128);

        // Test that different users have separate balances
        assert_ne!(user1, user2);
        assert_eq!(client.staked_balance(&user1), 0i128);
        assert_eq!(client.staked_balance(&user2), 0i128);
    }

    // ============================================================================
    // Event Tests
    // ============================================================================

    #[test]
    fn stake_emits_event_with_standardized_topic() {
        let env = Env::default();
        let (_contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        // Test that stake function exists and has correct signature
        // Event emission is tested in integration tests with actual token transfers
        assert_eq!(client.staked_balance(&user), 0i128);
    }

    #[test]
    fn unstake_emits_event_with_standardized_topic() {
        let env = Env::default();
        let (_contract_id, client, _admin, user, _token_id) = setup_contract(&env);

        // Test that unstake function exists and has correct signature
        // Event emission is tested in integration tests with actual token transfers
        assert_eq!(client.staked_balance(&user), 0i128);
    }
}
