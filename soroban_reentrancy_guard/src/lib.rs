#![no_std]
//! Shared in-contract reentrancy primitives.
//!
//! These are *linked* helpers, not cross-contract calls: a contract that guards a
//! path calls into this module directly, so the guard costs one instance-storage
//! read/write and never leaves the contract. That is deliberate — routing the check
//! through the deployable `reentrancy_guard` contract would put an external call
//! inside the guard itself, which is both more gas and a weaker security model.
//!
//! This is a **library-only** crate (`crate-type = ["rlib"]`, no `#[contract]`). The
//! primitives deliberately do not live alongside the `reentrancy_guard` contract:
//! the workspace release profile uses fat LTO, so linking a crate that carries
//! `#[contractimpl]` entry points pulls those exports into the dependent contract's
//! WASM and fails the build with `Linking globals named 'init': symbol multiply
//! defined!`. Keep this crate free of contract entry points.
//!
//! The module is generic over the caller's storage key and error types, mirroring
//! how `soroban_access_control` parameterises over the caller's `NotAuthorized`
//! variant. Each contract therefore keeps its own `DataKey::Reentrancy` variant and
//! its own `ReentrancyDetected` discriminant — adopting these helpers changes no
//! storage key and no error code, so there is no migration concern.
//!
//! Two shapes are provided because both are already in use across the workspace:
//!
//! - [`enter`] / [`exit`] for explicit, manually-paired locking.
//! - [`Scoped`] for RAII locking, which releases on drop (including on the `?`
//!   early-return path).
//!
//! The flag lives in **instance** storage, matching every existing hand-rolled
//! guard: the lock is contract-wide, not per-user.

use soroban_sdk::{Env, IntoVal, Val};

/// Acquire the reentrancy lock stored at `key`.
///
/// Returns `on_reentry` if the lock is already held. A missing flag reads as
/// unlocked, so contracts deployed before the key existed keep working.
#[inline]
pub fn enter<K, E>(env: &Env, key: &K, on_reentry: E) -> Result<(), E>
where
    K: IntoVal<Env, Val>,
{
    if env
        .storage()
        .instance()
        .get::<_, bool>(key)
        .unwrap_or(false)
    {
        return Err(on_reentry);
    }
    env.storage().instance().set(key, &true);
    Ok(())
}

/// Release the reentrancy lock stored at `key`.
///
/// Infallible by design: it must be safe to call from a `Drop` impl.
#[inline]
pub fn exit<K>(env: &Env, key: &K)
where
    K: IntoVal<Env, Val>,
{
    env.storage().instance().set(key, &false);
}

/// RAII reentrancy lock: acquires on construction, releases on drop.
///
/// Prefer this over manual [`enter`]/[`exit`] pairs in bodies with more than one
/// exit path — the lock is released even when an intervening `?` returns early.
pub struct Scoped<'a, K>
where
    K: IntoVal<Env, Val> + Clone,
{
    env: &'a Env,
    key: K,
}

impl<'a, K> Scoped<'a, K>
where
    K: IntoVal<Env, Val> + Clone,
{
    /// Acquire the lock, or return `on_reentry` if it is already held.
    #[inline]
    pub fn new<E>(env: &'a Env, key: K, on_reentry: E) -> Result<Self, E> {
        enter(env, &key, on_reentry)?;
        Ok(Scoped { env, key })
    }
}

impl<K> Drop for Scoped<'_, K>
where
    K: IntoVal<Env, Val> + Clone,
{
    fn drop(&mut self) {
        exit(self.env, &self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Env};

    #[contracttype]
    #[derive(Clone)]
    enum TestKey {
        Reentrancy,
    }

    #[contracterror]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[repr(u32)]
    enum TestError {
        ReentrancyDetected = 1,
    }

    #[contract]
    struct GuardHarness;

    #[contractimpl]
    impl GuardHarness {
        /// Manual enter/exit: the lock must be free again once the call returns.
        pub fn manual(env: Env) -> Result<(), TestError> {
            enter(&env, &TestKey::Reentrancy, TestError::ReentrancyDetected)?;
            exit(&env, &TestKey::Reentrancy);
            Ok(())
        }

        /// Attempt to take the lock twice in one frame — the second must be rejected.
        pub fn nested(env: Env) -> Result<(), TestError> {
            enter(&env, &TestKey::Reentrancy, TestError::ReentrancyDetected)?;
            let inner = enter(&env, &TestKey::Reentrancy, TestError::ReentrancyDetected);
            exit(&env, &TestKey::Reentrancy);
            inner
        }

        pub fn scoped(env: Env) -> Result<(), TestError> {
            let _guard = Scoped::new(&env, TestKey::Reentrancy, TestError::ReentrancyDetected)?;
            Ok(())
        }

        /// The scope guard must release the lock even when the body returns `Err`.
        pub fn scoped_err(env: Env) -> Result<(), TestError> {
            let _guard = Scoped::new(&env, TestKey::Reentrancy, TestError::ReentrancyDetected)?;
            Err(TestError::ReentrancyDetected)
        }

        pub fn locked(env: Env) -> bool {
            env.storage()
                .instance()
                .get::<_, bool>(&TestKey::Reentrancy)
                .unwrap_or(false)
        }
    }

    fn harness(env: &Env) -> GuardHarnessClient<'_> {
        GuardHarnessClient::new(env, &env.register(GuardHarness, ()))
    }

    #[test]
    fn enter_then_exit_releases_the_lock() {
        let env = Env::default();
        let client = harness(&env);
        client.try_manual().unwrap().unwrap();
        assert!(!client.locked());
    }

    #[test]
    fn reentrant_enter_is_rejected() {
        let env = Env::default();
        let client = harness(&env);
        assert_eq!(
            client.try_nested().unwrap_err().unwrap(),
            TestError::ReentrancyDetected
        );
    }

    #[test]
    fn scoped_guard_releases_on_drop() {
        let env = Env::default();
        let client = harness(&env);
        client.try_scoped().unwrap().unwrap();
        assert!(!client.locked());
    }

    #[test]
    fn scoped_guard_releases_on_error_path() {
        let env = Env::default();
        let client = harness(&env);
        assert_eq!(
            client.try_scoped_err().unwrap_err().unwrap(),
            TestError::ReentrancyDetected
        );
        assert!(!client.locked());
    }

    #[test]
    fn lock_is_reusable_across_calls() {
        let env = Env::default();
        let client = harness(&env);
        client.try_scoped().unwrap().unwrap();
        client.try_scoped().unwrap().unwrap();
        assert!(!client.locked());
    }
}
