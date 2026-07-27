use soroban_sdk::{Env, Vec};

use raffle_shared::FairnessData;

use crate::{read_raffle, DataKey, Error, FairnessMetadata};

pub(crate) fn get_raffle(env: Env) -> Result<crate::Raffle, Error> {
    read_raffle(&env)
}

pub(crate) fn get_fairness_data(env: Env) -> Result<FairnessData, Error> {
    let meta: FairnessMetadata = env.storage().persistent().get(&DataKey::RandomnessSeed).ok_or(Error::InvalidStatus)?;
    let raffle = read_raffle(&env)?;
    let mut ticket_ids = Vec::new(&env);
    for i in 1..=raffle.tickets_sold { ticket_ids.push_back(i); }
    Ok(FairnessData {
        seed: meta.seed,
        randomness_source: meta.randomness_source,
        ticket_ids,
        winning_ticket_indices: meta.winning_ticket_indices,
        draw_timestamp: meta.draw_timestamp,
        draw_sequence: meta.draw_sequence,
        unique_winners: meta.unique_winners,
    })
}

pub(crate) fn is_paused(env: Env) -> bool {
    env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}

pub(crate) fn is_ticket_sales_paused(env: Env) -> bool {
    read_raffle(&env).map(|r| r.ticket_sales_paused).unwrap_or(false)
}

pub(crate) fn get_accumulated_fees(env: Env) -> i128 {
    env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0)
}
