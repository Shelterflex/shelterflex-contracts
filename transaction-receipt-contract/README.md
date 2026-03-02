# Transaction Receipt Contract

This Soroban smart contract records immutable transaction receipts keyed by a deterministic transaction ID derived from an external payment reference.

## Purpose

- Record canonical transaction receipts for on-chain indexing and audit.
- Prevent duplicate receipts using a deterministic tx_id derived from external payment references.
- Provide query APIs to fetch receipts and list receipts by deal with pagination.

## Initialization

Call `init(admin: Address, operator: Address)` once to initialize the contract. This stores the `admin` (manages operator and pause) and `operator` (records receipts) addresses and sets the paused state to `false`.

Attempting to `init` a second time returns `AlreadyInitialized`.

## Public API

- `init(env, admin, operator) -> Result<(), ContractError>`: initialize contract.
- `pause(env, admin) -> Result<(), ContractError>`: pause recording (admin only).
- `unpause(env, admin) -> Result<(), ContractError>`: unpause (admin only).
- `set_operator(env, admin, new_operator) -> Result<(), ContractError>`: update operator (admin only).
- `record_receipt(env, operator, input: ReceiptInput) -> Result<BytesN<32>, ContractError>`: record a transaction receipt (operator only, rejects duplicates or invalid input).
- `get_receipt(env, tx_id: BytesN<32>) -> Option<Receipt>`: fetch a receipt by tx_id.
- `list_receipts_by_deal(env, deal_id: String, limit: u32, cursor: Option<u32>) -> Vec<Receipt>`: list receipts for a deal with pagination.

## ReceiptInput and Receipt

`ReceiptInput` contains all fields required to record a receipt, including `external_ref_source`, `external_ref`, `tx_type`, `amount_usdc`, `token`, `deal_id`, and optional metadata fields.

`Receipt` is the stored, immutable representation returned by queries and emitted in events. The `external_ref` field in `Receipt` equals the `tx_id` (SHA-256 of canonicalized external ref) per contract design.

## Transaction ID canonicalization

Canonical format: `v1|source=<lowercased_trimmed_source>|ref=<trimmed_ref>`

Validation rules:
- `external_ref_source` must be one of the allowed sources: `paystack`, `flutterwave`, `bank_transfer`, `stellar`, `onramp`, `offramp`, `manual_admin` (case-insensitive).
- `external_ref` is trimmed of whitespace and must be non-empty, must not contain the pipe character `|`, and must be at most 256 characters.

The canonical string is SHA-256 hashed to produce a 32-byte `tx_id` (type `BytesN<32>`).

## Metadata hash

`metadata_hash` is optional and expected to be the SHA-256 hash of the canonical receipt payload (v1). The contract stores it as `BytesN<32>` if provided; generation of this hash is the caller's responsibility.

## Error codes

The contract exposes the following `ContractError` variants (numeric values shown in tests):

- `AlreadyInitialized` (1)
- `NotAuthorized` (2)
- `ContractPaused` (3)
- `DuplicateTransaction` (4)
- `InvalidAmount` (5)
- `InvalidExternalRefSource` (6)
- `InvalidExternalRef` (7)
- `InvalidTimestamp` (8)

## Events

On successful `record_receipt`, the contract emits an event with topic `("receipt", tx_id)` and the `Receipt` payload.

## Usage examples (testing harness)

The `src/test.rs` and `src/integration_tests.rs` files contain examples of how to call the contract from the Soroban test environment (`Env`). Examples include initialization, recording receipts, and querying.

## Testing

Run the contract tests with:

```bash
cargo test --manifest-path monorepo/contracts/transaction-receipt-contract/Cargo.toml
```

The project includes unit tests and integration-style tests that cover canonicalization, authorization, pause/unpause, duplicate prevention, and pagination.

Property-based tests are described in the design spec and can be added using `proptest` where necessary. Current tests validate the core behaviors and invariants.

## Notes

- The contract uses Soroban SDK storage patterns and event emission.
- The caller is responsible for generating `metadata_hash` if desired.
- The contract enforces deterministic tx_id generation to prevent duplicates originating from the same external reference.
