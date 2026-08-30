#![no_std]
use soroban_sdk::{contracttype, Address, BytesN, Env, Symbol};

/// Storage keys for upgrade governance state.
#[contracttype]
pub enum UpgradeGovernanceKey {
    /// Admin address authorized to initiate upgrades
    Admin,
    /// Guardian address required for emergency upgrades (mandatory for fund contracts)
    Guardian,
    /// Proposed upgrade WASM hash
    PendingUpgradeHash,
    /// Timestamp when upgrade was proposed (for normal upgrades)
    PendingUpgradeAt,
    /// Proposed upgrade version (for schema versioning)
    PendingUpgradeVersion,
    /// Current contract version
    ContractVersion,
    /// Delay in seconds before normal upgrade can be executed
    UpgradeDelay,
    /// Current storage schema version
    StorageSchemaVersion,
}

/// Errors for upgrade governance operations.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum UpgradeGovernanceError {
    /// Unauthorized caller
    NotAuthorized = 1,
    /// Upgrade already pending
    UpgradeAlreadyPending = 2,
    /// No pending upgrade to execute
    NoPendingUpgrade = 3,
    /// Upgrade delay not elapsed
    UpgradeDelayNotElapsed = 4,
    /// Guardian not configured (required for emergency upgrades)
    GuardianNotConfigured = 5,
    /// Invalid upgrade version (must be sequential)
    InvalidUpgradeVersion = 6,
    /// Incompatible schema version
    IncompatibleSchemaVersion = 7,
}

impl From<UpgradeGovernanceError> for soroban_sdk::Error {
    fn from(e: UpgradeGovernanceError) -> Self {
        soroban_sdk::Error::from_contract_error(e as u32)
    }
}

/// Get the admin address from storage.
pub fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&UpgradeGovernanceKey::Admin)
        .expect("Admin must be initialized")
}

/// Get the guardian address from storage (returns None if not set).
pub fn get_guardian(env: &Env) -> Option<Address> {
    env.storage()
        .instance()
        .get(&UpgradeGovernanceKey::Guardian)
}

/// Get the current contract version from storage.
pub fn get_contract_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&UpgradeGovernanceKey::ContractVersion)
        .unwrap_or(0)
}

/// Get the current storage schema version from storage.
pub fn get_storage_schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&UpgradeGovernanceKey::StorageSchemaVersion)
        .unwrap_or(1)
}

/// Validate upgrade safety: ensures version is sequential and schema is compatible.
///
/// This function enforces:
/// - New version must be exactly current version + 1 (sequential upgrades only)
/// - Schema version compatibility (can be customized per contract)
pub fn validate_upgrade_safety(
    env: &Env,
    new_version: u32,
    new_schema_version: Option<u32>,
) -> Result<(), UpgradeGovernanceError> {
    let current_version = get_contract_version(env);

    // Enforce sequential versioning
    if new_version != current_version + 1 {
        return Err(UpgradeGovernanceError::InvalidUpgradeVersion);
    }

    // If schema version is provided, validate compatibility
    if let Some(new_schema) = new_schema_version {
        let current_schema = get_storage_schema_version(env);
        // Schema must be sequential or compatible (customize per contract needs)
        if new_schema < current_schema {
            return Err(UpgradeGovernanceError::IncompatibleSchemaVersion);
        }
    }

    Ok(())
}

/// Propose a normal upgrade with enforced timelock delay.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address proposing the upgrade
/// * `new_wasm_hash` - The hash of the new WASM to deploy
/// * `new_version` - The new contract version
/// * `new_schema_version` - Optional new schema version for validation
/// * `contract_name` - The contract name for event emission
///
/// # Errors
/// * `NotAuthorized` - If caller is not admin
/// * `UpgradeAlreadyPending` - If an upgrade is already pending
/// * `InvalidUpgradeVersion` - If version is not sequential
/// * `IncompatibleSchemaVersion` - If schema version is incompatible
pub fn propose_upgrade(
    env: &Env,
    admin: &Address,
    new_wasm_hash: &BytesN<32>,
    new_version: u32,
    new_schema_version: Option<u32>,
    contract_name: Symbol,
) -> Result<(), UpgradeGovernanceError> {
    let current_admin = get_admin(env);

    // Require admin authorization
    if admin != &current_admin {
        return Err(UpgradeGovernanceError::NotAuthorized);
    }
    admin.require_auth();

    // Check for existing pending upgrade
    if env
        .storage()
        .instance()
        .has(&UpgradeGovernanceKey::PendingUpgradeHash)
    {
        return Err(UpgradeGovernanceError::UpgradeAlreadyPending);
    }

    // Validate upgrade safety
    validate_upgrade_safety(env, new_version, new_schema_version)?;

    let now = env.ledger().timestamp();
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::PendingUpgradeHash, new_wasm_hash);
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::PendingUpgradeAt, &now);
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::PendingUpgradeVersion, &new_version);

    // Emit propose_upgrade event
    env.events().publish(
        (contract_name, Symbol::new(env, "propose_upgrade")),
        (new_wasm_hash.clone(), new_version, now),
    );

    Ok(())
}

/// Execute a proposed normal upgrade after the timelock delay has elapsed.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address executing the upgrade
/// * `new_wasm_hash` - The hash of the new WASM to deploy (must match pending)
/// * `contract_name` - The contract name for event emission
///
/// # Errors
/// * `NotAuthorized` - If caller is not admin
/// * `NoPendingUpgrade` - If no upgrade is pending
/// * `UpgradeDelayNotElapsed` - If delay has not elapsed
pub fn execute_upgrade(
    env: &Env,
    admin: &Address,
    new_wasm_hash: &BytesN<32>,
    contract_name: Symbol,
) -> Result<(), UpgradeGovernanceError> {
    let current_admin = get_admin(env);

    // Require admin authorization
    if admin != &current_admin {
        return Err(UpgradeGovernanceError::NotAuthorized);
    }
    admin.require_auth();

    // Get pending upgrade details
    let pending_hash: BytesN<32> = env
        .storage()
        .instance()
        .get(&UpgradeGovernanceKey::PendingUpgradeHash)
        .ok_or(UpgradeGovernanceError::NoPendingUpgrade)?;
    let pending_at: u64 = env
        .storage()
        .instance()
        .get(&UpgradeGovernanceKey::PendingUpgradeAt)
        .ok_or(UpgradeGovernanceError::NoPendingUpgrade)?;
    let pending_version: u32 = env
        .storage()
        .instance()
        .get(&UpgradeGovernanceKey::PendingUpgradeVersion)
        .ok_or(UpgradeGovernanceError::NoPendingUpgrade)?;

    // Verify the provided hash matches pending
    if pending_hash != *new_wasm_hash {
        return Err(UpgradeGovernanceError::NoPendingUpgrade);
    }

    // Check delay has elapsed
    let delay: u64 = env
        .storage()
        .instance()
        .get(&UpgradeGovernanceKey::UpgradeDelay)
        .unwrap_or(0);
    let now = env.ledger().timestamp();
    if now < pending_at + delay {
        return Err(UpgradeGovernanceError::UpgradeDelayNotElapsed);
    }

    // Clear pending upgrade
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeHash);
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeAt);
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeVersion);

    // Update contract version
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::ContractVersion, &pending_version);

    // Emit execute_upgrade event
    env.events().publish(
        (contract_name, Symbol::new(env, "execute_upgrade")),
        (admin, new_wasm_hash.clone(), pending_version, now),
    );

    // Deploy the new WASM
    env.deployer()
        .update_current_contract_wasm(new_wasm_hash.clone());

    Ok(())
}

/// Execute an emergency upgrade requiring mandatory guardian authorization.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address initiating the emergency upgrade
/// * `new_wasm_hash` - The hash of the new WASM to deploy
/// * `new_version` - The new contract version
/// * `new_schema_version` - Optional new schema version for validation
/// * `contract_name` - The contract name for event emission
/// * `require_guardian` - If true, guardian must be configured and must authorize
///
/// # Errors
/// * `NotAuthorized` - If caller is not admin
/// * `GuardianNotConfigured` - If guardian is required but not configured
/// * `InvalidUpgradeVersion` - If version is not sequential
/// * `IncompatibleSchemaVersion` - If schema version is incompatible
pub fn emergency_upgrade(
    env: &Env,
    admin: &Address,
    new_wasm_hash: &BytesN<32>,
    new_version: u32,
    new_schema_version: Option<u32>,
    contract_name: Symbol,
    require_guardian: bool,
) -> Result<(), UpgradeGovernanceError> {
    let current_admin = get_admin(env);

    // Require admin authorization
    if admin != &current_admin {
        return Err(UpgradeGovernanceError::NotAuthorized);
    }
    admin.require_auth();

    // Validate upgrade safety
    validate_upgrade_safety(env, new_version, new_schema_version)?;

    // Require guardian authorization if mandated
    if require_guardian {
        let guardian = get_guardian(env).ok_or(UpgradeGovernanceError::GuardianNotConfigured)?;
        guardian.require_auth();
    } else if let Some(guardian) = get_guardian(env) {
        // Optional guardian: if configured, require authorization
        guardian.require_auth();
    }

    // Clear any pending upgrade
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeHash);
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeAt);
    env.storage()
        .instance()
        .remove(&UpgradeGovernanceKey::PendingUpgradeVersion);

    // Update contract version
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::ContractVersion, &new_version);

    // Emit emergency_upgrade event
    env.events().publish(
        (contract_name, Symbol::new(env, "emergency_upgrade")),
        (
            admin,
            new_wasm_hash.clone(),
            new_version,
            env.ledger().timestamp(),
        ),
    );

    // Deploy the new WASM
    env.deployer()
        .update_current_contract_wasm(new_wasm_hash.clone());

    Ok(())
}

/// Set the upgrade delay for normal upgrades.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address setting the delay
/// * `delay_seconds` - The delay in seconds
/// * `contract_name` - The contract name for event emission
///
/// # Errors
/// * `NotAuthorized` - If caller is not admin
pub fn set_upgrade_delay(
    env: &Env,
    admin: &Address,
    delay_seconds: u64,
    contract_name: Symbol,
) -> Result<(), UpgradeGovernanceError> {
    let current_admin = get_admin(env);

    // Require admin authorization
    if admin != &current_admin {
        admin.require_auth();
        return Err(UpgradeGovernanceError::NotAuthorized);
    }
    admin.require_auth();

    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::UpgradeDelay, &delay_seconds);

    // Emit set_upgrade_delay event
    env.events().publish(
        (contract_name, Symbol::new(env, "set_upgrade_delay")),
        delay_seconds,
    );

    Ok(())
}

/// Set the guardian address for emergency upgrades.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address setting the guardian
/// * `guardian` - The guardian address (None to remove)
/// * `contract_name` - The contract name for event emission
///
/// # Errors
/// * `NotAuthorized` - If caller is not admin
pub fn set_guardian(
    env: &Env,
    admin: &Address,
    guardian: Option<Address>,
    contract_name: Symbol,
) -> Result<(), UpgradeGovernanceError> {
    let current_admin = get_admin(env);

    // Require admin authorization
    if admin != &current_admin {
        admin.require_auth();
        return Err(UpgradeGovernanceError::NotAuthorized);
    }
    admin.require_auth();

    match guardian {
        Some(g) => {
            env.storage()
                .instance()
                .set(&UpgradeGovernanceKey::Guardian, &g);
            // Emit set_guardian event with the guardian address
            env.events()
                .publish((contract_name, Symbol::new(env, "set_guardian")), &g);
        }
        None => {
            env.storage()
                .instance()
                .remove(&UpgradeGovernanceKey::Guardian);
            // Emit set_guardian event indicating guardian removed
            env.events()
                .publish((contract_name, Symbol::new(env, "set_guardian")), ());
        }
    }

    Ok(())
}

/// Initialize upgrade governance state.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `admin` - The admin address
/// * `guardian` - Optional guardian address
/// * `initial_version` - Initial contract version
/// * `initial_schema_version` - Initial storage schema version
/// * `upgrade_delay` - Initial upgrade delay in seconds
pub fn initialize_upgrade_governance(
    env: &Env,
    admin: &Address,
    guardian: Option<Address>,
    initial_version: u32,
    initial_schema_version: u32,
    upgrade_delay: u64,
) {
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::Admin, admin);
    if let Some(g) = guardian {
        env.storage()
            .instance()
            .set(&UpgradeGovernanceKey::Guardian, &g);
    }
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::ContractVersion, &initial_version);
    env.storage().instance().set(
        &UpgradeGovernanceKey::StorageSchemaVersion,
        &initial_schema_version,
    );
    env.storage()
        .instance()
        .set(&UpgradeGovernanceKey::UpgradeDelay, &upgrade_delay);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upgrade_governance_error_from_u32() {
        // Test that error conversion works
        let error = UpgradeGovernanceError::NotAuthorized;
        assert_eq!(error as u32, 1);

        let error = UpgradeGovernanceError::UpgradeAlreadyPending;
        assert_eq!(error as u32, 2);

        let error = UpgradeGovernanceError::NoPendingUpgrade;
        assert_eq!(error as u32, 3);

        let error = UpgradeGovernanceError::UpgradeDelayNotElapsed;
        assert_eq!(error as u32, 4);

        let error = UpgradeGovernanceError::GuardianNotConfigured;
        assert_eq!(error as u32, 5);

        let error = UpgradeGovernanceError::InvalidUpgradeVersion;
        assert_eq!(error as u32, 6);

        let error = UpgradeGovernanceError::IncompatibleSchemaVersion;
        assert_eq!(error as u32, 7);
    }
}
