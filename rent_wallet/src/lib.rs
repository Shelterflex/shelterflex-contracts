#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Map, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Balances,
    Paused,
}

#[contract]
pub struct RentWallet;

fn balances(env: &Env) -> Map<Address, i128> {
    env.storage()
        .instance()
        .get::<_, Map<Address, i128>>(&DataKey::Balances)
        .unwrap_or_else(|| Map::new(env))
}

fn put_balances(env: &Env, b: Map<Address, i128>) {
    env.storage().instance().set(&DataKey::Balances, &b)
}

fn require_admin(env: &Env) {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("admin not set");
    admin.require_auth();
}

fn get_paused_state(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) {
    if get_paused_state(env) {
        panic!("contract is paused")
    }
}

#[contractimpl]
impl RentWallet {
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized")
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Balances, &Map::<Address, i128>::new(&env));

        env.events().publish((Symbol::new(&env, "init"),), admin);
    }

    pub fn credit(env: Env, user: Address, amount: i128) {
        require_admin(&env);
        require_not_paused(&env);
        if amount <= 0 {
            panic!("amount must be positive")
        }

        let mut b = balances(&env);
        let cur = b.get(user.clone()).unwrap_or(0);
        let new_balance = cur + amount;
        b.set(user.clone(), new_balance);
        put_balances(&env, b);

        env.events()
            .publish((Symbol::new(&env, "credit"), user), (amount, new_balance));
    }

    pub fn debit(env: Env, user: Address, amount: i128) {
        require_admin(&env);
        require_not_paused(&env);
        if amount <= 0 {
            panic!("amount must be positive")
        }

        let mut b = balances(&env);
        let cur = b.get(user.clone()).unwrap_or(0);
        if cur < amount {
            panic!("insufficient balance")
        }
        let new_balance = cur - amount;
        b.set(user.clone(), new_balance);
        put_balances(&env, b);

        env.events()
            .publish((Symbol::new(&env, "debit"), user), (amount, new_balance));
    }

    pub fn balance(env: Env, user: Address) -> i128 {
        let b = balances(&env);
        b.get(user).unwrap_or(0)
    }

    pub fn set_admin(env: Env, new_admin: Address) {
        require_admin(&env);
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events()
            .publish((Symbol::new(&env, "set_admin"),), new_admin);
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
        get_paused_state(&env)
    }
}

#[cfg(test)]
mod test {
    extern crate std;

    use super::{RentWallet, RentWalletClient};
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::{Address, Env, IntoVal};

    fn setup(env: &Env) -> (soroban_sdk::Address, RentWalletClient<'_>, Address, Address, Address) {
        let contract_id = env.register_contract(None, RentWallet);
        let client = RentWalletClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let user = Address::generate(env);
        let non_admin = Address::generate(env);
        client.init(&admin);
        (contract_id, client, admin, user, non_admin)
    }

    #[test]
    #[should_panic]
    fn non_admin_cannot_credit() {
        let env = Env::default();
        let (contract_id, client, _admin, user, non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.credit(&user, &100i128);
    }

    #[test]
    #[should_panic]
    fn non_admin_cannot_debit() {
        let env = Env::default();
        let (contract_id, client, _admin, user, non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (user.clone(), 1i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.debit(&user, &1i128);
    }

    #[test]
    #[should_panic]
    fn non_admin_cannot_set_admin() {
        let env = Env::default();
        let (contract_id, client, _admin, _user, non_admin) = setup(&env);
        let new_admin = Address::generate(&env);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_admin",
                args: (new_admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.set_admin(&new_admin);
    }

    #[test]
    fn admin_can_pause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);

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
    fn admin_can_unpause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);

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
        assert!(client.is_paused());

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
        assert!(!client.is_paused());
    }

    #[test]
    #[should_panic]
    fn non_admin_cannot_pause() {
        let env = Env::default();
        let (contract_id, client, _admin, _user, non_admin) = setup(&env);

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

    #[test]
    #[should_panic]
    fn non_admin_cannot_unpause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, non_admin) = setup(&env);

        // First pause as admin
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

        // Try to unpause as non-admin
        env.mock_auths(&[MockAuth {
            address: &non_admin,
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
    fn credit_fails_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

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

        // Try to credit while paused
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.credit(&user, &100i128);
    }

    #[test]
    #[should_panic]
    fn debit_fails_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // First credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.credit(&user, &100i128);

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

        // Try to debit while paused
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        client.debit(&user, &50i128);
    }

    #[test]
    fn balance_works_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // Credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.credit(&user, &100i128);
        assert_eq!(client.balance(&user), 100i128);

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

        // Balance should still be readable
        assert_eq!(client.balance(&user), 100i128);
    }

    #[test]
    fn is_paused_returns_false_initially() {
        let env = Env::default();
        let (_contract_id, client, _admin, _user, _non_admin) = setup(&env);
        assert!(!client.is_paused());
    }
}
