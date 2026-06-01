#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env, Symbol,
};

pub mod access_control;

const DEFAULT_STALENESS_SECONDS: u64 = 600;
const PRICE_DECIMALS: u32 = 7;

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceFeed {
    pub pair: Symbol,
    pub price: i128,
    pub decimals: u32,
    pub updated_at: u64,
    pub sequence: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Operator,
    StalenessThreshold,
    Feed(Symbol),
    Sequence(Symbol),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    InvalidSequence = 3,
    PriceTooStale = 4,
    UnknownPair = 5,
}

#[contract]
pub struct OraclePriceFeeds;

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Admin)
        .expect("admin not set")
}

fn get_operator(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&DataKey::Operator)
        .expect("operator not set")
}

fn get_staleness_threshold(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::StalenessThreshold)
        .unwrap_or(DEFAULT_STALENESS_SECONDS)
}

fn emit_price_updated(env: &Env, feed: &PriceFeed) {
    env.events().publish(
        (
            Symbol::new(env, "oracle"),
            Symbol::new(env, "price_updated"),
            feed.pair.clone(),
        ),
        (feed.price, feed.sequence, feed.updated_at),
    );
}

#[contractimpl]
impl OraclePriceFeeds {
    pub fn init(
        env: Env,
        admin: Address,
        operator: Address,
        staleness_threshold: u64,
    ) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        let threshold = if staleness_threshold == 0 {
            DEFAULT_STALENESS_SECONDS
        } else {
            staleness_threshold
        };
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage()
            .instance()
            .set(&DataKey::StalenessThreshold, &threshold);
        Ok(())
    }

    pub fn update_price(
        env: Env,
        caller: Address,
        pair: Symbol,
        price: i128,
        sequence: u64,
    ) -> Result<(), ContractError> {
        access_control::require_admin_or_operator_permission(
            &env,
            &get_admin(&env),
            &get_operator(&env),
            &caller,
            "update_price",
        )?;

        let current_seq: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Sequence(pair.clone()))
            .unwrap_or(0);

        if sequence <= current_seq {
            return Err(ContractError::InvalidSequence);
        }

        let feed = PriceFeed {
            pair: pair.clone(),
            price,
            decimals: PRICE_DECIMALS,
            updated_at: env.ledger().timestamp(),
            sequence,
        };

        env.storage()
            .instance()
            .set(&DataKey::Feed(pair.clone()), &feed);
        env.storage()
            .instance()
            .set(&DataKey::Sequence(pair.clone()), &sequence);

        emit_price_updated(&env, &feed);
        Ok(())
    }

    pub fn get_price(env: Env, pair: Symbol) -> PriceFeed {
        let feed = Self::get_price_unsafe(env.clone(), pair);
        let threshold = get_staleness_threshold(&env);
        let now = env.ledger().timestamp();
        if now.saturating_sub(feed.updated_at) > threshold {
            panic_with_error!(&env, ContractError::PriceTooStale);
        }
        feed
    }

    pub fn get_price_unsafe(env: Env, pair: Symbol) -> PriceFeed {
        env.storage()
            .instance()
            .get(&DataKey::Feed(pair))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::UnknownPair))
    }

    pub fn is_stale(env: Env, pair: Symbol) -> bool {
        if !env.storage().instance().has(&DataKey::Feed(pair.clone())) {
            return true;
        }
        let feed: PriceFeed = env.storage().instance().get(&DataKey::Feed(pair)).unwrap();
        let threshold = get_staleness_threshold(&env);
        let now = env.ledger().timestamp();
        now.saturating_sub(feed.updated_at) > threshold
    }

    pub fn set_staleness_threshold(
        env: Env,
        caller: Address,
        threshold: u64,
    ) -> Result<(), ContractError> {
        access_control::require_admin_permission(
            &env,
            &get_admin(&env),
            &caller,
            "set_staleness_threshold",
        )?;
        env.storage()
            .instance()
            .set(&DataKey::StalenessThreshold, &threshold);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke};
    use soroban_sdk::{Address, Env, IntoVal, Symbol};

    fn pair(env: &Env) -> Symbol {
        Symbol::new(env, "NGN_USDC")
    }

    fn setup(
        env: &Env,
    ) -> (
        Address,
        OraclePriceFeedsClient<'_>,
        Address,
        Address,
        Symbol,
    ) {
        let contract_id = env.register(OraclePriceFeeds, ());
        let client = OraclePriceFeedsClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let operator = Address::generate(env);
        let p = pair(env);
        client
            .try_init(&admin, &operator, &600u64)
            .unwrap()
            .unwrap();
        (contract_id, client, admin, operator, p)
    }

    #[test]
    fn update_price_rejects_replay_sequence() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (contract_id, client, _admin, operator, p) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_price",
                args: (operator.clone(), p.clone(), 6170i128, 1u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_price(&operator, &p, &6170i128, &1u64)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_price",
                args: (operator.clone(), p.clone(), 6200i128, 1u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_price(&operator, &p, &6200i128, &1u64)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidSequence);
    }

    #[test]
    fn get_price_panics_when_stale() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (contract_id, client, _admin, operator, p) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_price",
                args: (operator.clone(), p.clone(), 6170i128, 1u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_price(&operator, &p, &6170i128, &1u64)
            .unwrap()
            .unwrap();

        env.ledger().set_timestamp(1_000 + 601);
        assert!(client.is_stale(&p));
        let _ = client.try_get_price(&p).unwrap_err();
    }

    #[test]
    fn get_price_unsafe_returns_without_staleness_check() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (contract_id, client, _admin, operator, p) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_price",
                args: (operator.clone(), p.clone(), 6170i128, 1u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_price(&operator, &p, &6170i128, &1u64)
            .unwrap()
            .unwrap();

        env.ledger().set_timestamp(1_000 + 900);
        let feed = client.get_price_unsafe(&p);
        assert_eq!(feed.price, 6170);
        assert_eq!(feed.decimals, 7);
    }

    #[test]
    fn operator_auth_required() {
        let env = Env::default();
        let (contract_id, client, _admin, _operator, p) = setup(&env);
        let stranger = Address::generate(&env);

        env.mock_auths(&[MockAuth {
            address: &stranger,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_price",
                args: (stranger.clone(), p.clone(), 6170i128, 1u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_price(&stranger, &p, &6170i128, &1u64)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::NotAuthorized);
    }
}
