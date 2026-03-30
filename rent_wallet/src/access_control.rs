//! Access-control helpers for the `rent_wallet` contract.
//!
//! This module standardises role-based permission checking for all public
//! functions in `rent_wallet`.  It maps directly to the permissions defined
//! in the `contract_access` library.
//!
//! ## Role → Permission mapping (rent_wallet)
//! | Function        | Required caller | Permission        |
//! |-----------------|-----------------|-------------------|
//! | init            | none (unchecked)| Initialize        |
//! | credit          | admin           | CreditFunds       |
//! | debit           | admin           | DebitFunds        |
//! | set_admin       | admin           | TransferAdmin     |
//! | pause / unpause | admin           | PauseContract     |
//! | propose_upgrade | admin           | UpgradeContract   |
//! | execute_upgrade | admin           | UpgradeContract   |
//!
//! ## Event topics
//! All authorization failures emit an `access_control::unauthorized` event
//! before returning `Err(ContractError::NotAuthorized)`.
//!
//! ## Closes: #383 (access-control integration for rent_wallet)

use soroban_sdk::{Address, Env, Symbol};

use crate::ContractError;

/// Emit a standardized unauthorized-access event and return the error.
///
/// Callers use this helper instead of returning the error directly so that
/// off-chain indexers can detect and alert on unauthorized calls.
#[inline]
pub fn deny(env: &Env, caller: &Address, operation: &str) -> ContractError {
    env.events().publish(
        (
            Symbol::new(env, "access_control"),
            Symbol::new(env, "unauthorized"),
            caller.clone(),
        ),
        Symbol::new(env, operation),
    );
    ContractError::NotAuthorized
}

/// Require that `caller` is the current `admin`.
///
/// Emits a standardized `access_control::unauthorized` event on failure.
pub fn require_admin_permission(
    env: &Env,
    admin: &Address,
    caller: &Address,
    operation: &str,
) -> Result<(), ContractError> {
    caller.require_auth();
    if caller != admin {
        return Err(deny(env, caller, operation));
    }
    Ok(())
}

/// Require that `caller` is either the `admin` OR an optional `operator`.
///
/// Used for operations where both roles share the same permission.
pub fn require_admin_or_operator_permission(
    env: &Env,
    admin: &Address,
    operator: Option<&Address>,
    caller: &Address,
    operation: &str,
) -> Result<(), ContractError> {
    caller.require_auth();
    if caller == admin {
        return Ok(());
    }
    if let Some(op) = operator {
        if caller == op {
            return Ok(());
        }
    }
    Err(deny(env, caller, operation))
}
