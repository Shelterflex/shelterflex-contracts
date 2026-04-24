# Whistleblower Rewards Contract

This Soroban smart contract allocates and allows claiming whistleblower rewards.

## Role in the Shelterflex Platform

Whistleblowers are a **first-class participant** in the Shelterflex RNPL marketplace. They submit reports on fraudulent or misrepresented property listings and earn on-chain USDC rewards when their reports are validated.

Beyond trust and safety, whistleblowers serve an **advertising function**: by continuously removing bad listings from the platform, they maintain a high-quality, trustworthy inventory that attracts genuine tenants and landlords. This organic quality signal reduces the cost of customer acquisition and supports Shelterflex's go-to-market strategy — particularly in Phase 1 (supply-side onboarding) where landlord trust is critical.

### Why this matters for the business model

- Verified listings convert at higher rates → more deals → more interest revenue
- Reduced fraud exposure → lower default risk and insurance costs
- Community-driven verification scales more cheaply than a centralized inspection team
- Whistleblower rewards are a variable cost tied directly to platform activity, not a fixed overhead

## Contract responsibilities

- **Allocate rewards**: The admin or operator credits a reward amount against a whistleblower address and a listing/report identifier
- **Claim rewards**: Whistleblowers can claim their accrued rewards up to the allocated amount
- **Pause/unpause**: Emergency circuit-breaker to halt claims if needed

## Key state

| Storage key | Description |
|---|---|
| `TotalAllocated(address, listing_id)` | Total USDC allocated to a whistleblower for a specific report |
| `TotalClaimed(address, listing_id)` | Total USDC already claimed by that whistleblower |

The claimable balance at any point is `TotalAllocated - TotalClaimed`.

## Access control

| Role | Permissions |
|---|---|
| Admin | Allocate rewards, pause/unpause, upgrade contract, set operator |
| Operator | Allocate rewards on behalf of admin |
| Whistleblower | Claim their own allocated rewards |

## Upgrade governance

Contract upgrades follow a time-locked guardian pattern (see `StorageKey::UpgradeDelay` and `StorageKey::PendingUpgradeHash`). No upgrade can be applied until the delay has elapsed after the guardian commits the upgrade hash.

See `docs/specs/contracts/CONVENTIONS.md` for shared conventions and `docs/specs/contracts/UPGRADE_STRATEGY.md` for the contract upgrade/versioning strategy.
