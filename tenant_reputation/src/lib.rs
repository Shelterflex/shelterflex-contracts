#![no_std]

use soroban_pausable::{Pausable, PausableError};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

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
    // ── Decay & bounds config (Issue #1132) ──────────────────────────────────
    /// Score units to subtract per decay period (0 = no decay)
    DecayRatePerPeriod,
    /// Duration of each decay period in seconds
    DecayPeriodSecs,
    /// Minimum allowed composite score (inclusive)
    ScoreMin,
    /// Maximum allowed composite score (inclusive)
    ScoreMax,
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

fn score_max(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get::<_, u32>(&DataKey::ScoreMax)
        .unwrap_or(1000)
}

fn score_min(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get::<_, u32>(&DataKey::ScoreMin)
        .unwrap_or(0)
}

fn clamp_score(env: &Env, score: u32) -> u32 {
    let lo = score_min(env);
    let hi = score_max(env);
    score.max(lo).min(hi)
}

/// Compute the decayed composite score without writing to storage.
/// Returns `(decayed_score, did_decay)`.
fn compute_decayed_score(env: &Env, record: &ReputationRecord) -> (u32, bool) {
    let rate: u32 = env
        .storage()
        .instance()
        .get::<_, u32>(&DataKey::DecayRatePerPeriod)
        .unwrap_or(0);
    let period: u64 = env
        .storage()
        .instance()
        .get::<_, u64>(&DataKey::DecayPeriodSecs)
        .unwrap_or(86400);

    if rate == 0 || period == 0 {
        return (record.composite_score, false);
    }

    let now = env.ledger().timestamp();
    if now <= record.last_updated {
        return (record.composite_score, false);
    }

    let elapsed = now - record.last_updated;
    let periods = (elapsed / period) as u32;
    if periods == 0 {
        return (record.composite_score, false);
    }

    let decay_amount = periods.saturating_mul(rate);
    let lo = score_min(env);
    let new_score = record.composite_score.saturating_sub(decay_amount).max(lo);

    (new_score, new_score != record.composite_score)
}

fn emit_updated(env: &Env, tenant: &Address, record: &ReputationRecord, reason: &Symbol) {
    env.events().publish(
        (
            Symbol::new(env, "tenant_reputation"),
            Symbol::new(env, "reputation_updated"),
            tenant.clone(),
        ),
        (
            record.composite_score,
            record.total_ratings,
            record.last_updated,
            reason.clone(),
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
        // Default decay config: no decay, bounds [0, 1000]
        env.storage()
            .instance()
            .set(&DataKey::DecayRatePerPeriod, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::DecayPeriodSecs, &86400u64);
        env.storage().instance().set(&DataKey::ScoreMin, &0u32);
        env.storage().instance().set(&DataKey::ScoreMax, &1000u32);
        Ok(())
    }

    /// Admin sets the linear decay rate: `decay_rate_per_period` score units per `period_secs`.
    /// Set `decay_rate_per_period = 0` to disable decay.
    pub fn set_decay_config(
        env: Env,
        admin: Address,
        decay_rate_per_period: u32,
        period_secs: u64,
    ) -> Result<(), ContractError> {
        access_control::require_admin_permission(
            &env,
            &get_admin(&env),
            &admin,
            "set_decay_config",
        )?;
        env.storage()
            .instance()
            .set(&DataKey::DecayRatePerPeriod, &decay_rate_per_period);
        env.storage()
            .instance()
            .set(&DataKey::DecayPeriodSecs, &period_secs);
        env.events().publish(
            (
                Symbol::new(&env, "tenant_reputation"),
                Symbol::new(&env, "decay_config_updated"),
            ),
            (decay_rate_per_period, period_secs),
        );
        Ok(())
    }

    /// Admin sets the min/max composite score bounds; future updates are clamped to these.
    pub fn set_score_bounds(
        env: Env,
        admin: Address,
        score_min: u32,
        score_max: u32,
    ) -> Result<(), ContractError> {
        access_control::require_admin_permission(
            &env,
            &get_admin(&env),
            &admin,
            "set_score_bounds",
        )?;
        env.storage().instance().set(&DataKey::ScoreMin, &score_min);
        env.storage().instance().set(&DataKey::ScoreMax, &score_max);
        env.events().publish(
            (
                Symbol::new(&env, "tenant_reputation"),
                Symbol::new(&env, "score_bounds_updated"),
            ),
            (score_min, score_max),
        );
        Ok(())
    }

    pub fn update_reputation(
        env: Env,
        caller: Address,
        tenant: Address,
        record: ReputationRecord,
        reason: Symbol,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;
        access_control::require_admin_or_operator_permission(
            &env,
            &get_admin(&env),
            &get_operator(&env),
            &caller,
            "update_reputation",
        )?;

        let clamped_score = clamp_score(&env, record.composite_score);
        let updated = ReputationRecord {
            composite_score: clamped_score,
            last_updated: env.ledger().timestamp(),
            ..record
        };
        env.storage()
            .persistent()
            .set(&DataKey::Reputation(tenant.clone()), &updated);
        emit_updated(&env, &tenant, &updated, &reason);
        Ok(())
    }

    /// Returns the reputation record with lazy decay applied.
    /// Decay is computed from elapsed ledger time since `last_updated`; the stored
    /// record is not written — call `update_reputation` to persist the new score.
    pub fn get_reputation(env: Env, tenant: Address) -> Option<ReputationRecord> {
        let record: ReputationRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Reputation(tenant.clone()))?;

        let (decayed_score, did_decay) = compute_decayed_score(&env, &record);
        if did_decay {
            env.events().publish(
                (
                    Symbol::new(&env, "tenant_reputation"),
                    Symbol::new(&env, "reputation_decayed"),
                    tenant,
                ),
                (record.composite_score, decayed_score),
            );
            Some(ReputationRecord {
                composite_score: decayed_score,
                ..record
            })
        } else {
            Some(record)
        }
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
                    Symbol::new(&env, "tenant_reputation"),
                    Symbol::new(&env, "revoked"),
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
            (Symbol::new(&env, "Pausable"), Symbol::new(&env, "pause")),
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
    use soroban_sdk::{Address, Env, IntoVal, Symbol};

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

    fn reason(env: &Env) -> Symbol {
        Symbol::new(env, "test_update")
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
        let r = reason(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
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
                args: (admin.clone(), tenant.clone(), updated.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&admin, &tenant, &updated, &r)
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
        let r = reason(&env);

        env.mock_auths(&[MockAuth {
            address: &stranger,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (stranger.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_reputation(&stranger, &tenant, &record, &r)
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
        let r = reason(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
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
        let r = reason(&env);

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
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::Paused);

        // reads still work
        assert_eq!(client.get_reputation(&tenant), None);
    }

    #[test]
    fn score_clamped_to_max() {
        let env = Env::default();
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let r = reason(&env);

        // max is 1000 by default; submitting 1500 should be clamped to 1000
        let mut record = sample_record(&env);
        record.composite_score = 1500;

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();

        let stored = client.get_reputation(&tenant).unwrap();
        assert_eq!(stored.composite_score, 1000);

        // Admin raises max to 1200; now 1500 should clamp to 1200
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_score_bounds",
                args: (admin.clone(), 0u32, 1200u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_score_bounds(&admin, &0u32, &1200u32)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();
        assert_eq!(
            client.get_reputation(&tenant).unwrap().composite_score,
            1200
        );
    }

    #[test]
    fn score_clamped_to_min() {
        let env = Env::default();
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let r = reason(&env);

        // Set min to 100
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_score_bounds",
                args: (admin.clone(), 100u32, 1000u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_score_bounds(&admin, &100u32, &1000u32)
            .unwrap()
            .unwrap();

        let mut record = sample_record(&env);
        record.composite_score = 50; // below min

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();

        assert_eq!(client.get_reputation(&tenant).unwrap().composite_score, 100);
    }

    #[test]
    fn decay_reduces_score_over_time() {
        let env = Env::default();
        env.ledger().set_timestamp(1000);
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let r = reason(&env);

        // decay 10 per day (86400 s)
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_decay_config",
                args: (admin.clone(), 10u32, 86400u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_decay_config(&admin, &10u32, &86400u64)
            .unwrap()
            .unwrap();

        let record = sample_record(&env); // composite_score = 750

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();

        // 3 days later → decay 30
        env.ledger().set_timestamp(1000 + 3 * 86400);
        let got = client.get_reputation(&tenant).unwrap();
        assert_eq!(got.composite_score, 720); // 750 - 30
    }

    #[test]
    fn no_decay_without_elapsed_time() {
        let env = Env::default();
        env.ledger().set_timestamp(5000);
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let r = reason(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_decay_config",
                args: (admin.clone(), 50u32, 86400u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_decay_config(&admin, &50u32, &86400u64)
            .unwrap()
            .unwrap();

        let record = sample_record(&env);
        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();

        // Same timestamp — no decay
        let got = client.get_reputation(&tenant).unwrap();
        assert_eq!(got.composite_score, 750);
    }

    #[test]
    fn decay_clamped_at_score_min() {
        let env = Env::default();
        env.ledger().set_timestamp(0);
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let r = reason(&env);

        // Very high decay rate
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_decay_config",
                args: (admin.clone(), 500u32, 86400u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_decay_config(&admin, &500u32, &86400u64)
            .unwrap()
            .unwrap();

        let mut record = sample_record(&env);
        record.composite_score = 100;
        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();

        // 10 days later → would decay 5000, but clamped to score_min (0)
        env.ledger().set_timestamp(10 * 86400);
        let got = client.get_reputation(&tenant).unwrap();
        assert_eq!(got.composite_score, 0);
    }

    #[test]
    fn revoke_resets_reputation() {
        let env = Env::default();
        let (contract_id, client, admin, operator) = setup(&env);
        let tenant = Address::generate(&env);
        let record = sample_record(&env);
        let r = reason(&env);

        env.mock_auths(&[MockAuth {
            address: &operator,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "update_reputation",
                args: (operator.clone(), tenant.clone(), record.clone(), r.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_update_reputation(&operator, &tenant, &record, &r)
            .unwrap()
            .unwrap();
        assert!(client.has_reputation(&tenant));

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

        // After revoke, reputation should be None regardless of prior score
        assert!(!client.has_reputation(&tenant));
        assert_eq!(client.get_reputation(&tenant), None);
    }
}
