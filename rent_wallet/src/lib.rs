#![no_std]

use soroban_pausable_core::{Pausable, PausableError};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
};
use soroban_storage_ttl::TtlStorage;
use soroban_upgrade_governance_core::{
    emergency_upgrade as ug_emergency_upgrade, execute_upgrade as ug_execute_upgrade,
    propose_upgrade as ug_propose_upgrade, set_guardian as ug_set_guardian,
    set_upgrade_delay as ug_set_upgrade_delay, UpgradeGovernanceError, UpgradeGovernanceKey,
};

// ── Storage keys ─────────────────────────────────────────────────────────────
pub mod monthly_cap;
pub mod validation;

#[cfg(kani)]
mod formal_properties;

#[cfg(test)]
mod access_control_tests;

#[cfg(test)]
mod ttl_tests;

#[cfg(test)]
mod monthly_cap_tests;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Per-user balance stored in persistent storage (gas-optimised, #386)
    Balance(Address),
    Paused,
    // ── Monthly spending cap (#1) ──────────────────────────────────────────
    /// Global default cap applied to any user without a per-user override.
    /// Unset (or explicitly `0`) means "no cap" — all debits are allowed.
    MonthlyCapDefault,
    /// Per-user cap that takes precedence over `MonthlyCapDefault` when present.
    /// Per-user data, so this lives in persistent storage (see `Balance(Address)`, #386).
    MonthlyCapOverride(Address),
    /// Cumulative amount debited by `user` during period `key` (see
    /// `monthly_cap::current_month_key`). Persistent, keyed per user per period.
    MonthlySpent(Address, u32),
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    Paused = 3,
    InvalidAmount = 4,
    InsufficientBalance = 5,
    // Upgrade governance errors
    UpgradeAlreadyPending = 6,
    NoUpgradePending = 7,
    UpgradeDelayNotMet = 8,
    /// Amount exceeds the allowed maximum (prevents overflow cascades)
    AmountTooLarge = 9,
    /// Credit would overflow the stored balance (checked arithmetic)
    BalanceOverflow = 17,
    /// Time/lock value exceeds the safe upper bound
    InvalidTimeValue = 10,
    /// String field was empty
    EmptyString = 11,
    /// String field exceeds maximum allowed length
    StringTooLong = 12,
    /// String contains non-printable or disallowed characters
    InvalidStringChar = 13,
    /// Two addresses that must differ were identical
    SameAddress = 14,
    /// Upgrade version must be strictly greater than current version
    InvalidUpgradeVersion = 15,
    /// Stored state schema is incompatible with this contract version
    IncompatibleStateSchema = 16,
    /// Debit would push the user's cumulative spend for the current period
    /// past their effective monthly cap (#1)
    MonthlyCapExceeded = 18,
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct RentWallet;

// ── Internal helpers ──────────────────────────────────────────────────────────

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Address>(&UpgradeGovernanceKey::Admin)
        .expect("admin not set")
}

/// Per-user balance from persistent storage (#386 gas optimisation)
fn get_balance(env: &Env, user: &Address) -> i128 {
    env.get_persistent::<_, i128>(&DataKey::Balance(user.clone()))
        .unwrap_or(0)
}

fn put_balance(env: &Env, user: &Address, amount: i128) {
    env.set_persistent(&DataKey::Balance(user.clone()), &amount);
}

fn get_paused_state(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), ContractError> {
    if get_paused_state(env) {
        return Err(ContractError::Paused);
    }
    Ok(())
}

// ── Contract implementation ───────────────────────────────────────────────────

#[contractimpl]
impl RentWallet {
    pub fn init(env: Env, admin: Address) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        if env.storage().instance().has(&UpgradeGovernanceKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }

        env.storage()
            .instance()
            .set(&UpgradeGovernanceKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&UpgradeGovernanceKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&UpgradeGovernanceKey::StorageSchemaVersion, &1u32);
        env.storage().instance().set(&DataKey::Paused, &false);

        // #389: include version in init event
        env.events().publish(
            (Symbol::new(&env, "rent_wallet"), Symbol::new(&env, "init")),
            (admin, 1u32),
        );

        Ok(())
    }

    pub fn contract_version(env: Env) -> u32 {
        env.extend_instance_ttl();

        env.storage()
            .instance()
            .get::<_, u32>(&UpgradeGovernanceKey::ContractVersion)
            .unwrap_or(0u32)
    }

    /// Current state schema version stored on-chain.
    pub fn state_schema_version(env: Env) -> u32 {
        env.extend_instance_ttl();

        env.storage()
            .instance()
            .get::<_, u32>(&UpgradeGovernanceKey::StorageSchemaVersion)
            .unwrap_or(1u32)
    }

    pub fn version(env: Env) -> u32 {
        env.extend_instance_ttl();

        Self::contract_version(env)
    }

    pub fn credit(
        env: Env,
        admin: Address,
        user: Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "credit",
            ContractError::NotAuthorized,
        )?;
        require_not_paused(&env)?;
        validation::validate_amount(amount)?;

        let cur = get_balance(&env, &user);
        let new_balance = cur
            .checked_add(amount)
            .ok_or(ContractError::BalanceOverflow)?;
        put_balance(&env, &user, new_balance);

        env.events().publish(
            (
                Symbol::new(&env, "rent_wallet"),
                Symbol::new(&env, "credit"),
                user,
            ),
            amount,
        );

        Ok(())
    }

    pub fn debit(
        env: Env,
        admin: Address,
        user: Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "debit",
            ContractError::NotAuthorized,
        )?;
        require_not_paused(&env)?;
        validation::validate_amount(amount)?;

        let cur = get_balance(&env, &user);
        if cur < amount {
            return Err(ContractError::InsufficientBalance);
        }

        // #1: enforce the monthly spending cap. Checked after balance
        // sufficiency (so a debit that would fail anyway doesn't consume
        // cap budget) and before the balance is mutated (so a rejected
        // debit never touches the balance).
        monthly_cap::check_and_record_debit(&env, &user, amount)?;

        let new_balance = cur - amount;
        put_balance(&env, &user, new_balance);

        env.events().publish(
            (
                Symbol::new(&env, "rent_wallet"),
                Symbol::new(&env, "debit"),
                user,
            ),
            amount,
        );

        Ok(())
    }

    pub fn balance(env: Env, user: Address) -> i128 {
        env.extend_instance_ttl();

        get_balance(&env, &user)
    }

    pub fn set_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_admin",
            ContractError::NotAuthorized,
        )?;

        let old_admin = get_admin(&env);
        env.storage()
            .instance()
            .set(&UpgradeGovernanceKey::Admin, &new_admin);

        // #389: include old_admin for full audit trail
        env.events().publish(
            (
                Symbol::new(&env, "rent_wallet"),
                Symbol::new(&env, "set_admin"),
            ),
            (old_admin, new_admin),
        );

        Ok(())
    }
}

#[contractimpl]
impl Pausable for RentWallet {
    fn pause(env: Env, admin: Address) -> Result<(), PausableError> {
        let current_admin = get_admin(&env);
        if soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "pause",
            ContractError::NotAuthorized,
        )
        .is_err()
        {
            return Err(PausableError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        // #389: emit admin address (was `()`)
        env.events().publish(
            (Symbol::new(&env, "Pausable"), Symbol::new(&env, "pause")),
            (),
        );
        Ok(())
    }

    fn unpause(env: Env, admin: Address) -> Result<(), PausableError> {
        let current_admin = get_admin(&env);
        if soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "unpause",
            ContractError::NotAuthorized,
        )
        .is_err()
        {
            return Err(PausableError::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        // #389: emit admin address (was `()`)
        env.events().publish(
            (Symbol::new(&env, "Pausable"), Symbol::new(&env, "unpause")),
            (),
        );
        Ok(())
    }

    fn is_paused(env: Env) -> bool {
        get_paused_state(&env)
    }
}

#[contractimpl]
impl RentWallet {
    // ── Upgrade governance (#392) ─────────────────────────────────────────────

    pub fn set_guardian(env: Env, admin: Address, guardian: Address) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        ug_set_guardian(
            &env,
            &admin,
            Some(guardian),
            Symbol::new(&env, "rent_wallet"),
        )
        .map_err(|_| ContractError::NotAuthorized)
    }

    pub fn set_upgrade_delay(
        env: Env,
        admin: Address,
        delay_seconds: u64,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        ug_set_upgrade_delay(
            &env,
            &admin,
            delay_seconds,
            Symbol::new(&env, "rent_wallet"),
        )
        .map_err(|_| ContractError::NotAuthorized)
    }

    /// Propose a normal upgrade. After the configured delay the upgrade can be executed with `execute_upgrade`.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
        new_version: u32,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        // Custom schema validation for rent_wallet
        let current_version = Self::contract_version(env.clone());
        let schema_version = env
            .storage()
            .instance()
            .get::<_, u32>(&UpgradeGovernanceKey::StorageSchemaVersion)
            .unwrap_or(0u32);

        // Ensure current on-chain state schema matches the currently running contract.
        if schema_version != current_version {
            return Err(ContractError::IncompatibleStateSchema);
        }

        // Use shared upgrade governance module
        ug_propose_upgrade(
            &env,
            &admin,
            &new_wasm_hash,
            new_version,
            Some(schema_version),
            Symbol::new(&env, "rent_wallet"),
        )
        .map_err(|e| match e {
            UpgradeGovernanceError::NotAuthorized => ContractError::NotAuthorized,
            UpgradeGovernanceError::UpgradeAlreadyPending => ContractError::UpgradeAlreadyPending,
            UpgradeGovernanceError::InvalidUpgradeVersion => ContractError::InvalidUpgradeVersion,
            UpgradeGovernanceError::IncompatibleSchemaVersion => {
                ContractError::IncompatibleStateSchema
            }
            _ => ContractError::NotAuthorized,
        })
    }

    /// Execute a previously proposed upgrade. Enforces the timelock delay.
    pub fn execute_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        ug_execute_upgrade(
            &env,
            &admin,
            &new_wasm_hash,
            Symbol::new(&env, "rent_wallet"),
        )
        .map_err(|e| match e {
            UpgradeGovernanceError::NotAuthorized => ContractError::NotAuthorized,
            UpgradeGovernanceError::NoPendingUpgrade => ContractError::NoUpgradePending,
            UpgradeGovernanceError::UpgradeDelayNotElapsed => ContractError::UpgradeDelayNotMet,
            _ => ContractError::NotAuthorized,
        })
    }

    /// Emergency upgrade — bypasses the timelock delay. Requires mandatory guardian authorization.
    pub fn emergency_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
        new_version: u32,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        // Custom schema validation for rent_wallet
        let current_version = Self::contract_version(env.clone());
        let schema_version = env
            .storage()
            .instance()
            .get::<_, u32>(&UpgradeGovernanceKey::StorageSchemaVersion)
            .unwrap_or(0u32);

        // Ensure current on-chain state schema matches the currently running contract.
        if schema_version != current_version {
            return Err(ContractError::IncompatibleStateSchema);
        }

        // Use shared upgrade governance module with mandatory guardian (require_guardian = true)
        ug_emergency_upgrade(
            &env,
            &admin,
            &new_wasm_hash,
            new_version,
            Some(schema_version),
            Symbol::new(&env, "rent_wallet"),
            true, // Mandatory guardian for fund contracts
        )
        .map_err(|e| match e {
            UpgradeGovernanceError::NotAuthorized => ContractError::NotAuthorized,
            UpgradeGovernanceError::GuardianNotConfigured => ContractError::NotAuthorized,
            UpgradeGovernanceError::InvalidUpgradeVersion => ContractError::InvalidUpgradeVersion,
            UpgradeGovernanceError::IncompatibleSchemaVersion => {
                ContractError::IncompatibleStateSchema
            }
            _ => ContractError::NotAuthorized,
        })
    }

    pub fn cancel_upgrade(env: Env, admin: Address) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "cancel_upgrade",
            ContractError::NotAuthorized,
        )?;
        let hash: BytesN<32> = env
            .storage()
            .instance()
            .get(&UpgradeGovernanceKey::PendingUpgradeHash)
            .ok_or(ContractError::NoUpgradePending)?;
        env.storage()
            .instance()
            .remove(&UpgradeGovernanceKey::PendingUpgradeHash);
        env.storage()
            .instance()
            .remove(&UpgradeGovernanceKey::PendingUpgradeAt);
        env.storage()
            .instance()
            .remove(&UpgradeGovernanceKey::PendingUpgradeVersion);
        env.events().publish(
            (
                Symbol::new(&env, "rent_wallet"),
                Symbol::new(&env, "cancel_upgrade"),
            ),
            (admin, hash),
        );
        Ok(())
    }
}

#[contractimpl]
impl RentWallet {
    // ── Monthly spending cap (#1) ─────────────────────────────────────────────

    /// Set the global default monthly spending cap. Applies to every user
    /// who doesn't have a per-user override (`set_user_monthly_cap`).
    /// `0` means no cap — this is also the default before this is ever called.
    pub fn set_default_monthly_cap(
        env: Env,
        admin: Address,
        cap: i128,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_default_monthly_cap",
            ContractError::NotAuthorized,
        )?;
        if cap < 0 {
            return Err(ContractError::InvalidAmount);
        }
        if cap > validation::MAX_AMOUNT {
            return Err(ContractError::AmountTooLarge);
        }

        env.storage()
            .instance()
            .set(&DataKey::MonthlyCapDefault, &cap);
        monthly_cap::emit_monthly_cap_set(&env, None, cap);
        Ok(())
    }

    /// Set a per-user monthly spending cap override, taking precedence over
    /// the global default for this user only. `0` means no cap for this user.
    pub fn set_user_monthly_cap(
        env: Env,
        admin: Address,
        user: Address,
        cap: i128,
    ) -> Result<(), ContractError> {
        env.extend_instance_ttl();

        let current_admin = get_admin(&env);
        soroban_access_control_core::require_admin_permission(
            &env,
            &current_admin,
            &admin,
            "set_user_monthly_cap",
            ContractError::NotAuthorized,
        )?;
        if cap < 0 {
            return Err(ContractError::InvalidAmount);
        }
        if cap > validation::MAX_AMOUNT {
            return Err(ContractError::AmountTooLarge);
        }

        env.set_persistent(&DataKey::MonthlyCapOverride(user.clone()), &cap);
        monthly_cap::emit_monthly_cap_set(&env, Some(user), cap);
        Ok(())
    }

    /// The monthly cap currently in effect for `user` (their override if
    /// set, otherwise the global default). `0` means no cap.
    pub fn get_monthly_cap(env: Env, user: Address) -> i128 {
        env.extend_instance_ttl();

        monthly_cap::effective_cap(&env, &user)
    }

    /// Amount `user` has debited during the current monthly period.
    pub fn get_monthly_spent(env: Env, user: Address) -> i128 {
        env.extend_instance_ttl();

        monthly_cap::get_monthly_spent(&env, &user)
    }
}

#[cfg(test)]
mod test {
    extern crate std;

    use super::{validation, ContractError, DataKey, RentWallet, RentWalletClient};
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::{Address, BytesN, Env, IntoVal};

    fn setup(
        env: &Env,
    ) -> (
        soroban_sdk::Address,
        RentWalletClient<'_>,
        Address,
        Address,
        Address,
    ) {
        let contract_id = env.register(RentWallet, ());

        let client = RentWalletClient::new(env, &contract_id);

        let admin = Address::generate(env);

        let user = Address::generate(env);

        let non_admin = Address::generate(env);

        client.try_init(&admin).unwrap().unwrap();

        (contract_id, client, admin, user, non_admin)
    }

    // ============================================================================
    // Init Tests
    // ============================================================================

    #[test]
    fn init_sets_admin() {
        let env = Env::default();
        let contract_id = env.register(RentWallet, ());
        let client = RentWalletClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        client.try_init(&admin).unwrap().unwrap();

        assert_eq!(client.contract_version(), 1u32);

        // Admin should be able to perform admin operations
        let user = Address::generate(&env);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &100i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 100i128);
    }

    #[test]
    fn version_matches_contract_version() {
        let env = Env::default();
        let contract_id = env.register(RentWallet, ());
        let client = RentWalletClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        client.try_init(&admin).unwrap().unwrap();

        assert_eq!(client.version(), 1u32);
        assert_eq!(client.version(), client.contract_version());
    }

    #[test]
    fn init_initializes_empty_balances() {
        let env = Env::default();
        let contract_id = env.register(RentWallet, ());
        let client = RentWalletClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let user = Address::generate(&env);

        client.try_init(&admin).unwrap().unwrap();

        // Balance should be zero for any user initially
        assert_eq!(client.balance(&user), 0i128);
    }

    #[test]
    fn init_cannot_be_called_twice() {
        let env = Env::default();
        let contract_id = env.register(RentWallet, ());
        let client = RentWalletClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        client.try_init(&admin).unwrap().unwrap();
        let err = client.try_init(&admin).unwrap_err().unwrap();
        assert_eq!(err, ContractError::AlreadyInitialized);
    }

    // ============================================================================
    // Credit Tests
    // ============================================================================

    #[test]
    fn credit_increases_balance() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        assert_eq!(client.balance(&user), 0i128);
        client.try_credit(&admin, &user, &100i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 100i128);
    }

    #[test]
    fn credit_accumulates_balance() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &50i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 50i128);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 75i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &75i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 125i128);
    }

    #[test]
    fn credit_fails_with_zero_amount() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 0i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_credit(&admin, &user, &0i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidAmount);
    }

    #[test]
    fn credit_fails_with_negative_amount() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), -10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_credit(&admin, &user, &-10i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidAmount);
    }

    // ============================================================================
    // Debit Tests
    // ============================================================================

    #[test]
    fn debit_decreases_balance() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // First credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &100i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 100i128);

        // Then debit
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 30i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_debit(&admin, &user, &30i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 70i128);
    }

    #[test]
    fn debit_can_reduce_balance_to_zero() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // Credit balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &50i128).unwrap().unwrap();

        // Debit entire balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_debit(&admin, &user, &50i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 0i128);
    }

    #[test]
    fn debit_fails_with_insufficient_balance() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // Credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 50i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &50i128).unwrap().unwrap();

        // Try to debit more than available
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_debit(&admin, &user, &100i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InsufficientBalance);
    }

    #[test]
    fn debit_fails_when_balance_is_zero() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 1i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_debit(&admin, &user, &1i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InsufficientBalance);
    }

    #[test]
    fn debit_fails_with_zero_amount() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // First credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &100i128).unwrap().unwrap();

        // Try to debit zero
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 0i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_debit(&admin, &user, &0i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidAmount);
    }

    #[test]
    fn debit_fails_with_negative_amount() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // First credit some balance
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &100i128).unwrap().unwrap();

        // Try to debit negative amount
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), -10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_debit(&admin, &user, &-10i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InvalidAmount);
    }

    // ============================================================================
    // Balance Tests
    // ============================================================================

    #[test]
    fn balance_returns_zero_for_new_user() {
        let env = Env::default();
        let (_contract_id, client, _admin, user, _non_admin) = setup(&env);
        let new_user = Address::generate(&env);

        assert_eq!(client.balance(&user), 0i128);
        assert_eq!(client.balance(&new_user), 0i128);
    }

    #[test]
    fn balance_reflects_credit_and_debit_operations() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        // Initial balance
        assert_eq!(client.balance(&user), 0i128);

        // After credit
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 200i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(&admin, &user, &200i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 200i128);

        // After debit
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 80i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_debit(&admin, &user, &80i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 120i128);
    }

    // ============================================================================
    // Balance Invariant Tests
    // ============================================================================

    #[test]
    fn invariant_balance_never_negative_after_failed_debit() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 1i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let err = client
            .try_debit(&admin, &user, &1i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InsufficientBalance);
        assert!(client.balance(&user) >= 0i128);
    }

    // ============================================================================
    // Pause Tests
    // ============================================================================

    #[test]
    fn admin_can_pause_and_unpause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);

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
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unpause",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_unpause(&admin).unwrap().unwrap();
        assert!(!client.is_paused());
    }

    #[test]
    fn paused_contract_blocks_credit_and_debit() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

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

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_credit(&admin, &user, &10i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::Paused);
    }

    #[test]
    fn non_admin_cannot_pause() {
        let env = Env::default();
        let (contract_id, client, _admin, _user, non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "pause",
                args: (non_admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client.try_pause(&non_admin).unwrap_err().unwrap();
        assert_eq!(err, soroban_pausable_core::PausableError::NotAuthorized);
    }

    #[test]
    fn non_admin_cannot_unpause() {
        let env = Env::default();
        let (contract_id, client, admin, _user, non_admin) = setup(&env);

        // First pause as admin
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

        // Try to unpause as non-admin
        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "unpause",
                args: (non_admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client.try_unpause(&non_admin).unwrap_err().unwrap();
        assert_eq!(err, soroban_pausable_core::PausableError::NotAuthorized);
    }

    #[test]
    fn credit_fails_when_paused() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

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

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 10i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_credit(&admin, &user, &10i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::Paused);
    }

    // ============================================================================
    // Upgrade Governance Tests (#392)
    // ============================================================================

    #[test]
    fn propose_upgrade_stores_pending_hash() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn propose_upgrade_fails_if_already_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::UpgradeAlreadyPending);
    }

    #[test]
    fn cancel_upgrade_clears_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "cancel_upgrade",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_cancel_upgrade(&admin).unwrap().unwrap();

        // Can propose again after cancellation
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn execute_upgrade_fails_when_delay_not_met() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        // Set a 1-hour delay
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_upgrade_delay",
                args: (admin.clone(), 3600u64).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_set_upgrade_delay(&admin, &3600u64)
            .unwrap()
            .unwrap();

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();

        // Execute immediately — should fail because delay not met
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "execute_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_execute_upgrade(&admin, &hash)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::UpgradeDelayNotMet);
    }

    #[test]
    fn execute_upgrade_fails_with_no_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "execute_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_execute_upgrade(&admin, &hash)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::NoUpgradePending);
    }

    #[test]
    fn cancel_upgrade_fails_with_no_pending() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "cancel_upgrade",
                args: (admin.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client.try_cancel_upgrade(&admin).unwrap_err().unwrap();
        assert_eq!(err, ContractError::NoUpgradePending);
    }

    #[test]
    fn failed_execute_upgrade_does_not_clear_pending_upgrade() {
        let env = Env::default();
        let (contract_id, client, admin, _user, _non_admin) = setup(&env);
        let hash = BytesN::from_array(&env, &[0u8; 32]);

        // Propose an upgrade to version 2.
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap()
            .unwrap();

        // Break schema compatibility to force execute_upgrade to fail.
        env.as_contract(&contract_id, || {
            env.storage().instance().set(
                &soroban_upgrade_governance_core::UpgradeGovernanceKey::StorageSchemaVersion,
                &0u32,
            );
        });

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "execute_upgrade",
                args: (admin.clone(), hash.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let err = client
            .try_execute_upgrade(&admin, &hash)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::IncompatibleStateSchema);

        // Pending proposal should still exist; re-proposing should fail.
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "propose_upgrade",
                args: (admin.clone(), hash.clone(), 2u32).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let err2 = client
            .try_propose_upgrade(&admin, &hash, &2u32)
            .unwrap_err()
            .unwrap();
        assert_eq!(err2, ContractError::UpgradeAlreadyPending);
    }

    // ============================================================================
    // Arithmetic safety & property invariants (#1141)
    // ============================================================================

    fn mock_admin_credit(
        env: &Env,
        contract_id: &Address,
        client: &RentWalletClient,
        admin: &Address,
        user: &Address,
        amount: i128,
    ) {
        env.mock_auths(&[MockAuth {
            address: admin,
            invoke: &MockAuthInvoke {
                contract: contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), amount).into_val(env),
                sub_invokes: &[],
            },
        }]);
        client.try_credit(admin, user, &amount).unwrap().unwrap();
    }

    #[test]
    fn credit_overflow_returns_error_without_mutating_balance() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Balance(user.clone()), &(i128::MAX - 1));
        });

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 2i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_credit(&admin, &user, &2i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::BalanceOverflow);
        assert_eq!(client.balance(&user), i128::MAX - 1);
    }

    #[test]
    fn credit_at_max_amount_boundary_succeeds_from_zero() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);
        mock_admin_credit(
            &env,
            &contract_id,
            &client,
            &admin,
            &user,
            validation::MAX_AMOUNT,
        );
        assert_eq!(client.balance(&user), validation::MAX_AMOUNT);
    }

    #[test]
    fn debit_exact_balance_succeeds_and_debit_over_balance_is_rejected() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);
        mock_admin_credit(&env, &contract_id, &client, &admin, &user, 75i128);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 75i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_debit(&admin, &user, &75i128).unwrap().unwrap();
        assert_eq!(client.balance(&user), 0i128);

        let before = client.balance(&user);
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 1i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let err = client
            .try_debit(&admin, &user, &1i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, ContractError::InsufficientBalance);
        assert_eq!(client.balance(&user), before);
    }

    #[test]
    fn rejected_credit_and_debit_leave_balance_unchanged() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);
        mock_admin_credit(&env, &contract_id, &client, &admin, &user, 40i128);
        let before = client.balance(&user);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (admin.clone(), user.clone(), 0i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert_eq!(
            client
                .try_credit(&admin, &user, &0i128)
                .unwrap_err()
                .unwrap(),
            ContractError::InvalidAmount
        );
        assert_eq!(client.balance(&user), before);

        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (admin.clone(), user.clone(), 100i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert_eq!(
            client
                .try_debit(&admin, &user, &100i128)
                .unwrap_err()
                .unwrap(),
            ContractError::InsufficientBalance
        );
        assert_eq!(client.balance(&user), before);
    }

    #[test]
    fn non_admin_cannot_credit_or_debit() {
        let env = Env::default();
        let (contract_id, client, admin, user, non_admin) = setup(&env);
        mock_admin_credit(&env, &contract_id, &client, &admin, &user, 10i128);

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "credit",
                args: (non_admin.clone(), user.clone(), 5i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert_eq!(
            client
                .try_credit(&non_admin, &user, &5i128)
                .unwrap_err()
                .unwrap(),
            ContractError::NotAuthorized
        );

        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "debit",
                args: (non_admin.clone(), user.clone(), 1i128).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert_eq!(
            client
                .try_debit(&non_admin, &user, &1i128)
                .unwrap_err()
                .unwrap(),
            ContractError::NotAuthorized
        );
        assert_eq!(client.balance(&user), 10i128);
    }

    #[test]
    fn property_randomized_credit_debit_sequence_preserves_balance_invariant() {
        let env = Env::default();
        let (contract_id, client, admin, user, _non_admin) = setup(&env);

        let mut rng: u64 = 0xC0FFEE_u64;
        let mut net_applied: i128 = 0;
        const OPS: u32 = 200;

        for step in 0..OPS {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let roll = (rng % 100) as u32;
            let amount = ((rng % 50) + 1) as i128;

            if roll < 45 {
                env.mock_auths(&[MockAuth {
                    address: &admin,
                    invoke: &MockAuthInvoke {
                        contract: &contract_id,
                        fn_name: "credit",
                        args: (admin.clone(), user.clone(), amount).into_val(&env),
                        sub_invokes: &[],
                    },
                }]);
                if matches!(client.try_credit(&admin, &user, &amount), Ok(Ok(()))) {
                    net_applied += amount;
                }
            } else {
                env.mock_auths(&[MockAuth {
                    address: &admin,
                    invoke: &MockAuthInvoke {
                        contract: &contract_id,
                        fn_name: "debit",
                        args: (admin.clone(), user.clone(), amount).into_val(&env),
                        sub_invokes: &[],
                    },
                }]);
                if matches!(client.try_debit(&admin, &user, &amount), Ok(Ok(()))) {
                    net_applied -= amount;
                }
            }

            let balance = client.balance(&user);
            assert!(balance >= 0, "negative balance at step {}", step);
            assert_eq!(balance, net_applied, "balance drift at step {}", step);
        }
    }
}
