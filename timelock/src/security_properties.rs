//! Documented security invariants for the timelock contract (issue #975).
//!
//! These properties are enforced by unit tests in `tests.rs` and mirror the
//! on-chain behaviour in `lib.rs`.

// INVARIANT 1: Delay enforcement
// A queued operation cannot execute before `eta` (proposal time + delay).
// Violation returns `TimelockError::TimestampNotMet`.

// INVARIANT 2: Post-delay execution
// Once `now >= eta` and the operation is still within the grace window,
// execution proceeds and invokes the target contract.

// INVARIANT 3: Expiry window
// Operations expire if not executed within `GRACE_PERIOD` seconds after `eta`.
// Late execution returns `TimelockError::TransactionExpired`.

// INVARIANT 4: Admin-only queueing
// Only the configured admin may queue or cancel operations.
// Non-admin callers receive `TimelockError::NotAuthorized`.

// INVARIANT 5: Cancel before execution
// Admin cancellation removes the queue entry; subsequent execute attempts
// return `TimelockError::TransactionNotQueued`.

// INVARIANT 6: Operation ID uniqueness
// Proposing the same `(target, function, args, delay)` at the same ledger
// time yields the same hash; a duplicate queue returns
// `TimelockError::TransactionAlreadyQueued`.

// INVARIANT 7: Minimum delay bounds
// Queue delay must satisfy `MinDelay <= delay <= MaxDelay`. Admin may increase
// `MinDelay` via `set_min_delay`; decreases are rejected.

// INVARIANT 8: Zero-delay protection
// `init` and `set_min_delay` reject `min_delay == 0` with `InvalidDelay`.

// INVARIANT 9: Pause blocks execution
// While paused, `execute` returns `TimelockError::ContractPaused` even if the
// delay has elapsed.
