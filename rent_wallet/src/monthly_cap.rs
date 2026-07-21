//! Monthly spending cap module for `RentWallet` (#1).
//!
//! Enforces an optional per-user monthly spending cap on `debit()`, the
//! contract's only outbound-value path. A cap consists of a global default
//! (`MonthlyCapDefault`) that applies to every user, plus an optional
//! per-user override (`MonthlyCapOverride`) that takes precedence when set.
//!
//! A cap value of `0` — whether from an unset default or an explicit
//! override — means "no cap": all debits are allowed. This mirrors the
//! lazy-default-on-read convention used elsewhere in this contract
//! (`get_paused_state`, `get_state_schema_version`), so pre-existing wallets
//! from before this module was wired in are unaffected until an admin
//! explicitly opts them into a cap.
//!
//! Month key formula: `ledger_timestamp / 2_592_000` gives an approximate
//! 30-day month number. This is a documented approximation — actual calendar
//! months vary in length, but the 30-day window is consistent and sufficient
//! for spending-cap enforcement.

use soroban_sdk::{Address, Env, Symbol};

const SECONDS_PER_MONTH: u64 = 2_592_000; // 30 × 24 × 60 × 60

/// Current 30-day period bucket, derived from the ledger timestamp.
pub fn current_month_key(env: &Env) -> u32 {
    (env.ledger().timestamp() / SECONDS_PER_MONTH) as u32
}

/// Global default cap. `0` (including unset) means no cap.
pub fn get_monthly_cap_default(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get::<_, i128>(&crate::DataKey::MonthlyCapDefault)
        .unwrap_or(0)
}

/// Per-user override cap, if one has been set for this user.
/// Stored in persistent storage (not instance) — per-user data follows the
/// same storage tier as `Balance(Address)` (#386) to avoid growing instance
/// storage unboundedly as more users receive overrides.
pub fn get_monthly_cap_override(env: &Env, user: &Address) -> Option<i128> {
    env.storage()
        .persistent()
        .get::<_, i128>(&crate::DataKey::MonthlyCapOverride(user.clone()))
}

/// The cap that actually applies to `user`: their override if one is set,
/// otherwise the global default. `0` means no cap either way.
pub fn effective_cap(env: &Env, user: &Address) -> i128 {
    get_monthly_cap_override(env, user).unwrap_or_else(|| get_monthly_cap_default(env))
}

/// Amount `user` has debited during the current period. `0` if no debit has
/// been recorded yet this period (including the very first period a wallet
/// is ever active in — see module docs on pre-existing wallets).
pub fn get_monthly_spent(env: &Env, user: &Address) -> i128 {
    let key = current_month_key(env);
    env.storage()
        .persistent()
        .get::<_, i128>(&crate::DataKey::MonthlySpent(user.clone(), key))
        .unwrap_or(0)
}

fn record_monthly_spent(env: &Env, user: &Address, additional: i128) {
    let key = current_month_key(env);
    let current = env
        .storage()
        .persistent()
        .get::<_, i128>(&crate::DataKey::MonthlySpent(user.clone(), key))
        .unwrap_or(0);
    env.storage().persistent().set(
        &crate::DataKey::MonthlySpent(user.clone(), key),
        &(current + additional),
    );
}

/// Must be called from `debit()` after the balance-sufficiency check but
/// before the balance is mutated, so that a rejected debit never touches the
/// balance *and* a debit rejected for insufficient balance never pollutes
/// the monthly-spend counter.
///
/// Returns `Err(MonthlyCapExceeded)` if `amount` would push `user`'s
/// cumulative spend for the current period over their effective monthly
/// cap. A cap of `0` (unset default, or an explicit `0` override) means no
/// cap — all debits are allowed and nothing is recorded against a cap that
/// doesn't exist.
pub fn check_and_record_debit(
    env: &Env,
    user: &Address,
    amount: i128,
) -> Result<(), crate::ContractError> {
    let cap = effective_cap(env, user);
    if cap == 0 {
        // No cap configured — backward-compatible pass-through
        return Ok(());
    }
    let spent = get_monthly_spent(env, user);
    if spent + amount > cap {
        return Err(crate::ContractError::MonthlyCapExceeded);
    }
    record_monthly_spent(env, user, amount);
    Ok(())
}

/// Emits `("rent_wallet", "monthly_cap_set")`, matching the topic
/// convention used by every other event in this contract.
pub fn emit_monthly_cap_set(env: &Env, user: Option<Address>, cap: i128) {
    env.events().publish(
        (
            Symbol::new(env, "rent_wallet"),
            Symbol::new(env, "monthly_cap_set"),
        ),
        (user, cap),
    );
}
