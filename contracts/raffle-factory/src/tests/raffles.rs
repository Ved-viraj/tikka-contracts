use super::*;

#[test]
fn non_whitelisted_creator_is_rate_limited() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    let delay: u64 = 300;
    client.set_creation_delay(&delay);

    let creator = Address::generate(&env);
    let token = make_token(&env);

    client.create_raffle(&creator, &rate_limit_config(&env, &token, "r1"));

    assert_eq!(
        client.try_create_raffle(&creator, &rate_limit_config(&env, &token, "r2")),
        Err(Ok(ContractError::RateLimitExceeded))
    );
    assert!(env.events().all().events().is_empty());

    env.ledger().set_timestamp(1_000 + delay);

    client.create_raffle(&creator, &rate_limit_config(&env, &token, "r3"));
}

#[test]
fn whitelisted_partner_bypasses_rate_limit() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    client.set_creation_delay(&300u64);

    let creator = Address::generate(&env);
    let token = make_token(&env);

    client.set_whitelist_status(&creator, &true);
    client.create_raffle(&creator, &rate_limit_config(&env, &token, "w1"));
    client.create_raffle(&creator, &rate_limit_config(&env, &token, "w2"));
}

#[test]
fn partner_dashboard_tracks_stats_across_creations() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(5_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    let partner = Address::generate(&env);
    let outsider = Address::generate(&env);
    let token = make_token(&env);

    assert!(client.get_partner_stats(&partner).is_none());

    client.set_whitelist_status(&partner, &true);

    let empty = client.get_partner_stats(&partner).unwrap();
    assert_eq!(empty.total_raffles, 0);
    assert_eq!(empty.total_volume, 0);
    assert_eq!(empty.total_fees_generated, 0);

    let partners = client.get_all_partners(&PaginationParams {
        limit: 10,
        offset: 0,
    });
    assert_eq!(partners.len(), 1);
    assert_eq!(partners.get(0).unwrap(), partner);

    client.create_raffle(&partner, &rate_limit_config(&env, &token, "p1"));
    env.ledger().set_timestamp(5_100);
    client.create_raffle(&partner, &rate_limit_config(&env, &token, "p2"));
    env.ledger().set_timestamp(5_200);
    client.create_raffle(&partner, &rate_limit_config(&env, &token, "p3"));

    let stats = client.get_partner_stats(&partner).unwrap();
    assert_eq!(stats.total_raffles, 3);
    assert_eq!(stats.first_raffle_at, 5_000);
    assert_eq!(stats.latest_raffle_at, 5_200);
    assert_eq!(stats.total_volume, 0);
    assert_eq!(stats.total_fees_generated, 0);

    assert!(client.get_partner_stats(&outsider).is_none());
    client.set_whitelist_status(&partner, &false);
    assert!(client.get_partner_stats(&partner).is_none());
    assert_eq!(
        client
            .get_all_partners(&PaginationParams {
                limit: 10,
                offset: 0,
            })
            .len(),
        0
    );
}

#[test]
fn set_creation_delay_affects_rate_limiter() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    client.set_creation_delay(&60u64);

    let creator = Address::generate(&env);
    let token = make_token(&env);

    client.create_raffle(&creator, &rate_limit_config(&env, &token, "d1"));

    env.ledger().set_timestamp(1_000 + 59);
    assert_eq!(
        client.try_create_raffle(&creator, &rate_limit_config(&env, &token, "d2")),
        Err(Ok(ContractError::RateLimitExceeded))
    );
    assert!(env.events().all().events().is_empty());

    env.ledger().set_timestamp(1_000 + 60);
    client.create_raffle(&creator, &rate_limit_config(&env, &token, "d3"));
}

#[test]
fn create_raffle_rejects_internal_randomness_above_prize_cap() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token = make_token(&env);

    let mut config = rate_limit_config(&env, &token, "cap-over");
    config.prize_amount = MAX_INTERNAL_RANDOMNESS_PRIZE_AMOUNT + 1;

    assert_eq!(
        client.try_create_raffle(&creator, &config),
        Err(Ok(ContractError::RandomnessSourceTooWeakForPrize))
    );
}

#[test]
fn create_raffle_accepts_internal_randomness_at_prize_cap_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token = make_token(&env);

    let mut config = rate_limit_config(&env, &token, "cap-boundary");
    config.prize_amount = MAX_INTERNAL_RANDOMNESS_PRIZE_AMOUNT;

    client.create_raffle(&creator, &config);
}

#[test]
fn test_set_creation_paused_blocks_create_raffle() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token = make_token(&env);

    assert!(!client.is_creation_paused());

    client.set_creation_paused(&true);
    assert!(client.is_creation_paused());

    assert_eq!(
        client.try_create_raffle(&creator, &rate_limit_config(&env, &token, "cp1")),
        Err(Ok(ContractError::CreationPaused))
    );
}

#[test]
fn test_set_creation_paused_unpause_allows_create_raffle() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token = make_token(&env);

    client.set_creation_paused(&true);
    client.set_creation_paused(&false);
    assert!(!client.is_creation_paused());

    client.create_raffle(&creator, &rate_limit_config(&env, &token, "cp2"));
}

#[test]
fn test_creation_paused_does_not_affect_full_pause() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);

    client.set_creation_paused(&true);
    assert!(!client.is_factory_paused());
}

#[test]
fn test_only_admin_can_set_creation_paused() {
    let env = Env::default();
    let (client, _admin, _treasury) = setup_factory(&env);
    let stranger = Address::generate(&env);

    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "set_creation_paused",
            args: (true,).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    // `require_admin` calls `admin.require_auth()`, so a stranger signature is
    // rejected by the host before the contract can return `NotAuthorized`.
    assert!(client.try_set_creation_paused(&true).is_err());
    assert!(!client.is_creation_paused());
}

#[test]
fn create_raffle_address_matches_predict_raffle_address() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let token = make_token(&env);
    // Same description twice: the salt is (creator, NextRaffleId), not the text.
    let config = rate_limit_config(&env, &token, "same-description");

    let nonce = client.get_next_raffle_id() as u64;
    let predicted = client.predict_raffle_address(&creator, &nonce);
    let deployed = client.create_raffle(&creator, &config);
    assert_eq!(deployed, predicted);

    let nonce_after = client.get_next_raffle_id() as u64;
    assert_eq!(nonce_after, nonce + 1);
    let predicted_next = client.predict_raffle_address(&creator, &nonce_after);
    let deployed_next = client.create_raffle(&creator, &config);
    assert_eq!(deployed_next, predicted_next);
    assert_ne!(deployed, deployed_next);
}
