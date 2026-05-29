#![no_std]

use soroban_pausable::{Pausable, PausableError};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

pub mod access_control;

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReputationRecord {
    pub composite_score: u32,
    pub payment_score: u32,
    pub property_care_score: u32,
    pub communication_score: u32,
    pub total_ratings: u32,
    pub last_updated: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Operator,
    Paused,
    Reputation(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    Paused = 3,
    InvalidScore = 4,
}

#[contract]
pub struct TenantReputation;

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

fn require_not_paused(env: &Env) -> Result<(), ContractError> {
    let paused = env
        .storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        return Err(ContractError::Paused);
    }
    Ok(())
}

fn validate_record(record: &ReputationRecord) -> Result<(), ContractError> {
    if record.composite_score > 1000 {
        return Err(ContractError::InvalidScore);
    }
    Ok(())
}

fn emit_updated(env: &Env, tenant: &Address, record: &ReputationRecord) {
    env.events().publish(
        (
            soroban_sdk::Symbol::new(env, "tenant_reputation"),
            soroban_sdk::Symbol::new(env, "updated"),
            tenant.clone(),
        ),
        (
            record.composite_score,
            record.total_ratings,
            record.last_updated,
        ),
    );
}

#[contractimpl]
impl TenantReputation {
    pub fn init(env: Env, admin: Address, operator: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn update_reputation(
        env: Env,
        caller: Address,
        tenant: Address,
        record: ReputationRecord,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        access_control::require_admin_or_operator_permission(
            &env,
            &get_admin(&env),
            &get_operator(&env),
            &caller,
            "update_reputation",
        )?;
        validate_record(&record)?;
        let updated = ReputationRecord {
            last_updated: env.ledger().timestamp(),
            ..record
        };
        env.storage()
            .persistent()
            .set(&DataKey::Reputation(tenant.clone()), &updated);
        emit_updated(&env, &tenant, &updated);
        Ok(())
    }

    pub fn get_reputation(env: Env, tenant: Address) -> Option<ReputationRecord> {
        env.storage().persistent().get(&DataKey::Reputation(tenant))
    }

    pub fn has_reputation(env: Env, tenant: Address) -> bool {
        env.storage().persistent().has(&DataKey::Reputation(tenant))
    }

    pub fn revoke_reputation(
        env: Env,
        caller: Address,
        tenant: Address,
    ) -> Result<(), ContractError> {
        access_control::require_admin_permission(
            &env,
            &get_admin(&env),
            &caller,
            "revoke_reputation",
        )?;
        if env
            .storage()
            .persistent()
            .has(&DataKey::Reputation(tenant.clone()))
        {
            env.storage()
                .persistent()
                .remove(&DataKey::Reputation(tenant.clone()));
            env.events().publish(
                (
                    soroban_sdk::Symbol::new(&env, "tenant_reputation"),
                    soroban_sdk::Symbol::new(&env, "revoked"),
                    tenant,
                ),
                (),
            );
        }
        Ok(())
    }
}

#[contractimpl]
impl Pausable for TenantReputation {
    fn pause(env: Env, admin: Address) -> Result<(), PausableError> {
        access_control::require_admin_permission(&env, &get_admin(&env), &admin, "pause")
            .map_err(|_| PausableError::NotAuthorized)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "Pausable"),
                soroban_sdk::Symbol::new(&env, "pause"),
            ),
            (),
        );
        Ok(())
    }

    fn unpause(env: Env, admin: Address) -> Result<(), PausableError> {
        access_control::require_admin_permission(&env, &get_admin(&env), &admin, "unpause")
            .map_err(|_| PausableError::NotAuthorized)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke};
    use soroban_sdk::{Address, Env, IntoVal};

    fn sample_record(env: &Env) -> ReputationRecord {
        ReputationRecord {
            composite_score: 750,
            payment_score: 80,
            property_care_score: 70,
            communication_score: 90,
            total_ratings: 5,
            last_updated: env.ledger().timestamp(),
        }
    }

    fn setup(env: &Env) -> (Address, TenantReputationClient<'_>, Address, Address) {
        let contract_id = env.register(TenantReputation, ());
        let client = TenantReputationClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let operator = Address::generate(env);
        client.try_init(&admin, &operator).unwrap().unwrap();
        (contract_id, client, admin, operator)
    }

    #[test]
    fn init_succeeds_once() {
        let env = Env::default();
        let (_id, client, admin, operator) = setup(&env);
        assert!(!client.is_paused());
        let _ = (admin, operator);
    }

    #[test]
    fn init_cannot_be_called_twice() {
        let env = Env::default();
        let (_id, client, admin, operator) = setup(&env);
        let err = client.try_init(&admin, &operator).unwrap_err().unwrap();
        assert_eq!(err, ContractError::AlreadyInitialized);
    }

    #[test]
    fn get_reputation_returns_none_for_unknown() {
        let env = Env::default();
        let (_id, client, _admin, _op) = setup(&env);
        let tenant = Address::generate(&env);
        assert_eq!(client.get_reputation(&tenant), None);
        assert!(!client.has_reputation(&tenant));
    }

    #[test]
    fn operator_can_update_and_overwrite() {
        let env = Env::default();
        env.ledger().set_timestamp(100);
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let record = sample_record(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record)
            .unwrap()
            .unwrap();

        let stored = client.get_reputation(&tenant).unwrap();
        assert_eq!(stored.composite_score, 750);
        assert!(client.has_reputation(&tenant));

        let mut updated = record.clone();
        updated.composite_score = 800;
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (admin.clone(), tenant.clone(), updated.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&admin, &tenant, &updated)
            .unwrap()
            .unwrap();
        assert_eq!(client.get_reputation(&tenant).unwrap().composite_score, 800);
    }

    #[test]
    fn unauthorized_update_panics() {
        let env = Env::default();
        let (contract_id, client, _admin, _operator) = setup(&env);
        let tenant = Address::generate(&env);
        let stranger = Address::generate(&env);
        let record = sample_record(&env);

        env.mock_auths(&[MockAuth {
            address: &stranger,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (stranger.clone(), tenant.clone(), record.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_reputation(&stranger, &tenant, &record)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::NotAuthorized);
    }

    #[test]
    fn revoke_removes_record() {
        let env = Env::default();
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let record = sample_record(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "revoke_reputation",
                args: (admin.clone(), tenant.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_revoke_reputation(&admin, &tenant)
            .unwrap()
            .unwrap();
        assert!(!client.has_reputation(&tenant));
        assert_eq!(client.get_reputation(&tenant), None);
    }

    #[test]
    fn pause_blocks_update() {
        let env = Env::default();
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let record = sample_record(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_pause(&admin).unwrap().unwrap();
        assert!(client.is_paused());

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_reputation(&operator, &tenant, &record)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::Paused);

        // reads still work
        assert_eq!(client.get_reputation(&tenant), None);
    }

    #[test]
    fn invalid_composite_score_rejected() {
        let env = Env::default();
        let (contract_id, client, _admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let mut record = sample_record(&env);
        record.composite_score = 1001;

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_reputation(&operator, &tenant, &record)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidScore);
    }
}
