.PHONY: kani-staking-pool kani

# Run Kani formal verification for the staking_pool contract.
kani-staking-pool:
	cargo kani --package staking_pool

# Alias for CI and local use.
kani: kani-staking-pool
