use super::*;
use raffle_instance::RaffleInstanceClient;

/// List 20_000, bundle of 5 at 15_000, early-bird 50% of 6 tickets at 10%.
/// A purchase of 5 straddles the early-bird window; the sixth ticket is list price.
const LIST: i128 = 20_000;
const BUNDLE: i128 = 15_000;

fn pricing_config(env: &Env, payment_token: &Address) -> RaffleConfig {
    RaffleConfigBuilder::new(env, payment_token.clone())
        .description(String::from_str(env, "priced"))
        .max_tickets(6)
        .max_tickets_per_tx(6)
        .ticket_price(LIST)
        .prize_amount(LIST)
        .prizes(SdkVec::from_array(env, [10_000u32]))
        .early_bird_ticket_percentage(50)
        .early_bird_discount_bp(1_000)
        .bundles(SdkVec::from_array(
            env,
            [TicketBundle {
                quantity: 5,
                price_per_ticket: BUNDLE,
            }],
        ))
        .metadata_hash(BytesN::from_array(env, &[1u8; 32]))
        .build()
        .expect("priced config")
}

fn sum_price_paid(env: &Env, raffle: &Address, sold: u32) -> i128 {
    env.as_contract(raffle, || {
        let mut total = 0i128;
        for id in 1..=sold {
            let ticket: Ticket = env
                .storage()
                .persistent()
                .get(&raffle_instance::DataKey::Ticket(id))
                .unwrap();
            total += ticket.price_paid;
        }
        total
    })
}

#[test]
fn gross_revenue_matches_price_paid_with_bundles_and_early_bird() {
    let env = Env::default();
    env.mock_all_auths();
    let (factory, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let buyer = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let payment_token = sac.address();
    let token = StellarAssetClient::new(&env, &payment_token);
    token.mint(&creator, &(LIST * 10));
    token.mint(&buyer, &(LIST * 10));

    let raffle_address = factory.create_raffle(&creator, &pricing_config(&env, &payment_token));
    let raffle = RaffleInstanceClient::new(&env, &raffle_address);
    raffle.deposit_prize();
    raffle.buy_tickets(&buyer, &5);
    raffle.buy_tickets(&buyer, &1);

    let stats = raffle.get_stats();
    let paid = sum_price_paid(&env, &raffle_address, stats.tickets_sold);
    // 5 bundle tickets: 3 early-bird at 15_000 - 10%, 2 at 15_000, then 1 at list.
    // 3 * 13_500 + 2 * 15_000 + 20_000 = 90_500.
    assert_eq!(paid, 90_500);
    assert_eq!(stats.gross_revenue, paid);
    assert_ne!(stats.gross_revenue, stats.tickets_sold as i128 * LIST);
}

#[test]
fn leaderboard_volume_equals_tokens_collected() {
    let env = Env::default();
    env.mock_all_auths();
    let (factory, _admin, _treasury) = setup_factory(&env);
    let creator = Address::generate(&env);
    let buyer = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let payment_token = sac.address();
    let token = StellarAssetClient::new(&env, &payment_token);
    token.mint(&creator, &(LIST * 10));
    token.mint(&buyer, &(LIST * 10));

    let raffle_address = factory.create_raffle(&creator, &pricing_config(&env, &payment_token));
    let raffle = RaffleInstanceClient::new(&env, &raffle_address);
    raffle.deposit_prize();
    raffle.buy_tickets(&buyer, &5);
    raffle.buy_tickets(&buyer, &1);
    raffle.finalize_raffle();

    let paid = sum_price_paid(&env, &raffle_address, 6);
    let board = factory.get_leaderboard(&LeaderboardMetric::TotalVolume);
    assert_eq!(board.len(), 1);
    let entry = board.get(0).unwrap();
    assert_eq!(entry.0, raffle_address);
    assert_eq!(entry.1, paid);
}
