#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
};

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Deal(BytesN<32>),
    Payment(BytesN<32>, u32),
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotAuthorized = 2,
    DealNotFound = 3,
    DealNotActive = 4,
    PaymentsNotComplete = 5,
    EquityOverflow = 6,
    InvalidAmount = 7,
    DealAlreadyExists = 8,
}

// ── Data Structures ───────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DealStatus {
    Active,
    Completed,
    Defaulted,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RentToOwnDeal {
    pub deal_id: BytesN<32>,
    pub tenant: Address,
    pub property_value_usdc: i128,
    pub equity_accumulated_usdc: i128,
    pub monthly_equity_usdc: i128,
    pub payments_made: u32,
    pub total_payments_required: u32,
    pub status: DealStatus,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct EquityPayment {
    pub deal_id: BytesN<32>,
    pub payment_number: u32,
    pub equity_amount: i128,
    pub total_rent_amount: i128,
    pub paid_at: u64,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RentToOwn;

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("not init")
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
    caller.require_auth();
    if caller != &get_admin(env) {
        return Err(ContractError::NotAuthorized);
    }
    Ok(())
}

#[contractimpl]
impl RentToOwn {
    pub fn init(env: Env, admin: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    /// Admin registers a rent-to-own deal.
    pub fn register_deal(
        env: Env,
        admin: Address,
        deal_id: BytesN<32>,
        tenant: Address,
        property_value_usdc: i128,
        monthly_equity_usdc: i128,
        total_payments_required: u32,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        if property_value_usdc <= 0 || monthly_equity_usdc <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::Deal(deal_id.clone()))
        {
            return Err(ContractError::DealAlreadyExists);
        }

        let deal = RentToOwnDeal {
            deal_id: deal_id.clone(),
            tenant,
            property_value_usdc,
            equity_accumulated_usdc: 0,
            monthly_equity_usdc,
            payments_made: 0,
            total_payments_required,
            status: DealStatus::Active,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Deal(deal_id.clone()), &deal);

        env.events().publish(
            (
                Symbol::new(&env, "rent_to_own"),
                Symbol::new(&env, "deal_registered"),
            ),
            deal_id,
        );
        Ok(())
    }

    /// Backend calls per monthly payment; stores payment record; increments equity.
    pub fn record_equity_payment(
        env: Env,
        admin: Address,
        deal_id: BytesN<32>,
        rent_amount: i128,
        equity_amount: i128,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        if rent_amount <= 0 || equity_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut deal: RentToOwnDeal = env
            .storage()
            .persistent()
            .get(&DataKey::Deal(deal_id.clone()))
            .ok_or(ContractError::DealNotFound)?;

        if !matches!(deal.status, DealStatus::Active) {
            return Err(ContractError::DealNotActive);
        }

        // Overpayment protection: equity cannot exceed property value
        let new_equity = deal.equity_accumulated_usdc + equity_amount;
        if new_equity > deal.property_value_usdc {
            return Err(ContractError::EquityOverflow);
        }

        deal.equity_accumulated_usdc = new_equity;
        deal.payments_made += 1;
        let payment_number = deal.payments_made;

        env.storage()
            .persistent()
            .set(&DataKey::Deal(deal_id.clone()), &deal);

        let payment = EquityPayment {
            deal_id: deal_id.clone(),
            payment_number,
            equity_amount,
            total_rent_amount: rent_amount,
            paid_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Payment(deal_id.clone(), payment_number), &payment);

        env.events().publish(
            (
                Symbol::new(&env, "rent_to_own"),
                Symbol::new(&env, "equity_payment_recorded"),
            ),
            (deal_id, payment_number, new_equity),
        );
        Ok(())
    }

    /// Admin marks deal completed when all payments made.
    pub fn complete_deal(
        env: Env,
        admin: Address,
        deal_id: BytesN<32>,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;

        let mut deal: RentToOwnDeal = env
            .storage()
            .persistent()
            .get(&DataKey::Deal(deal_id.clone()))
            .ok_or(ContractError::DealNotFound)?;

        if !matches!(deal.status, DealStatus::Active) {
            return Err(ContractError::DealNotActive);
        }
        if deal.payments_made != deal.total_payments_required {
            return Err(ContractError::PaymentsNotComplete);
        }

        deal.status = DealStatus::Completed;
        env.storage()
            .persistent()
            .set(&DataKey::Deal(deal_id.clone()), &deal);

        env.events().publish(
            (
                Symbol::new(&env, "rent_to_own"),
                Symbol::new(&env, "deal_completed"),
            ),
            deal_id,
        );
        Ok(())
    }

    /// Admin marks deal defaulted.
    pub fn default_deal(
        env: Env,
        admin: Address,
        deal_id: BytesN<32>,
        reason: Symbol,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;

        let mut deal: RentToOwnDeal = env
            .storage()
            .persistent()
            .get(&DataKey::Deal(deal_id.clone()))
            .ok_or(ContractError::DealNotFound)?;

        if !matches!(deal.status, DealStatus::Active) {
            return Err(ContractError::DealNotActive);
        }

        let accumulated = deal.equity_accumulated_usdc;
        deal.status = DealStatus::Defaulted;
        env.storage()
            .persistent()
            .set(&DataKey::Deal(deal_id.clone()), &deal);

        env.events().publish(
            (
                Symbol::new(&env, "rent_to_own"),
                Symbol::new(&env, "deal_defaulted"),
            ),
            (deal_id, reason, accumulated),
        );
        Ok(())
    }

    pub fn get_deal(env: Env, deal_id: BytesN<32>) -> Option<RentToOwnDeal> {
        env.storage().persistent().get(&DataKey::Deal(deal_id))
    }

    /// Returns equity as basis points of property value (0–10000).
    pub fn get_equity_percentage(env: Env, deal_id: BytesN<32>) -> u32 {
        let deal: RentToOwnDeal = match env.storage().persistent().get(&DataKey::Deal(deal_id)) {
            Some(d) => d,
            None => return 0,
        };
        if deal.property_value_usdc == 0 {
            return 0;
        }
        ((deal.equity_accumulated_usdc * 10_000) / deal.property_value_usdc) as u32
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup(env: &Env) -> (Address, RentToOwnClient<'_>) {
        env.mock_all_auths();
        let id = env.register(RentToOwn, ());
        let client = RentToOwnClient::new(env, &id);
        let admin = Address::generate(env);
        client.init(&admin);
        (admin, client)
    }

    fn make_deal_id(env: &Env, seed: u8) -> BytesN<32> {
        BytesN::from_array(env, &[seed; 32])
    }

    #[test]
    fn full_lifecycle_register_payments_complete() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 1);

        // property = 100_000, monthly_equity = 10_000, 10 payments
        client.register_deal(&admin, &deal_id, &tenant, &100_000, &10_000, &10);

        for _ in 0..10 {
            client.record_equity_payment(&admin, &deal_id, &15_000, &10_000);
        }

        let deal = client.get_deal(&deal_id).unwrap();
        assert_eq!(deal.payments_made, 10);
        assert_eq!(deal.equity_accumulated_usdc, 100_000);

        client.complete_deal(&admin, &deal_id);
        let deal = client.get_deal(&deal_id).unwrap();
        assert!(matches!(deal.status, DealStatus::Completed));
    }

    #[test]
    fn equity_is_monotonically_increasing() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 2);

        client.register_deal(&admin, &deal_id, &tenant, &100_000, &10_000, &5);

        let mut prev_equity = 0i128;
        for _ in 0..5 {
            client.record_equity_payment(&admin, &deal_id, &15_000, &10_000);
            let deal = client.get_deal(&deal_id).unwrap();
            assert!(deal.equity_accumulated_usdc > prev_equity);
            prev_equity = deal.equity_accumulated_usdc;
        }
    }

    #[test]
    fn default_mid_deal() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 3);

        client.register_deal(&admin, &deal_id, &tenant, &100_000, &10_000, &10);
        client.record_equity_payment(&admin, &deal_id, &15_000, &10_000);
        client.record_equity_payment(&admin, &deal_id, &15_000, &10_000);

        client.default_deal(&admin, &deal_id, &Symbol::new(&env, "missed_payment"));
        let deal = client.get_deal(&deal_id).unwrap();
        assert!(matches!(deal.status, DealStatus::Defaulted));
        assert_eq!(deal.equity_accumulated_usdc, 20_000);
    }

    #[test]
    fn overpayment_protection() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 4);

        // property = 10_000, monthly_equity = 6_000 — second payment would overflow
        client.register_deal(&admin, &deal_id, &tenant, &10_000, &6_000, &2);
        client.record_equity_payment(&admin, &deal_id, &8_000, &6_000);

        // Second payment of 6_000 would push equity to 12_000 > 10_000
        let result = client.try_record_equity_payment(&admin, &deal_id, &8_000, &6_000);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::EquityOverflow);
    }

    #[test]
    fn complete_deal_fails_if_payments_not_done() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 5);

        client.register_deal(&admin, &deal_id, &tenant, &100_000, &10_000, &10);
        client.record_equity_payment(&admin, &deal_id, &15_000, &10_000);

        let result = client.try_complete_deal(&admin, &deal_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            ContractError::PaymentsNotComplete
        );
    }

    #[test]
    fn equity_percentage_correct() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 6);

        client.register_deal(&admin, &deal_id, &tenant, &100_000, &25_000, &4);
        client.record_equity_payment(&admin, &deal_id, &30_000, &25_000);

        // 25_000 / 100_000 * 10_000 = 2_500 bps = 25%
        assert_eq!(client.get_equity_percentage(&deal_id), 2_500);
    }

    #[test]
    fn non_admin_cannot_register_deal() {
        let env = Env::default();
        let (_admin, client) = setup(&env);
        let attacker = Address::generate(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 7);

        let result = client.try_register_deal(&attacker, &deal_id, &tenant, &100_000, &10_000, &10);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::NotAuthorized);
    }

    #[test]
    fn payment_on_completed_deal_fails() {
        let env = Env::default();
        let (admin, client) = setup(&env);
        let tenant = Address::generate(&env);
        let deal_id = make_deal_id(&env, 8);

        client.register_deal(&admin, &deal_id, &tenant, &10_000, &10_000, &1);
        client.record_equity_payment(&admin, &deal_id, &10_000, &10_000);
        client.complete_deal(&admin, &deal_id);

        let result = client.try_record_equity_payment(&admin, &deal_id, &10_000, &10_000);
        assert_eq!(result.unwrap_err().unwrap(), ContractError::DealNotActive);
    }
}
