extern crate std;

use crate::{ContractError, EpochRewards, EpochRewardsClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};

fn setup(env: &Env, duration: u64) -> (Address, EpochRewardsClient<'_>) {
    env.mock_all_auths();
    let id = env.register(EpochRewards, ());
    let client = EpochRewardsClient::new(env, &id);
    let admin = Address::generate(env);
    client.init(&admin, &duration);
    (admin, client)
}

// ── 1. Happy path ─────────────────────────────────────────────────────────────

#[test]
fn happy_path_stake_fund_seal_claim() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let user = Address::generate(&env);
    client.stake(&user, &1_000);
    client.fund_epoch_rewards(&admin, &500);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    let claimable = client.get_claimable(&user);
    assert!(claimable > 0, "user should have claimable rewards");

    let claimed = client.claim(&user);
    assert_eq!(claimed, claimable);
    assert_eq!(client.get_claimable(&user), 0);
}

// ── 2. Pro-rata distribution ──────────────────────────────────────────────────

#[test]
fn pro_rata_three_stakers() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);

    // Stakes: 500, 300, 200 → total 1000
    client.stake(&a, &500);
    client.stake(&b, &300);
    client.stake(&c, &200);

    let total_reward: i128 = 1_000;
    client.fund_epoch_rewards(&admin, &total_reward);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    let ra = client.get_claimable(&a);
    let rb = client.get_claimable(&b);
    let rc = client.get_claimable(&c);

    // Expected: 500, 300, 200 (±1 stroop tolerance)
    assert!((ra - 500).abs() <= 1, "a expected ~500, got {}", ra);
    assert!((rb - 300).abs() <= 1, "b expected ~300, got {}", rb);
    assert!((rc - 200).abs() <= 1, "c expected ~200, got {}", rc);
}

// ── 3. Late joiner ────────────────────────────────────────────────────────────

#[test]
fn late_joiner_earns_less() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let early = Address::generate(&env);
    client.stake(&early, &1_000);
    // Fund half the rewards before late joiner
    client.fund_epoch_rewards(&admin, &500);

    // Late joiner stakes after first funding
    let late = Address::generate(&env);
    client.stake(&late, &1_000);
    // Fund second half after late joiner
    client.fund_epoch_rewards(&admin, &500);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    let early_rewards = client.get_claimable(&early);
    let late_rewards = client.get_claimable(&late);

    // Early staker should earn more than late joiner
    assert!(
        early_rewards > late_rewards,
        "early={} should > late={}",
        early_rewards,
        late_rewards
    );
    // Late joiner should still earn something (from second funding)
    assert!(late_rewards > 0);
}

// ── 4. Zero stakers ───────────────────────────────────────────────────────────

#[test]
fn zero_stakers_distribute_does_not_panic() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    // Fund with no stakers — should not panic
    let result = client.try_fund_epoch_rewards(&admin, &1_000);
    assert!(result.is_ok());

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    // Epoch sealed, rewards unallocated (reward index stays 0)
    let epoch = client.get_epoch(&1).unwrap();
    assert!(epoch.sealed);
    assert_eq!(epoch.total_rewards, 1_000);
}

// ── 5. Epoch boundary ─────────────────────────────────────────────────────────

#[test]
fn seal_before_epoch_end_returns_error() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    // Timestamp is 0, epoch not expired
    let result = client.try_seal_epoch(&admin, &1, &100);
    assert_eq!(
        result.unwrap_err().unwrap(),
        ContractError::EpochNotExpired
    );
}

// ── 6. Double claim ───────────────────────────────────────────────────────────

#[test]
fn double_claim_second_returns_zero() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let user = Address::generate(&env);
    client.stake(&user, &1_000);
    client.fund_epoch_rewards(&admin, &1_000);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    let first = client.claim(&user);
    assert!(first > 0);

    let second = client.claim(&user);
    assert_eq!(second, 0, "second claim should return 0");
}

// ── 7. Admin controls ─────────────────────────────────────────────────────────

#[test]
fn non_admin_cannot_seal_epoch() {
    let env = Env::default();
    let duration = 100u64;
    let (_admin, client) = setup(&env, duration);

    let attacker = Address::generate(&env);
    env.ledger().with_mut(|li| li.timestamp = duration + 1);

    let result = client.try_seal_epoch(&attacker, &1, &100);
    assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
}

#[test]
fn non_admin_cannot_fund_rewards() {
    let env = Env::default();
    let (_admin, client) = setup(&env, 100);

    let attacker = Address::generate(&env);
    let result = client.try_fund_epoch_rewards(&attacker, &1_000);
    assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
}

#[test]
fn operator_can_fund_and_seal() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let operator = Address::generate(&env);
    client.set_operator(&admin, &operator);

    let user = Address::generate(&env);
    client.stake(&user, &1_000);
    client.fund_epoch_rewards(&operator, &500);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&operator, &1, &100);

    assert_eq!(client.current_epoch(), 2);
}

// ── 8. Large numbers (100 stakers) ───────────────────────────────────────────

#[test]
fn large_numbers_100_stakers_no_overflow() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    let mut stakers = std::vec::Vec::new();
    for _ in 0..100 {
        let addr = Address::generate(&env);
        client.stake(&addr, &1_000_000);
        stakers.push(addr);
    }

    // Fund large reward
    client.fund_epoch_rewards(&admin, &100_000_000);

    env.ledger().with_mut(|li| li.timestamp = duration + 1);
    client.seal_epoch(&admin, &1, &100);

    // Each staker should have equal share ≈ 1_000_000 (±1)
    let mut total_claimed: i128 = 0;
    for staker in &stakers {
        let r = client.get_claimable(staker);
        assert!((r - 1_000_000).abs() <= 1, "staker reward {} out of range", r);
        total_claimed += r;
    }
    // Total claimed should be close to total funded (rounding losses ≤ 100)
    assert!((total_claimed - 100_000_000).abs() <= 100);
}

// ── 9. Paused state ───────────────────────────────────────────────────────────

#[test]
fn distribute_rewards_fails_when_paused() {
    let env = Env::default();
    let duration = 100u64;
    let (admin, client) = setup(&env, duration);

    client.pause(&admin);
    assert!(client.is_paused());

    let result = client.try_fund_epoch_rewards(&admin, &1_000);
    assert_eq!(result.unwrap_err().unwrap(), ContractError::Paused);

    // Unpause and verify it works again
    client.unpause(&admin);
    assert!(!client.is_paused());
    assert!(client.try_fund_epoch_rewards(&admin, &1_000).is_ok());
}
