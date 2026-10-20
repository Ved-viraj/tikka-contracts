use super::*;

#[test]
fn test_stable_ids_initial_state() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    assert_eq!(client.get_next_raffle_id(), 0u32);
    assert_eq!(client.get_raffle_count(), 0u32);
    assert_eq!(client.get_raffle_by_id(&0u32), None);
}

#[test]
fn test_stable_ids_seeded_lookup() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let addrs = seed_raffles(&env, &client.address, 3);

    assert_eq!(client.get_next_raffle_id(), 3u32);
    assert_eq!(client.get_raffle_count(), 3u32);
    assert_eq!(client.get_raffle_by_id(&0u32), Some(addrs.get(0).unwrap()));
    assert_eq!(client.get_raffle_by_id(&1u32), Some(addrs.get(1).unwrap()));
    assert_eq!(client.get_raffle_by_id(&2u32), Some(addrs.get(2).unwrap()));
    assert_eq!(client.get_raffle_by_id(&99u32), None);
}

#[test]
fn test_get_raffles_page_returns_correct_slice() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let addrs = seed_raffles(&env, &client.address, 5);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 3,
        offset: 0,
    });
    assert_eq!(page.items.len(), 3u32);
    assert_eq!(page.items.get(0).unwrap(), addrs.get(0).unwrap());
    assert_eq!(page.items.get(2).unwrap(), addrs.get(2).unwrap());
    assert!(page.has_more);

    let page2 = client.get_raffles_page(&PaginationParams {
        limit: 3,
        offset: 3,
    });
    assert_eq!(page2.items.len(), 2u32);
    assert_eq!(page2.items.get(0).unwrap(), addrs.get(3).unwrap());
    assert_eq!(page2.items.get(1).unwrap(), addrs.get(4).unwrap());
    assert!(!page2.has_more);

    let page3 = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 99,
    });
    assert_eq!(page3.items.len(), 0u32);
    assert!(!page3.has_more);
}

#[test]
fn test_get_raffles_page_skips_tombstoned_slots() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let addrs = seed_raffles(&env, &client.address, 3);

    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::RaffleById(1u32));
        let count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::RaffleCount)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&DataKey::RaffleCount, &count.saturating_sub(1));
    });

    assert_eq!(client.get_raffle_count(), 2u32);
    assert_eq!(client.get_next_raffle_id(), 3u32);
    assert_eq!(client.get_raffle_by_id(&1u32), None);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 0,
    });
    assert_eq!(page.items.len(), 2u32);
    assert_eq!(page.items.get(0).unwrap(), addrs.get(0).unwrap());
    assert_eq!(page.items.get(1).unwrap(), addrs.get(2).unwrap());
}

#[test]
fn get_raffles_page_empty_list() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 0,
    });
    assert_eq!(page.items.len(), 0u32);
    assert_eq!(page.total, 0u32);
    assert!(!page.has_more);
}

#[test]
fn get_raffles_page_first_page() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    create_raffles_via_factory(&env, &client, &_admin, &_treasury, &creator, 15);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 0,
    });
    assert_eq!(page.items.len(), 10u32);
    assert_eq!(page.total, 15u32);
    assert!(page.has_more);
}

#[test]
fn get_raffles_page_last_page() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    create_raffles_via_factory(&env, &client, &_admin, &_treasury, &creator, 15);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 10,
    });
    assert_eq!(page.items.len(), 5u32);
    assert_eq!(page.total, 15u32);
    assert!(!page.has_more);
}

#[test]
fn get_raffles_page_offset_beyond_total() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    create_raffles_via_factory(&env, &client, &_admin, &_treasury, &creator, 5);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 10,
        offset: 10,
    });
    assert_eq!(page.items.len(), 0u32);
    assert_eq!(page.total, 5u32);
    assert!(!page.has_more);
}

#[test]
fn get_raffles_page_limit_zero_uses_default() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    create_raffles_via_factory(&env, &client, &_admin, &_treasury, &creator, 150);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 0,
        offset: 0,
    });
    assert_eq!(page.items.len(), DEFAULT_PAGE_LIMIT);
    assert_eq!(page.total, 150u32);
    assert!(page.has_more);
}

#[test]
fn get_raffles_page_limit_above_max_is_clamped() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    create_raffles_via_factory(&env, &client, &_admin, &_treasury, &creator, 250);

    let page = client.get_raffles_page(&PaginationParams {
        limit: 999,
        offset: 0,
    });
    assert_eq!(page.items.len(), MAX_PAGE_LIMIT);
    assert_eq!(page.total, 250u32);
    assert!(page.has_more);
}

#[test]
fn clean_old_raffle_prunes_pagination() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let addrs = create_raffles_via_factory(&env, &client, &admin, &_treasury, &creator, 10);

    for id in [1u32, 3, 5] {
        mark_raffle_failed(&env, &addrs.get(id).unwrap());
        assert!(client.try_clean_old_raffle(&id).is_ok());
    }

    assert_eq!(client.get_raffle_count(), 7u32);
    assert_eq!(client.get_next_raffle_id(), 10u32);

    let mut all_pages = SdkVec::new(&env);
    for page in 0..10 {
        let p = client.get_raffles_page(&PaginationParams {
            limit: 3,
            offset: (page * 3) as u32,
        });
        for i in 0..p.items.len() {
            all_pages.push_back(p.items.get(i).unwrap());
        }
    }

    let mut expected = SdkVec::new(&env);
    for i in 0..10 {
        if i != 1 && i != 3 && i != 5 {
            expected.push_back(addrs.get(i).unwrap().clone());
        }
    }

    assert_eq!(all_pages.len(), expected.len());
    for i in 0..expected.len() {
        assert_eq!(all_pages.get(i).unwrap(), expected.get(i).unwrap());
    }
}

#[test]
fn clean_old_raffle_prunes_creator_index() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let addrs = create_raffles_via_factory(&env, &client, &admin, &_treasury, &creator, 3);

    assert_eq!(
        client
            .get_raffles_by_creator(
                &creator,
                &PaginationParams {
                    limit: 10,
                    offset: 0
                }
            )
            .total,
        3u32
    );

    mark_raffle_failed(&env, &addrs.get(2).unwrap());
    client.clean_old_raffle(&2u32);

    assert_eq!(
        client
            .get_raffles_by_creator(
                &creator,
                &PaginationParams {
                    limit: 10,
                    offset: 0
                }
            )
            .total,
        2u32
    );
}

#[test]
fn clean_old_raffle_prunes_category_index() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let mut config = test_raffle_config(&env, &payment_token);
    config.category = Some(String::from_str(&env, "gaming"));
    let _addr1 = client.create_raffle(&creator, &config);
    let addr2 = client.create_raffle(&creator, &config);

    assert_eq!(
        client
            .get_raffles_by_category(
                &String::from_str(&env, "gaming"),
                &PaginationParams {
                    limit: 10,
                    offset: 0
                }
            )
            .total,
        2u32
    );

    mark_raffle_failed(&env, &addr2);
    client.clean_old_raffle(&1u32);

    assert_eq!(
        client
            .get_raffles_by_category(
                &String::from_str(&env, "gaming"),
                &PaginationParams {
                    limit: 10,
                    offset: 0
                }
            )
            .total,
        1u32
    );
}

#[test]
fn test_clean_old_raffle_invalid_id_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    assert_eq!(
        client.try_clean_old_raffle(&0u32),
        Err(Ok(ContractError::InvalidRaffleId))
    );
    assert!(env.events().all().events().is_empty());
}

#[test]
fn test_clean_old_raffle_already_tombstoned_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    seed_raffles(&env, &client.address, 3);

    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::RaffleById(1u32));
    });

    assert_eq!(
        client.try_clean_old_raffle(&1u32),
        Err(Ok(ContractError::InvalidRaffleId))
    );
    assert!(env.events().all().events().is_empty());
}
