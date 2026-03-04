#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Env, Map, Symbol,
};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    StakedBalances,
    TotalStaked,
    Paused,
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

fn staked_balances(env: &Env) -> Map<Address, i128> {
    env.storage()
        .instance()
        .get::<_, Map<Address, i128>>(&DataKey::StakedBalances)
        .unwrap_or_else(|| Map::new(env))
}

fn put_staked_balances(env: &Env, balances: Map<Address, i128>) {
    env.storage().instance().set(&DataKey::StakedBalances, &balances);
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
            .set(&DataKey::StakedBalances, &Map::<Address, i128>::new(&env));
        env.storage().instance().set(&DataKey::TotalStaked, &0i128);

        env.events().publish((Symbol::new(&env, "init"),), admin);
    }

    pub fn stake(env: Env, from: Address, amount: i128) {
        from.require_auth();
        require_not_paused(&env);
        require_positive_amount(amount);

        let token_address = get_token(&env);
        let token_client = token::Client::new(&env, &token_address);

        // Transfer tokens from user to contract
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        // Update staked balance
        let mut balances = staked_balances(&env);
        let current_balance = balances.get(from.clone()).unwrap_or(0);
        balances.set(from.clone(), current_balance + amount);
        put_staked_balances(&env, balances);

        // Update total staked
        let total = get_total_staked(&env);
        put_total_staked(&env, total + amount);

        // Emit event
        let new_user_balance = current_balance + amount;
        let new_total = total + amount;
        env.events().publish(
            (Symbol::new(&env, "stake"), from.clone()),
            (amount, new_user_balance, new_total),
        );
    }

    pub fn unstake(env: Env, to: Address, amount: i128) {
        to.require_auth();
        require_not_paused(&env);
        require_positive_amount(amount);

        // Check sufficient staked balance
        let mut balances = staked_balances(&env);
        let current_balance = balances.get(to.clone()).unwrap_or(0);
        if current_balance < amount {
            panic!("insufficient staked balance");
        }

        let token_address = get_token(&env);
        let token_client = token::Client::new(&env, &token_address);

        // Update staked balance
        balances.set(to.clone(), current_balance - amount);
        put_staked_balances(&env, balances);

        // Update total staked
        let total = get_total_staked(&env);
        put_total_staked(&env, total - amount);

        // Transfer tokens from contract to user
        token_client.transfer(&env.current_contract_address(), &to, &amount);

        // Emit event
        let new_user_balance = current_balance - amount;
        let new_total = total - amount;
        env.events().publish(
            (Symbol::new(&env, "unstake"), to.clone()),
            (amount, new_user_balance, new_total),
        );
    }

    pub fn staked_balance(env: Env, user: Address) -> i128 {
        let balances = staked_balances(&env);
        balances.get(user).unwrap_or(0)
    }

    pub fn total_staked(env: Env) -> i128 {
        get_total_staked(&env)
    }

    pub fn pause(env: Env) {
        require_admin(&env);
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((Symbol::new(&env, "pause"),), ());
    }

    pub fn unpause(env: Env) {
        require_admin(&env);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((Symbol::new(&env, "unpause"),), ());
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }
}

#[cfg(test)]
mod test {
    extern crate std;

    use super::{StakingPool, StakingPoolClient};
    use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
    use soroban_sdk::{
        Address, Env, IntoVal, Symbol, TryIntoVal,
    };

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
        assert!(client.is_paused());
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
    fn is_paused_returns_false_initially() {
        let env = Env::default();
        let (_contract_id, client, _admin, _user, _token_id) = setup_contract(&env);
        assert!(!client.is_paused());
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
        assert!(client.is_paused());

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
        assert!(!client.is_paused());
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
    // Event Tests
    // ============================================================================

    #[test]
    fn pause_emits_event() {
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

        let events = env.events().all();
        let pause_event = events.last().unwrap();

        let topics: soroban_sdk::Vec<soroban_sdk::Val> = pause_event.1.clone();
        assert_eq!(topics.len(), 1);

        let event_name: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(event_name, Symbol::new(&env, "pause"));
    }

    #[test]
    fn unpause_emits_event() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _token_id) = setup_contract(&env);

        // First pause
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

        // Then unpause
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

        let events = env.events().all();
        let unpause_event = events.last().unwrap();

        let topics: soroban_sdk::Vec<soroban_sdk::Val> = unpause_event.1.clone();
        assert_eq!(topics.len(), 1);

        let event_name: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(event_name, Symbol::new(&env, "unpause"));
    }

    #[test]
    fn init_emits_event() {
        let env = Env::default();
        let contract_id = env.register(StakingPool, ());
        let client = StakingPoolClient::new(&env, &contract_id);
        
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin);
        let token_contract_id = token_contract.address();

        client.init(&admin, &token_contract_id);

        let events = env.events().all();
        let init_event = events.last().unwrap();

        let topics: soroban_sdk::Vec<soroban_sdk::Val> = init_event.1.clone();
        assert_eq!(topics.len(), 1);

        let event_name: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(event_name, Symbol::new(&env, "init"));

        let data: Address = init_event.2.try_into_val(&env).unwrap();
        assert_eq!(data, admin);
    }
}
