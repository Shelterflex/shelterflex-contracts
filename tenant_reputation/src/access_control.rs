use soroban_sdk::{Address, Env};

use crate::ContractError;

#[inline]
pub fn deny(env: &Env, caller: &Address, operation: &str) -> ContractError {
    soroban_access_control::deny(env, caller, operation, ContractError::NotAuthorized)
}

pub fn require_admin_permission(
    env: &Env,
    admin: &Address,
    caller: &Address,
    operation: &str,
) -> Result<(), ContractError> {
    soroban_access_control::require_admin_permission(
        env,
        admin,
        caller,
        operation,
        ContractError::NotAuthorized,
    )
}

pub fn require_admin_or_operator_permission(
    env: &Env,
    admin: &Address,
    operator: &Address,
    caller: &Address,
    operation: &str,
) -> Result<(), ContractError> {
    soroban_access_control::require_admin_or_operator_permission(
        env,
        admin,
        Some(operator),
        caller,
        operation,
        ContractError::NotAuthorized,
    )
}
