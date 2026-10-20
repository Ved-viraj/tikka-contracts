//! Pause, cancel, fees, rescue, emergency withdraw, and admin updates.

use super::*;

#[test]
fn test_admin_updates_oracle_address() {
    let env = Env::default();
    env.mock_all_auths();

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let oracle = Address::generate(&env);
    let new_oracle = Address::generate(&env);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let payment_token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let config = RaffleConfigBuilder::new(&env, payment_token)
        .description(String::from_str(&env, "Oracle migration"))
        .max_tickets(5)
        .max_tickets_per_tx(5)
        .prize_amount(MIN_TICKET_PRICE * 5)
        .randomness_source(RandomnessSource::External)
        .oracle_address(Some(oracle.clone()))
        .build()
        .expect("valid oracle migration config");

    client.init(&factory, &admin, &creator, &config);

    let raffle = client.get_raffle();
    assert_eq!(raffle.claim_lockup_seconds, DEFAULT_CLAIM_LOCKUP_SECONDS);
    assert_eq!(raffle.swap_deadline_seconds, DEFAULT_SWAP_DEADLINE_SECONDS);
    client.update_oracle_address(&new_oracle, &None);

    let raffle = client.get_raffle();
    assert_eq!(raffle.oracle_address, Some(new_oracle));
}

#[test]
fn test_admin_sets_protocol_fee_before_sales() {
    let env = Env::default();
    env.mock_all_auths();

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let treasury = Address::generate(&env);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let payment_token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let config = RaffleConfigBuilder::new(&env, payment_token)
        .description(String::from_str(&env, "Fee update"))
        .max_tickets(5)
        .max_tickets_per_tx(5)
        .prize_amount(MIN_TICKET_PRICE * 5)
        .protocol_fee_bp(100)
        .treasury_address(Some(treasury))
        .metadata_hash(BytesN::from_array(&env, &[3; 32]))
        .build()
        .expect("valid fee config");

    client.init(&factory, &admin, &creator, &config);

    let raffle = client.get_raffle();
    assert_eq!(raffle.claim_lockup_seconds, DEFAULT_CLAIM_LOCKUP_SECONDS);
    assert_eq!(raffle.swap_deadline_seconds, DEFAULT_SWAP_DEADLINE_SECONDS);

    client.set_protocol_fee_bp(&500);

    let raffle = client.get_raffle();
    assert_eq!(raffle.protocol_fee_bp, 500);
}

#[test]
fn test_wipe_storage_removes_all_keys() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let buyer_a = Address::generate(&env);
    let buyer_b = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let (token_addr, token_mint) = create_token(&env, &token_admin);
    token_mint.mint(&creator, &1_000_000);
    token_mint.mint(&buyer_a, &1_000_000);
    token_mint.mint(&buyer_b, &1_000_000);

    let mut config = base_config(&env, &token_addr);
    config.description = String::from_str(&env, "wipe test");
    config.max_tickets = 5;
    config.max_tickets_per_tx = 5;
    config.prize_amount = MIN_TICKET_PRICE * 10;
    config.metadata_hash = BytesN::from_array(&env, &[9u8; 32]);
    config.randomness_source = RandomnessSource::External;
    config.oracle_address = Some(Address::generate(&env));
    let expected_metadata_hash = config.metadata_hash.clone();

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&buyer_a, &3);
    client.buy_tickets(&buyer_b, &2);
    assert_metadata_hash(&client, &expected_metadata_hash);

    // Admin cancels are timelocked; use the emergency path to return the
    // prize and then refund every ticket so wipe can succeed.
    env.ledger().with_mut(|l| {
        l.sequence_number += 1_555_201;
    });
    client.emergency_withdraw(&creator);

    for i in 1..=5 {
        client.refund_ticket(&i);
    }

    assert_eq!(client.get_raffle().status, RaffleStatus::Cancelled);
    assert_metadata_hash(&client, &expected_metadata_hash);

    client.wipe_storage();

    env.as_contract(&contract_id, || {
        for i in 1..=5 {
            assert!(!env.storage().persistent().has(&DataKey::Ticket(i)));
            assert!(!env.storage().persistent().has(&DataKey::TicketRefunded(i)));
            assert!(!env.storage().persistent().has(&DataKey::CommitEntry(i)));
        }
        assert!(!env
            .storage()
            .persistent()
            .has(&DataKey::TicketCount(buyer_a.clone())));
        assert!(!env
            .storage()
            .persistent()
            .has(&DataKey::TicketCount(buyer_b.clone())));
        assert!(!env.storage().persistent().has(&DataKey::TicketBuyers));
        assert!(!env.storage().instance().has(&DataKey::Raffle));
        assert!(!env.storage().instance().has(&DataKey::Factory));
        assert!(!env.storage().instance().has(&DataKey::Admin));
        assert!(!env.storage().instance().has(&DataKey::PendingAdmin));
        assert!(!env.storage().instance().has(&DataKey::Paused));
        assert!(!env.storage().instance().has(&DataKey::ReentrancyGuard));
        assert!(!env.storage().instance().has(&DataKey::AccumulatedFees));
        assert!(!env.storage().instance().has(&DataKey::RandomnessRequested));
        assert!(!env
            .storage()
            .instance()
            .has(&DataKey::RandomnessRequestLedger));
        assert!(!env.storage().instance().has(&DataKey::RandomnessRequestId));
        assert!(!env.storage().instance().has(&DataKey::DrawingLock));
        assert!(!env.storage().instance().has(&DataKey::FinishTime));
        assert!(!env.storage().persistent().has(&DataKey::RandomnessSeed));
        assert!(!env.storage().persistent().has(&DataKey::Admin));
    });
}

#[test]
fn emergency_withdraw_fails_before_delay_in_finalized_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &10_000_000);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Test");
    config.end_time = 2_000;
    config.no_deadline = false;
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 10;
    config.claim_lockup_seconds = Some(0);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);
    client.finalize_raffle();

    let result = client.try_emergency_withdraw(&creator);
    assert_eq!(env.events().all().events().len(), 0);
    assert_eq!(result.err(), Some(Ok(Error::InvalidStatus)));
}

#[test]
fn emergency_withdraw_succeeds_after_delay_in_drawing_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Test");
    config.end_time = 2_000;
    config.no_deadline = false;
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 10;
    config.claim_lockup_seconds = Some(0);

    let contract_id = env.register(RaffleInstance, ());
    let client = RaffleInstanceClient::new(&env, &contract_id);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);
    env.ledger()
        .set_timestamp(2_000 + EMERGENCY_WITHDRAW_DELAY_SECONDS + 1);

    client.emergency_withdraw(&creator);
    let raffle = client.get_raffle();
    assert_eq!(raffle.status, RaffleStatus::Cancelled);
    assert!(!raffle.prize_deposited);
}

#[test]
fn emergency_withdraw_fails_for_no_deadline_raffle_before_timeout() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let oracle = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Refund test");
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 5;
    config.randomness_source = RandomnessSource::External;
    config.oracle_address = Some(oracle);
    config.claim_lockup_seconds = Some(0);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);

    let result = client.try_emergency_withdraw(&creator);
    assert_eq!(env.events().all().events().len(), 0);
    assert_eq!(result.err(), Some(Ok(Error::EmergencyTooEarly)));
}

#[test]
fn emergency_withdraw_succeeds_for_no_deadline_drawing_raffle_after_timeout() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let oracle = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Test");
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 10;
    config.randomness_source = RandomnessSource::External;
    config.oracle_address = Some(oracle);
    config.claim_lockup_seconds = Some(0);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);
    assert_eq!(client.get_raffle().status, RaffleStatus::Drawing);

    env.ledger().with_mut(|ledger| {
        ledger.sequence_number += (EMERGENCY_WITHDRAW_DELAY_SECONDS / 5) as u32 + 1;
    });

    client.emergency_withdraw(&creator);
    let raffle = client.get_raffle();
    assert_eq!(raffle.status, RaffleStatus::Cancelled);
    assert!(!raffle.prize_deposited);
}

#[test]
fn emergency_withdraw_fails_in_active_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Test");
    config.end_time = 10_000;
    config.no_deadline = false;
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 10;

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();

    let result = client.try_emergency_withdraw(&creator);
    assert_eq!(env.events().all().events().len(), 0);
    assert_eq!(result.err(), Some(Ok(Error::InvalidStatus)));
}

#[test]
fn emergency_withdraw_fails_in_cancelled_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Test");
    config.end_time = 2_000;
    config.no_deadline = false;
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 10;

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.cancel_raffle(&CancelReason::CreatorCancelled);

    let result = client.try_emergency_withdraw(&creator);
    assert_eq!(result.err(), Some(Ok(Error::InvalidStatus)));
}

#[test]
fn emergency_withdraw_fails_if_prize_not_deposited() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.end_time = 10_000;
    config.no_deadline = false;

    client.init(&factory, &admin, &creator, &config);
    assert_eq!(
        client.try_emergency_withdraw(&creator),
        Err(Ok(Error::PrizeNotDeposited))
    );
}

#[test]
fn emergency_withdraw_only_callable_by_creator_or_admin() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let stranger = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Guard release");
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 5;
    config.randomness_source = RandomnessSource::External;
    config.oracle_address = Some(Address::generate(&env));
    config.claim_lockup_seconds = Some(0);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);
    assert_eq!(client.get_raffle().status, RaffleStatus::Drawing);

    env.ledger().with_mut(|ledger| {
        ledger.sequence_number += (EMERGENCY_WITHDRAW_DELAY_SECONDS / 5) as u32 + 1;
    });

    let stranger_result = client.try_emergency_withdraw(&stranger);
    assert_eq!(stranger_result.err(), Some(Ok(Error::NotAuthorized)));

    client.emergency_withdraw(&admin);
}

#[test]
fn emergency_withdraw_sets_status_to_cancelled_and_clears_prize_deposited() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let factory = env.register(MockFactory, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let buyer = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = StellarAssetClient::new(&env, &payment_token);
    token_client.mint(&creator, &1_000_000);
    token_client.mint(&buyer, &1_000_000);

    let contract_id = env.register(Contract, ());
    let client = ContractClient::new(&env, &contract_id);

    let mut config = base_config(&env, &payment_token);
    config.description = String::from_str(&env, "Guard release");
    config.end_time = 2_000;
    config.no_deadline = false;
    config.max_tickets = 1;
    config.max_tickets_per_tx = 1;
    config.prize_amount = MIN_TICKET_PRICE * 5;
    config.randomness_source = RandomnessSource::External;
    config.oracle_address = Some(Address::generate(&env));
    config.claim_lockup_seconds = Some(0);

    client.init(&factory, &admin, &creator, &config);
    client.deposit_prize();
    client.buy_tickets(&creator, &1);
    assert_eq!(client.get_raffle().status, RaffleStatus::Drawing);
    env.ledger()
        .set_timestamp(2_000 + EMERGENCY_WITHDRAW_DELAY_SECONDS + 1);
    client.emergency_withdraw(&creator);

    let raffle = client.get_raffle();
    assert_eq!(raffle.status, RaffleStatus::Cancelled);
    assert!(!raffle.prize_deposited);
}

#[test]
fn every_admin_entrypoint_succeeds_for_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _creator, _buyer, _factory, _token_mint) = setup_active_raffle(&env);

    for entrypoint in ["set_protocol_fee_bp", "set_swap_deadline"] {
        match entrypoint {
            "set_protocol_fee_bp" => client.set_protocol_fee_bp(&1),
            "set_swap_deadline" => client.set_swap_deadline(&1),
            _ => unreachable!(),
        }
    }

    assert_eq!(client.get_raffle().protocol_fee_bp, 1);
    assert_eq!(client.get_raffle().swap_deadline_seconds, 1);
}

#[test]
fn every_admin_entrypoint_rejects_non_admin() {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    use soroban_sdk::IntoVal;

    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _creator, _buyer, _factory, _token_mint) = setup_active_raffle(&env);
    let stranger = Address::generate(&env);

    for entrypoint in ["set_protocol_fee_bp", "set_swap_deadline"] {
        let auth = match entrypoint {
            "set_protocol_fee_bp" => MockAuth {
                address: &stranger,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: entrypoint,
                    args: (1u32,).into_val(&env),
                    sub_invokes: &[],
                },
            },
            "set_swap_deadline" => MockAuth {
                address: &stranger,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: entrypoint,
                    args: (1u64,).into_val(&env),
                    sub_invokes: &[],
                },
            },
            _ => unreachable!(),
        };
        env.mock_auths(&[auth]);

        let result = match entrypoint {
            "set_protocol_fee_bp" => client.try_set_protocol_fee_bp(&1).map(|_| ()),
            "set_swap_deadline" => client.try_set_swap_deadline(&1).map(|_| ()),
            _ => unreachable!(),
        };
        assert!(result.is_err());
    }
}
