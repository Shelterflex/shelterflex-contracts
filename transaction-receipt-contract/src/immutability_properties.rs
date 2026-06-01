//! Immutability Properties and Invariants of the Transaction Receipt Contract
//!
//! This module documents and proves the core invariants that guarantee receipt
//! immutability on-chain.
//!
//! # Core Invariants
//!
//! 1. **Duplicate Prevention (Deterministic Unique Keying)**
//!    Every receipt is stored using a unique, deterministic key (`tx_id`) derived from the SHA-256
//!    hash of the trimmed, canonicalized payment reference source and reference value:
//!    `v1|source=<lowercased_trimmed_source>|ref=<trimmed_ref>`.
//!    The contract enforces that any attempt to record a receipt with an existing `tx_id` fails
//!    with `ContractError::DuplicateTransaction`, preventing existing records from being overwritten.
//!
//! 2. **No Mutation Interface (Write-Once-Read-Always)**
//!    Once stored in the contract's persistent storage, there are absolutely no update, delete, or
//!    overwrite pathways exposed. Neither the admin, the operator, nor any other party has the privilege
//!    to alter the content of a recorded receipt. The contract provides only read-only methods
//!    (`get_receipt`, `list_receipts_by_deal`, `list_receipts_by_user`) after a receipt is recorded.
//!
//! 3. **Tamper-Proof Audit Trail (Ledger Time & Event Integrity)**
//!    Each recorded receipt permanently captures the consensus-enforced ledger timestamp
//!    (`env.ledger().timestamp()`) at the exact execution transaction block. Successful recording
//!    unconditionally publishes a `receipt_recorded` event. Because the Stellar/Soroban ledger
//!    history is immutable, this guarantees that both the storage and the event history represent a
//!    tamper-proof, historically accurate ledger of payments.
