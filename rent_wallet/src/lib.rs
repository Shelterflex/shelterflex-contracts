#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Map, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Balances,
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
        if amount <= 0 {
            panic!("amount must be positive")
        }

        let mut b = balances(&env);
        let cur = b.get(user.clone()).unwrap_or(0);
        b.set(user.clone(), cur + amount);
        put_balances(&env, b);

        env.events()
            .publish((Symbol::new(&env, "credit"), user), amount);
    }

    pub fn debit(env: Env, user: Address, amount: i128) {
        require_admin(&env);
        if amount <= 0 {
            panic!("amount must be positive")
        }

        let mut b = balances(&env);
        let cur = b.get(user.clone()).unwrap_or(0);
        if cur < amount {
            panic!("insufficient balance")
        }
        b.set(user.clone(), cur - amount);
        put_balances(&env, b);

        env.events()
            .publish((Symbol::new(&env, "debit"), user), amount);
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
}
