# Contract Upgrade Process (Soroban)

## Overview
- Contributors can **deploy** new contract instances.
- Only the **multisig admin** (maintainers) can **upgrade** existing contracts.
- All upgrades must be proposed via a PR using the provided template.

## Step-by-step for contributors

### 1. Build and deploy the new contract
```bash
# From repo root
cd contracts/<your-contract>
cargo build --target wasm32-unknown-unknown --release
# Deploy with your tool of choice (e.g., Stellar CLI, soroban-cli)
# Record the new contract ID and WASM hash
```

### 2. Open a PR
- Use the `.github/PULL_REQUEST_TEMPLATE.md`
- Fill in the “Contract Upgrade Details” section:
  - Network (testnet/mainnet)
  - New contract ID
  - WASM hash (`sha256sum` of the wasm file)
  - Deployer public key
  - Link to the deploy transaction on a Stellar explorer
- Check the governance boxes:
  - Admin/upgrade authority is a multisig requiring maintainer sign-off
  - Maintainer has reviewed and approved the upgrade
  - Upgrade transaction is ready for maintainer signature (provide XDR if applicable)

### 3. Verification steps
- [ ] New contract deployed successfully
- [ ] All existing tests pass against the new contract
- [ ] Manual testing checklist completed (describe what you tested)
- [ ] No breaking changes for existing integrations (or list them)

### 4. Submit and wait for maintainer review
- Maintainers will review the PR.
- If approved, they will sign and submit the upgrade transaction via the multisig.

## Multisig admin setup (maintainers only)
- Create a Stellar multisig (e.g., 2-of-3 or 3-of-5).
- Ensure at least one signer is the maintainer (preferably on a hardware wallet).
- Set this multisig as the **admin** for any upgradeable contract during deployment.
- Never set a single contributor wallet as admin.

## Optional: timelock
For additional safety, you can use a timelock contract as the admin:
- Multisig proposes upgrade.
- Timelock enforces a delay (e.g., 24–48h).
- Multisig executes after the delay.

## Security notes
- Never log `process.env` or secrets (enforced by ESLint).
- Do not commit private keys or seed phrases.
- Use testnet for initial testing before proposing mainnet upgrades.
- Verify the WASM hash matches the source build before approving.

## References
- Stellar multisig documentation
- Soroban contract upgrade guide
- Project PR template (`.github/PULL_REQUEST_TEMPLATE.md`)
