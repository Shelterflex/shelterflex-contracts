#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, Address, BytesN, Env, String, Symbol, Vec,
};

#[contracttype]
#[derive(Clone)]
pub struct ScheduledInstalment {
    pub instalment_number: u32,
    pub due_timestamp: u64,
    pub amount_due: i128,
    pub amount_paid: i128,
    pub status: InstalmentStatus,
    pub paid_at: Option<u64>,
    pub last_tx_id: Option<BytesN<32>>,
}

#[contracttype]
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u32)]
pub enum WaiverReason {
    Hardship = 1,
    DisputeResolved = 2,
    AdminAdjustment = 3,
    Promotional = 4,
}

#[contracttype]
#[derive(Clone)]
pub struct WaiverAudit {
    pub actor: Address,
    pub reason: WaiverReason,
    pub amount_waived: i128,
    pub waived_at: u64,
}

#[contracttype]
#[derive(Clone, PartialEq)]
pub enum InstalmentStatus {
    Pending,
    Paid,
    Overdue,
    Waived,
}

#[contracttype]
pub enum DataKey {
    Config,
    Schedule(String),
    Waiver(String, u32),
    Paused,
}

#[contracttype]
pub struct Config {
    pub admin: Address,
    pub operator: Address,
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) {
    if is_paused(env) {
        panic!("ContractPaused");
    }
}

fn instalment_remaining(inst: &ScheduledInstalment) -> i128 {
    inst.amount_due - inst.amount_paid
}

fn assert_positive_payment(amount: i128) {
    if amount <= 0 {
        panic!("InvalidAmount");
    }
}

#[contract]
pub struct RentSchedule;

#[contractimpl]
impl RentSchedule {
    pub fn init(env: Env, admin: Address, operator: Address) {
        if env.storage().instance().has(&DataKey::Config) {
            panic!("AlreadyInitialized");
        }
        let cfg = Config {
            admin: admin.clone(),
            operator: operator.clone(),
        };
        env.storage().instance().set(&DataKey::Config, &cfg);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "init"),
            ),
            (),
        );
    }

    pub fn pause(env: Env, caller: Address) {
        caller.require_auth();
        let cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        if caller != cfg.admin {
            panic!("NotAuthorized");
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "paused"),
            ),
            (),
        );
    }

    pub fn unpause(env: Env, caller: Address) {
        caller.require_auth();
        let cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        if caller != cfg.admin {
            panic!("NotAuthorized");
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "unpaused"),
            ),
            (),
        );
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    pub fn create_schedule(
        env: Env,
        caller: Address,
        deal_id: String,
        instalments: Vec<ScheduledInstalment>,
    ) {
        let cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        if caller != cfg.admin && caller != cfg.operator {
            panic!("NotAuthorized");
        }
        caller.require_auth();
        if env
            .storage()
            .persistent()
            .has(&DataKey::Schedule(deal_id.clone()))
        {
            panic!("ScheduleExists");
        }
        let total_amount: i128 = instalments.iter().map(|i| i.amount_due).sum();
        let count = instalments.len();
        env.storage()
            .persistent()
            .set(&DataKey::Schedule(deal_id.clone()), &instalments);
        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "schedule_created"),
                deal_id,
            ),
            (count, total_amount),
        );
    }

    pub fn record_payment(
        env: Env,
        caller: Address,
        deal_id: String,
        instalment_number: u32,
        amount: i128,
        tx_id: BytesN<32>,
        paid_at: u64,
    ) {
        require_not_paused(&env);
        let _cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        caller.require_auth();
        assert_positive_payment(amount);

        let mut schedule: Vec<ScheduledInstalment> = env
            .storage()
            .persistent()
            .get(&DataKey::Schedule(deal_id.clone()))
            .expect("NoSchedule");
        let idx = schedule
            .iter()
            .position(|i| i.instalment_number == instalment_number)
            .expect("NotFound");
        let mut inst = schedule.get(idx as u32).unwrap();

        if inst.status == InstalmentStatus::Paid || inst.status == InstalmentStatus::Waived {
            panic!("InvalidStatus");
        }

        let remaining = instalment_remaining(&inst);
        if amount > remaining {
            panic!("Overpayment");
        }

        inst.amount_paid += amount;
        inst.last_tx_id = Option::Some(tx_id.clone());

        if inst.amount_paid == inst.amount_due {
            inst.status = InstalmentStatus::Paid;
            inst.paid_at = Option::Some(paid_at);
            schedule.set(idx as u32, inst.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Schedule(deal_id.clone()), &schedule);
            env.events().publish(
                (
                    Symbol::new(&env, "rent_schedule"),
                    Symbol::new(&env, "instalment_paid"),
                    deal_id.clone(),
                ),
                (instalment_number, inst.amount_due, tx_id),
            );
        } else {
            schedule.set(idx as u32, inst.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Schedule(deal_id.clone()), &schedule);
            env.events().publish(
                (
                    Symbol::new(&env, "rent_schedule"),
                    Symbol::new(&env, "partial_payment_recorded"),
                    deal_id.clone(),
                ),
                (
                    instalment_number,
                    amount,
                    inst.amount_paid,
                    instalment_remaining(&inst),
                    tx_id,
                ),
            );
        }
    }

    pub fn mark_overdue(env: Env, caller: Address, deal_id: String, instalment_number: u32) {
        require_not_paused(&env);
        let cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        if caller != cfg.admin && caller != cfg.operator {
            panic!("NotAuthorized");
        }
        caller.require_auth();
        let mut schedule: Vec<ScheduledInstalment> = env
            .storage()
            .persistent()
            .get(&DataKey::Schedule(deal_id.clone()))
            .expect("NoSchedule");
        let idx = schedule
            .iter()
            .position(|i| i.instalment_number == instalment_number)
            .expect("NotFound");
        let mut inst = schedule.get(idx as u32).unwrap();
        if inst.status == InstalmentStatus::Paid || inst.status == InstalmentStatus::Waived {
            panic!("InvalidStatus");
        }
        if instalment_remaining(&inst) <= 0 {
            panic!("InvalidStatus");
        }
        inst.status = InstalmentStatus::Overdue;
        let remaining = instalment_remaining(&inst);
        schedule.set(idx as u32, inst);
        env.storage()
            .persistent()
            .set(&DataKey::Schedule(deal_id.clone()), &schedule);
        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "instalment_overdue"),
                deal_id,
            ),
            (instalment_number, remaining),
        );
    }

    pub fn waive_instalment(
        env: Env,
        caller: Address,
        deal_id: String,
        instalment_number: u32,
        reason: WaiverReason,
    ) {
        let cfg: Config = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .expect("NotInitialized");
        if caller != cfg.admin {
            panic!("NotAuthorized");
        }
        caller.require_auth();
        let mut schedule: Vec<ScheduledInstalment> = env
            .storage()
            .persistent()
            .get(&DataKey::Schedule(deal_id.clone()))
            .expect("NoSchedule");
        let idx = schedule
            .iter()
            .position(|i| i.instalment_number == instalment_number)
            .expect("NotFound");
        let mut inst = schedule.get(idx as u32).unwrap();
        if inst.status == InstalmentStatus::Waived {
            panic!("AlreadyWaived");
        }
        if inst.status == InstalmentStatus::Paid {
            panic!("InvalidStatus");
        }

        let amount_waived = instalment_remaining(&inst);
        let waived_at = env.ledger().timestamp();
        inst.status = InstalmentStatus::Waived;
        schedule.set(idx as u32, inst);
        env.storage()
            .persistent()
            .set(&DataKey::Schedule(deal_id.clone()), &schedule);

        let audit = WaiverAudit {
            actor: caller.clone(),
            reason,
            amount_waived,
            waived_at,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Waiver(deal_id.clone(), instalment_number), &audit);

        env.events().publish(
            (
                Symbol::new(&env, "rent_schedule"),
                Symbol::new(&env, "instalment_waived"),
                deal_id,
            ),
            (
                instalment_number,
                caller,
                reason as u32,
                amount_waived,
                waived_at,
            ),
        );
    }

    pub fn get_waiver(env: Env, deal_id: String, instalment_number: u32) -> Option<WaiverAudit> {
        env.storage()
            .persistent()
            .get(&DataKey::Waiver(deal_id, instalment_number))
    }

    pub fn instalment_remaining(env: Env, deal_id: String, instalment_number: u32) -> i128 {
        let inst = Self::get_instalment(env.clone(), deal_id, instalment_number);
        instalment_remaining(&inst)
    }

    /// Returns instalments sorted by instalment_number ascending.
    pub fn get_schedule(env: Env, deal_id: String) -> Vec<ScheduledInstalment> {
        let mut schedule: Vec<ScheduledInstalment> = env
            .storage()
            .persistent()
            .get(&DataKey::Schedule(deal_id))
            .unwrap_or(Vec::new(&env));
        let len = schedule.len();
        let mut i = 1u32;
        while i < len {
            let mut j = i;
            while j > 0 {
                let a = schedule.get(j - 1).unwrap();
                let b = schedule.get(j).unwrap();
                if a.instalment_number > b.instalment_number {
                    schedule.set(j - 1, b.clone());
                    schedule.set(j, a);
                    j -= 1;
                } else {
                    break;
                }
            }
            i += 1;
        }
        schedule
    }

    pub fn get_instalment(
        env: Env,
        deal_id: String,
        instalment_number: u32,
    ) -> ScheduledInstalment {
        let schedule: Vec<ScheduledInstalment> = env
            .storage()
            .persistent()
            .get(&DataKey::Schedule(deal_id))
            .expect("NoSchedule");
        schedule
            .iter()
            .find(|i| i.instalment_number == instalment_number)
            .unwrap()
    }
}

#[cfg(test)]
mod test {
    extern crate std;
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, BytesN, Env, String, Vec};

    fn make_deal_id(env: &Env, s: &str) -> String {
        String::from_str(env, s)
    }

    fn make_instalments(env: &Env, count: u32) -> Vec<ScheduledInstalment> {
        let mut v: Vec<ScheduledInstalment> = Vec::new(env);
        for i in 0..count {
            v.push_back(ScheduledInstalment {
                instalment_number: i + 1,
                due_timestamp: (i as u64 + 1) * 30 * 24 * 3600,
                amount_due: 100_000i128 * (i as i128 + 1),
                amount_paid: 0,
                status: InstalmentStatus::Pending,
                paid_at: Option::None,
                last_tx_id: Option::None,
            });
        }
        v
    }

    fn setup(env: &Env) -> (Address, Address, soroban_sdk::Address) {
        let admin = Address::generate(env);
        let operator = Address::generate(env);
        let contract_id = env.register(RentSchedule, ());
        (admin, operator, contract_id)
    }

    #[test]
    fn create_schedule_and_get() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_001");
        let instalments = make_instalments(&env, 3);
        client.create_schedule(&admin, &deal_id, &instalments);

        let schedule = client.get_schedule(&deal_id);
        assert_eq!(schedule.len(), 3);
    }

    #[test]
    #[should_panic(expected = "ScheduleExists")]
    fn create_schedule_fails_if_duplicate() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_002");
        let instalments = make_instalments(&env, 2);
        client.create_schedule(&admin, &deal_id, &instalments.clone());
        client.create_schedule(&admin, &deal_id, &instalments);
    }

    #[test]
    fn partial_then_full_payment_transitions_to_paid() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_partial");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);

        let tx1 = BytesN::from_array(&env, &[1u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &40_000i128, &tx1, &1_000u64);
        let partial = client.get_instalment(&deal_id, &1u32);
        assert!(matches!(partial.status, InstalmentStatus::Pending));
        assert_eq!(partial.amount_paid, 40_000i128);
        assert_eq!(client.instalment_remaining(&deal_id, &1u32), 60_000i128);

        let tx2 = BytesN::from_array(&env, &[2u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &60_000i128, &tx2, &2_000u64);
        let paid = client.get_instalment(&deal_id, &1u32);
        assert!(matches!(paid.status, InstalmentStatus::Paid));
        assert_eq!(paid.amount_paid, 100_000i128);
        assert_eq!(client.instalment_remaining(&deal_id, &1u32), 0i128);
    }

    #[test]
    #[should_panic(expected = "Overpayment")]
    fn overpayment_beyond_due_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_overpay");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);

        let tx = BytesN::from_array(&env, &[3u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &100_001i128, &tx, &1_000u64);
    }

    #[test]
    fn mark_overdue_reflects_remaining_on_partial_payment() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_overdue_partial");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);

        let tx = BytesN::from_array(&env, &[4u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &25_000i128, &tx, &1_000u64);
        client.mark_overdue(&admin, &deal_id, &1u32);

        let inst = client.get_instalment(&deal_id, &1u32);
        assert!(matches!(inst.status, InstalmentStatus::Overdue));
        assert_eq!(client.instalment_remaining(&deal_id, &1u32), 75_000i128);
    }

    #[test]
    #[should_panic(expected = "InvalidStatus")]
    fn mark_overdue_on_paid_instalment_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_004");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);

        let tx_id = BytesN::from_array(&env, &[2u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &100_000i128, &tx_id, &1_000_000u64);
        client.mark_overdue(&admin, &deal_id, &1u32);
    }

    #[test]
    fn get_schedule_returns_sorted_order() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_006");
        let mut v: Vec<ScheduledInstalment> = Vec::new(&env);
        v.push_back(ScheduledInstalment {
            instalment_number: 3,
            due_timestamp: 90,
            amount_due: 300,
            amount_paid: 0,
            status: InstalmentStatus::Pending,
            paid_at: Option::None,
            last_tx_id: Option::None,
        });
        v.push_back(ScheduledInstalment {
            instalment_number: 1,
            due_timestamp: 30,
            amount_due: 100,
            amount_paid: 0,
            status: InstalmentStatus::Pending,
            paid_at: Option::None,
            last_tx_id: Option::None,
        });
        v.push_back(ScheduledInstalment {
            instalment_number: 2,
            due_timestamp: 60,
            amount_due: 200,
            amount_paid: 0,
            status: InstalmentStatus::Pending,
            paid_at: Option::None,
            last_tx_id: Option::None,
        });
        client.create_schedule(&admin, &deal_id, &v);

        let schedule = client.get_schedule(&deal_id);
        assert_eq!(schedule.get(0).unwrap().instalment_number, 1);
        assert_eq!(schedule.get(1).unwrap().instalment_number, 2);
        assert_eq!(schedule.get(2).unwrap().instalment_number, 3);
    }

    #[test]
    fn waiver_persists_audit_fields() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_waiver");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);
        client.waive_instalment(&admin, &deal_id, &1u32, &WaiverReason::Hardship);

        let audit = client.get_waiver(&deal_id, &1u32).unwrap();
        assert_eq!(audit.actor, admin);
        assert_eq!(audit.reason, WaiverReason::Hardship);
        assert_eq!(audit.amount_waived, 100_000i128);
        assert_eq!(audit.waived_at, env.ledger().timestamp());
    }

    #[test]
    #[should_panic(expected = "InvalidStatus")]
    fn record_payment_on_waived_instalment_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_008");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);
        client.waive_instalment(&admin, &deal_id, &1u32, &WaiverReason::AdminAdjustment);

        let tx_id = BytesN::from_array(&env, &[3u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &1_000i128, &tx_id, &1_000_000u64);
    }

    #[test]
    #[should_panic(expected = "InvalidStatus")]
    fn double_pay_on_paid_instalment_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_double_pay");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);

        let tx1 = BytesN::from_array(&env, &[5u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &100_000i128, &tx1, &1_000u64);
        let tx2 = BytesN::from_array(&env, &[6u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &1i128, &tx2, &2_000u64);
    }

    #[test]
    #[should_panic(expected = "ContractPaused")]
    fn paused_blocks_record_payment() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_009");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);
        client.pause(&admin);

        let tx_id = BytesN::from_array(&env, &[4u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &1_000i128, &tx_id, &1_000_000u64);
    }

    #[test]
    #[should_panic(expected = "ContractPaused")]
    fn paused_blocks_mark_overdue() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_010");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);
        client.pause(&admin);
        client.mark_overdue(&admin, &deal_id, &1u32);
    }

    #[test]
    fn unpause_restores_operations() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, operator, contract_id) = setup(&env);
        let client = RentScheduleClient::new(&env, &contract_id);
        client.init(&admin, &operator);

        let deal_id = make_deal_id(&env, "deal_011");
        let instalments = make_instalments(&env, 1);
        client.create_schedule(&admin, &deal_id, &instalments);
        client.pause(&admin);
        client.unpause(&admin);

        let tx_id = BytesN::from_array(&env, &[5u8; 32]);
        client.record_payment(&admin, &deal_id, &1u32, &100_000i128, &tx_id, &1_000_000u64);
        let inst = client.get_instalment(&deal_id, &1u32);
        assert!(matches!(inst.status, InstalmentStatus::Paid));
    }
}
