use super::*;
use soroban_sdk::testutils::Ledger;

#[test]
fn test_init_factory() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = existing_address(&env);
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);
    assert_eq!(env.events().all().events().len(), 1);
    assert_event(&env, &client.address, "factory_initialized");
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_record_volume_overflow() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let asset = Address::generate(&env);

    client.record_volume(&asset, &(i128::MAX - 1));
    assert_eq!(client.get_total_volume(&asset), i128::MAX - 1);
    assert!(client.try_record_volume(&asset, &2).is_err());
    assert!(env.events().all().events().is_empty());
    assert_eq!(client.get_total_volume(&asset), i128::MAX - 1);
}

#[test]
fn test_propose_fee_change_rejects_excessive_protocol_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let excessive_fee = MAX_PROTOCOL_FEE_BP + 1;

    assert_eq!(
        client.try_propose_fee_change(&excessive_fee),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_sync_admin_stages_instance_transfer_for_factory_pending_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let instance_address =
        create_raffles_via_factory(&env, &client, &admin, &treasury, &creator, 1)
            .get(0)
            .unwrap();
    let instance = raffle_instance::RaffleInstanceClient::new(&env, &instance_address);
    let new_admin = existing_address(&env);

    client.transfer_factory_admin(&new_admin);
    client.sync_admin(&instance_address);

    let pending_instance_admin: Option<Address> = env.as_contract(&instance_address, || {
        env.storage()
            .instance()
            .get(&raffle_instance::DataKey::PendingAdmin)
    });
    assert_eq!(pending_instance_admin, Some(new_admin.clone()));
    let instance_admin_before_accept: Address = env.as_contract(&instance_address, || {
        env.storage()
            .instance()
            .get(&raffle_instance::DataKey::Admin)
            .unwrap()
    });
    assert_eq!(instance_admin_before_accept, admin);

    instance.accept_admin();
    assert_eq!(env.events().all().events().len(), 1);
    assert_event(&env, &instance_address, "admin_changed");

    let instance_admin: Address = env.as_contract(&instance_address, || {
        env.storage()
            .instance()
            .get(&raffle_instance::DataKey::Admin)
            .unwrap()
    });
    assert_eq!(instance_admin, new_admin);

    client.accept_factory_admin();
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_init_factory_rejects_second_call() {
    let env = Env::default();

    env.mock_all_auths();
    let admin = existing_address(&env);
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);
    assert_eq!(
        client.try_init_factory(&admin, &wasm_hash, &0u32, &treasury),
        Err(Ok(ContractError::AlreadyInitialized))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_init_factory_rejects_zero_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&zero_address(&env), &wasm_hash, &0u32, &treasury),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_init_factory_rejects_zero_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&admin, &wasm_hash, &0u32, &zero_address(&env)),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_init_factory_rejects_self_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&contract_id, &wasm_hash, &0u32, &treasury),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

fn nonexistent_contract(env: &Env) -> Address {
    env.deployer()
        .with_address(Address::generate(env), BytesN::from_array(env, &[9u8; 32]))
        .deployed_address()
}

#[test]
fn test_init_factory_rejects_nonexistent_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let treasury = env.register(RaffleFactory, ());
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&nonexistent_contract(&env), &wasm_hash, &0u32, &treasury),
        Err(Ok(ContractError::InvalidParameters))
    );
}

#[test]
fn test_init_factory_rejects_nonexistent_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = env.register(RaffleFactory, ());
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&admin, &wasm_hash, &0u32, &nonexistent_contract(&env)),
        Err(Ok(ContractError::InvalidParameters))
    );
}

#[test]
fn test_init_factory_rejects_self_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);

    assert_eq!(
        client.try_init_factory(&admin, &wasm_hash, &0u32, &contract_id),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_transfer_factory_admin_rejects_zero_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    assert_eq!(
        client.try_transfer_factory_admin(&zero_address(&env)),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_transfer_factory_admin_rejects_self() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let self_address = client.address.clone();

    assert_eq!(
        client.try_transfer_factory_admin(&self_address),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_propose_config_change_rejects_zero_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    assert_eq!(
        client.try_set_config(&ConfigKey::Treasury, &zero_address(&env)),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_set_config_rejects_nonexistent_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    assert_eq!(
        client.try_set_config(&ConfigKey::Treasury, &nonexistent_contract(&env)),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_propose_config_change_rejects_self_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let self_address = client.address.clone();

    assert_eq!(
        client.try_set_config(&ConfigKey::Treasury, &self_address),
        Err(Ok(ContractError::InvalidParameters))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_upgrade_requires_admin_authorization() {
    let env = Env::default();
    let admin = existing_address(&env);
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);
    env.mock_all_auths();
    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);

    let new_hash = BytesN::from_array(&env, &[9u8; 32]);
    env.set_auths(&[]);
    assert!(client.try_upgrade(&new_hash).is_err());
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_upgrade_lifecycle_preserves_state() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = existing_address(&env);
    let treasury = existing_address(&env);
    let wasm_hash = upload_instance_wasm(&env);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);
    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);
    client.set_creation_delay(&0u64);

    let creator = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let mut config = test_raffle_config(&env, &payment_token);
    config.protocol_fee_bp = 0;
    config.treasury_address = Some(treasury.clone());
    let raffle_address = client.create_raffle(&creator, &config);

    let new_hash = BytesN::from_array(&env, &[9u8; 32]);
    let op_id = client.propose_wasm_upgrade(&new_hash);
    assert_eq!(
        client.get_pending_op(&op_id).unwrap().op,
        AdminOp::UpdateWasmHash(new_hash.clone())
    );

    let err = client.try_execute_config_change(&op_id);
    assert_eq!(err.err(), Some(Ok(ContractError::TimelockNotElapsed)));

    env.ledger()
        .with_mut(|l| l.timestamp += TIMELOCK_DELAY_SECONDS + 1);
    client.execute_config_change(&op_id);

    let pending = client.get_pending_op(&op_id);
    assert!(pending.is_none());
    let raffle = raffle_instance::RaffleInstanceClient::new(&env, &raffle_address);
    let raffle_state = raffle.get_raffle();
    assert_eq!(raffle_state.creator, creator);
    assert_eq!(raffle_state.treasury_address, Some(treasury.clone()));
}

#[test]
fn test_admin_transfer_two_step_completes_correctly() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let new_admin = existing_address(&env);

    client.transfer_factory_admin(&new_admin);

    assert_event(&env, &client.address, "admin_transfer_proposed");

    client.accept_factory_admin();

    assert_event(&env, &client.address, "admin_transfer_accepted");
    assert_eq!(env.events().all().events().len(), 1);

    let actual: Address = env.as_contract(&client.address, || {
        env.storage().persistent().get(&DataKey::Admin).unwrap()
    });
    assert_eq!(actual, new_admin);

    let pending_still_exists: bool = env.as_contract(&client.address, || {
        env.storage().persistent().has(&DataKey::PendingAdmin)
    });
    assert!(!pending_still_exists);
}

#[test]
fn test_admin_transfer_rejected_if_pending_already_exists() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let admin_b = existing_address(&env);
    let admin_c = existing_address(&env);

    client.transfer_factory_admin(&admin_b);

    assert_eq!(
        client.try_transfer_factory_admin(&admin_c),
        Err(Ok(ContractError::AdminTransferPending))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_admin_accept_fails_if_wrong_address_accepts() {
    let env = Env::default();
    let (client, _admin, _treasury) = setup_factory(&env);
    let admin_b = existing_address(&env);
    let admin_c = existing_address(&env);

    env.mock_all_auths();
    client.transfer_factory_admin(&admin_b);

    env.mock_auths(&[MockAuth {
        address: &admin_c,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "accept_factory_admin",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_accept_factory_admin().is_err());
}

#[test]
fn test_admin_transfer_to_same_address_clears_pending() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _treasury) = setup_factory(&env);
    let new_admin = existing_address(&env);

    client.transfer_factory_admin(&new_admin);

    let pending_before: bool = env.as_contract(&client.address, || {
        env.storage().persistent().has(&DataKey::PendingAdmin)
    });
    assert!(pending_before);

    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "transfer_factory_admin",
            args: (&admin,).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.transfer_factory_admin(&admin);
    assert!(env.events().all().events().is_empty());

    let pending_after: bool = env.as_contract(&client.address, || {
        env.storage().persistent().has(&DataKey::PendingAdmin)
    });
    assert!(!pending_after);

    let actual: Address = env.as_contract(&client.address, || {
        env.storage().persistent().get(&DataKey::Admin).unwrap()
    });
    assert_eq!(actual, admin);
}

#[test]
fn test_only_new_admin_can_accept_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _treasury) = setup_factory(&env);
    let new_admin = existing_address(&env);

    client.transfer_factory_admin(&new_admin);

    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "accept_factory_admin",
            args: ().into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_accept_factory_admin().is_err());
}
