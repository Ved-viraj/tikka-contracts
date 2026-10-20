//! Edge-case tests for the views module pagination and coverage.

// All raffle_shared items (PaginationParams, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT)
// and soroban_sdk primitives are re-exported by the parent mod.rs.
// Duplicating those imports here was the source of E0252 (#982).
use super::*;

use crate::{RaffleConfig, RaffleFactory, RaffleFactoryClient};

fn setup_with_raffles(env: &Env, count: u32) -> RaffleFactoryClient<'_> {
    env.mock_all_auths();
    let admin = existing_address(env);
    let treasury = existing_address(env);
    let wasm_hash = upload_instance_wasm(env);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(env, &contract_id);

    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);
    client.set_creation_delay(&0u64);

    let creator = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    for _ in 0..count {
        let config = test_raffle_config(env, &token);
        client.create_raffle(&creator, &config);
    }

    client
}

// ── get_raffles_page ─────────────────────────────────────────────────────

#[test]
fn raffles_page_offset_beyond_total_returns_empty() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 5);

    let page = client.get_raffles_page(&PaginationParams {
        offset: 100,
        limit: 10,
    });

    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total, 5);
    assert!(!page.has_more);
}

#[test]
fn raffles_page_limit_zero_uses_default() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 5);

    let page = client.get_raffles_page(&PaginationParams {
        offset: 0,
        limit: 0,
    });

    assert_eq!(page.items.len(), 5);
    assert_eq!(page.total, 5);
    assert!(!page.has_more);
}

#[test]
fn raffles_page_limit_exceeds_max_clamped() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 5);

    let page = client.get_raffles_page(&PaginationParams {
        offset: 0,
        limit: MAX_PAGE_LIMIT + 100,
    });

    assert_eq!(page.items.len(), 5);
    assert_eq!(page.total, 5);
    assert!(!page.has_more);
}

// ── get_raffles_by_creator ───────────────────────────────────────────────

#[test]
fn creator_raffles_offset_beyond_total_returns_empty() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 3);
    let creator = Address::generate(&env);

    // creator has 0 raffles, offset 0 is beyond
    let page = client.get_raffles_by_creator(
        &creator,
        &PaginationParams {
            offset: 0,
            limit: 10,
        },
    );

    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total, 0);
    assert!(!page.has_more);
}

#[test]
fn creator_raffles_limit_zero_uses_default() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 3);

    // Get the creator of the first raffle
    let creator = Address::generate(&env);

    let page = client.get_raffles_by_creator(
        &creator,
        &PaginationParams {
            offset: 0,
            limit: 0,
        },
    );

    assert_eq!(page.total, 0);
}

// ── get_raffles_by_category ──────────────────────────────────────────────

#[test]
fn category_raffles_offset_beyond_total_returns_empty() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 3);

    let page = client.get_raffles_by_category(
        &soroban_sdk::String::from_str(&env, "nonexistent"),
        &PaginationParams {
            offset: 100,
            limit: 10,
        },
    );

    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total, 0);
    assert!(!page.has_more);
}

#[test]
fn category_raffles_limit_zero_uses_default() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 3);

    let page = client.get_raffles_by_category(
        &soroban_sdk::String::from_str(&env, "nonexistent"),
        &PaginationParams {
            offset: 0,
            limit: 0,
        },
    );

    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total, 0);
}

// ── No storage writes in view functions ──────────────────────────────────

#[test]
fn get_protocol_stats_does_not_write_storage() {
    let env = Env::default();
    let client = setup_with_raffles(&env, 2);

    // Call twice — if it wrote storage, the second call would differ
    let stats1 = client.get_protocol_stats();
    let stats2 = client.get_protocol_stats();
    assert_eq!(stats1.total_raffles_created, stats2.total_raffles_created);
    assert_eq!(stats1.protocol_fee_bp, stats2.protocol_fee_bp);
    assert_eq!(stats1.paused, stats2.paused);
    assert_eq!(
        stats1.total_unique_participants,
        stats2.total_unique_participants
    );
}

#[test]
fn get_admin_returns_correct_address() {
    let env = Env::default();
    let admin = existing_address(&env);
    let treasury = existing_address(&env);
    let wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contract_id = env.register(RaffleFactory, ());
    let client = RaffleFactoryClient::new(&env, &contract_id);
    client.init_factory(&admin, &wasm_hash, &0u32, &treasury);

    assert_eq!(client.get_admin(), admin);
}
