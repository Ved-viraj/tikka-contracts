use soroban_sdk::{xdr::ToXdr, Address, Bytes, BytesN, Env, Vec};

use raffle_shared::{
    constants::MIN_COMMITS_FOR_DRAW, CancelReason, FailureReason, QuorumConfig, RandomnessSource,
    RandomnessType,
};

use crate::events::{
    DrawTriggered, OracleSeedDelivered, RaffleCancelled, RaffleFailed, RandomnessFallbackTriggered,
    RandomnessReceived, RandomnessRequested,
};
use crate::helpers::{
    build_internal_seed_u64, do_finalize_with_seed, read_raffle, request_randomness,
    require_awaiting_randomness, revert_status, transition_status, transition_to_drawing,
};
use crate::randomness::{build_vrf_proof_message, derive_random_seed_from_proof};
use crate::{
    CommitRevealEntry, DataKey, Error, RaffleStatus, ORACLE_TIMEOUT_LEDGERS,
    RANDOMNESS_MIN_DELAY_LEDGERS,
};

pub(crate) fn finalize_raffle(env: Env) -> Result<(), Error> {
    let drawing_lock: bool = env
        .storage()
        .instance()
        .get(&DataKey::DrawingLock)
        .unwrap_or(false);
    if drawing_lock {
        // A randomness delivery is already in flight: an earlier `finalize`
        // call (or an `External` sell-out) dispatched the request and the
        // oracle/quorum has not answered yet.
        let pending: bool = env
            .storage()
            .instance()
            .get(&DataKey::RandomnessRequested)
            .unwrap_or(false);
        if pending {
            return Err(Error::DrawingAlreadyInProgress);
        }
        // No request in flight: the raffle merely auto-entered `Drawing`
        // when it sold out, and the creator may still finalize it here.
    }
    let mut raffle = read_raffle(&env)?;

    // Finalization is permissionless: the preconditions below (time_ended ||
    // tickets_full) are fully verifiable on chain, so anyone may call this once
    // they hold. Requiring creator auth let a creator stall a raffle that was
    // already contractually over, leaving buyers' funds escrowed with no path
    // out (refund_ticket needs Cancelled or Failed). #1000
    if raffle.status != RaffleStatus::Active && raffle.status != RaffleStatus::Drawing {
        return Err(Error::InvalidStatus);
    }

    let now = env.ledger().timestamp();
    // end_time is an exclusive boundary: sales/finalization are gated on
    // now < end_time, so the deadline is reached starting at now == end_time.
    // Must stay in sync with the RaffleExpired checks in tickets.rs
    // (buy_tickets, buy_tickets_for) and time_remaining in
    // views.rs::get_stats. See docs/GLOSSARY.md § "End Time".
    let time_ended = !raffle.no_deadline && now >= raffle.end_time;
    let tickets_full = raffle.tickets_sold >= raffle.max_tickets;

    if raffle.status == RaffleStatus::Active && !time_ended && !tickets_full {
        return Err(Error::InvalidStateTransition);
    }

    if raffle.tickets_sold == 0 || raffle.tickets_sold < raffle.min_tickets {
        let failure_reason = if raffle.tickets_sold == 0 {
            FailureReason::ZeroTicketsSold
        } else {
            FailureReason::MinTicketsNotMet
        };
        transition_status(&env, &mut raffle, RaffleStatus::Failed, now)?;
        RaffleFailed {
            creator: raffle.creator.clone(),
            reason: failure_reason,
            tickets_sold: raffle.tickets_sold,
            timestamp: now,
        }
        .publish(&env);
        return Ok(());
    }

    // `DrawTriggered.caller` keeps reporting the raffle creator. The SDK
    // exposes no invoker address (`Env::invoker` does not exist in
    // soroban-sdk 23.x), so now that finalization is permissionless there is no
    // trustworthy value for this field; the event schema is unchanged to avoid
    // breaking existing consumers. #1000
    let caller = raffle.creator.clone();
    let pre_status = raffle.status.clone();
    let already_drawing = pre_status == RaffleStatus::Drawing;
    if !already_drawing {
        transition_to_drawing(&env, &mut raffle, now)?;
    }

    // === Quorum fan-out ===
    if let RandomnessSource::Quorum(QuorumConfig { oracles, .. }) = &raffle.randomness_source {
        match request_randomness(&env) {
            Ok(request_id) => {
                DrawTriggered {
                    caller: caller.clone(),
                    total_tickets_sold: raffle.tickets_sold,
                    timestamp: now,
                }
                .publish(&env);

                // Emit RandomnessRequested for each oracle (fan-out)
                for i in 0..oracles.len() {
                    if let Some(addr) = oracles.get(i) {
                        RandomnessRequested {
                            oracle: addr,
                            request_id,
                            timestamp: now,
                        }
                        .publish(&env);
                    }
                }

                // Initialise empty quorum submission tracker
                env.storage()
                    .persistent()
                    .set(&DataKey::QuorumSubmittedOracles, &Vec::<Address>::new(&env));

                return Ok(());
            }
            Err(err) => {
                if !already_drawing {
                    revert_status(&env, &mut raffle, pre_status)?;
                }
                env.storage().instance().set(&DataKey::DrawingLock, &false);
                return Err(err);
            }
        }
    }

    // === External (single oracle) ===
    if raffle.randomness_source == RandomnessSource::External {
        match request_randomness(&env) {
            Ok(request_id) => {
                DrawTriggered {
                    caller: caller.clone(),
                    total_tickets_sold: raffle.tickets_sold,
                    timestamp: now,
                }
                .publish(&env);
                RandomnessRequested {
                    oracle: raffle
                        .oracle_address
                        .clone()
                        .unwrap_or(env.current_contract_address()),
                    request_id,
                    timestamp: now,
                }
                .publish(&env);
                return Ok(());
            }
            Err(err) => {
                if !already_drawing {
                    revert_status(&env, &mut raffle, pre_status)?;
                }
                env.storage().instance().set(&DataKey::DrawingLock, &false);
                return Err(err);
            }
        }
    }

    DrawTriggered {
        caller: caller.clone(),
        total_tickets_sold: raffle.tickets_sold,
        timestamp: now,
    }
    .publish(&env);

    if raffle.randomness_source == RandomnessSource::CommitReveal {
        // Seed is derived from **revealed pre-images only** (#989).  A commit
        // that is never opened contributes nothing, so it cannot be ground
        // offline to steer the winner selection.
        let mut combined = Bytes::new(&env);
        let mut reveals: u32 = 0;
        for ticket_id in 1..=raffle.tickets_sold {
            if let Some(entry) = env
                .storage()
                .persistent()
                .get::<_, CommitRevealEntry>(&DataKey::CommitEntry(ticket_id))
            {
                if let Some(preimage) = entry.revealed {
                    combined.extend_from_array(&preimage.to_array());
                    reveals += 1;
                }
            }
        }

        if reveals >= MIN_COMMITS_FOR_DRAW {
            let hash: BytesN<32> = env.crypto().sha256(&combined).into();
            let arr = hash.to_array();
            let mut seed_bytes = [0u8; 8];
            seed_bytes.copy_from_slice(&arr[..8]);
            let seed = u64::from_be_bytes(seed_bytes);
            return do_finalize_with_seed(&env, raffle, seed, RandomnessType::Prng, None);
        }

        // Below the documented minimum: a lone participant could have ground
        // their commitment, so fall back to the internal seed.
        let seed = build_internal_seed_u64(&env);
        RandomnessFallbackTriggered {
            triggered_by: raffle.creator.clone(),
            seed_used: seed,
            request_ledger: 0,
            fallback_ledger: env.ledger().sequence(),
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);
        return do_finalize_with_seed(&env, raffle, seed, RandomnessType::Fallback, None);
    }

    let seed = build_internal_seed_u64(&env);
    do_finalize_with_seed(&env, raffle, seed, RandomnessType::Prng, None)
}

/// Remove every quorum commit/reveal entry so a re-draw starts from an empty
/// submitter set (#988).
///
/// Clears `QuorumSeed`, `QuorumCommit` and `QuorumSubmittedOracles`.
pub(crate) fn clear_quorum_storage(env: &Env) {
    if let Some(submitted) = env
        .storage()
        .persistent()
        .get::<_, Vec<Address>>(&DataKey::QuorumSubmittedOracles)
    {
        for i in 0..submitted.len() {
            if let Some(addr) = submitted.get(i) {
                env.storage()
                    .persistent()
                    .remove(&DataKey::QuorumSeed(addr.clone()));
                env.storage()
                    .persistent()
                    .remove(&DataKey::QuorumCommit(addr.clone()));
            }
        }
    }
    env.storage()
        .persistent()
        .remove(&DataKey::QuorumSubmittedOracles);
}

/// Handle a single-oracle VRF randomness submission (existing External mode).
pub(crate) fn provide_randomness(
    env: Env,
    random_seed: u64,
    public_key: BytesN<32>,
    proof: BytesN<64>,
    request_id: u64,
) -> Result<Address, Error> {
    let drawing_lock: bool = env
        .storage()
        .instance()
        .get(&DataKey::DrawingLock)
        .unwrap_or(false);
    if !drawing_lock {
        return Err(Error::DrawingAlreadyComplete);
    }

    let raffle = read_raffle(&env)?;

    // Reject Quorum-mode submissions on this path
    if matches!(raffle.randomness_source, RandomnessSource::Quorum(_)) {
        return Err(Error::InvalidParameters);
    }

    let oracle = match &raffle.oracle_address {
        Some(addr) => {
            let addr_ref: &Address = addr;
            addr_ref.require_auth();
            addr.clone()
        }
        None => return Err(Error::OracleNotSet),
    };

    // Shared lifecycle guard with the quorum callback (#987).
    require_awaiting_randomness(&env, &raffle, request_id)?;

    let req_ledger: u32 = env
        .storage()
        .instance()
        .get(&DataKey::RandomnessRequestLedger)
        .unwrap_or(0);
    if env.ledger().sequence() < req_ledger.saturating_add(RANDOMNESS_MIN_DELAY_LEDGERS) {
        return Err(Error::RandomnessTooEarly);
    }

    let derived_seed = derive_random_seed_from_proof(&env, &proof);
    if derived_seed != random_seed {
        return Err(Error::InvalidParameters);
    }

    // FIX(#985): bind the submitted public_key to the oracle's registered key.
    // Without this check the ed25519_verify below only proves "this proof matches
    // THIS key" — it never proved the key belongs to the trusted oracle.  An
    // adversary (even the registered oracle) could supply a throwaway keypair
    // whose proof SHA-256 hashes to a seed that makes their own ticket win.
    if let Some(stored_key) = &raffle.oracle_public_key {
        if public_key != *stored_key {
            return Err(Error::OraclePublicKeyMismatch);
        }
    }
    // If no key was stored (legacy raffle created before #985), we fall
    // through to the signature check — better than silently accepting anything.

    let message = build_vrf_proof_message(&env, request_id);
    env.crypto().ed25519_verify(&public_key, &message, &proof);

    RandomnessReceived {
        oracle,
        seed: random_seed,
        request_id,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);
    do_finalize_with_seed(&env, raffle, random_seed, RandomnessType::Vrf, None)?;
    Ok(env.current_contract_address())
}

pub(crate) fn trigger_randomness_fallback(
    env: Env,
    caller: Address,
    do_cancel: bool,
) -> Result<(), Error> {
    // NOTE: the drawing lock is intentionally *not* checked here. It is set
    // whenever a randomness request is in flight, which is exactly the state
    // the fallback exists to recover from; the status / pending-request /
    // timeout checks below are the real guards.
    caller.require_auth();
    let mut raffle = read_raffle(&env)?;

    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotAuthorized)?;
    if caller != raffle.creator && caller != admin {
        return Err(Error::NotAuthorized);
    }
    if raffle.status != RaffleStatus::Drawing {
        return Err(Error::InvalidStateTransition);
    }

    let pending: bool = env
        .storage()
        .instance()
        .get(&DataKey::RandomnessRequested)
        .unwrap_or(false);
    if !pending {
        return Err(Error::NoRandomnessRequest);
    }

    let req_ledger: u32 = env
        .storage()
        .instance()
        .get(&DataKey::RandomnessRequestLedger)
        .unwrap_or(0);
    if env.ledger().sequence() < req_ledger.saturating_add(ORACLE_TIMEOUT_LEDGERS) {
        return Err(Error::FallbackTooEarly);
    }

    if do_cancel {
        transition_status(
            &env,
            &mut raffle,
            RaffleStatus::Cancelled,
            env.ledger().timestamp(),
        )?;
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequested);
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequestId);
        env.storage()
            .instance()
            .remove(&DataKey::RandomnessRequestLedger);
        env.storage().instance().set(&DataKey::DrawingLock, &false);
        clear_quorum_storage(&env);
        RaffleCancelled {
            creator: raffle.creator.clone(),
            reason: CancelReason::OracleTimeout,
            tickets_sold: raffle.tickets_sold,
            prize_refunded: false,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);
        return Ok(());
    }

    let seed = build_internal_seed_u64(&env);
    RandomnessFallbackTriggered {
        triggered_by: caller,
        seed_used: seed,
        request_ledger: req_ledger,
        fallback_ledger: env.ledger().sequence(),
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);

    clear_quorum_storage(&env);
    do_finalize_with_seed(&env, raffle, seed, RandomnessType::Fallback, None)
}

/// Extract the k-of-n quorum configuration of a raffle.
fn quorum_config(raffle: &crate::Raffle) -> Result<(u32, Vec<Address>), Error> {
    match &raffle.randomness_source {
        RandomnessSource::Quorum(QuorumConfig { k, oracles }) => Ok((*k, oracles.clone())),
        _ => Err(Error::InvalidParameters),
    }
}

/// Whether `oracle` is listed in the raffle's quorum.
fn is_registered(oracles: &Vec<Address>, oracle: &Address) -> bool {
    for i in 0..oracles.len() {
        if let Some(addr) = oracles.get(i) {
            if addr == *oracle {
                return true;
            }
        }
    }
    false
}

/// `sha256(seed_be || oracle_address_xdr || request_id_be)` — the commitment
/// an oracle must publish in phase 1 before it may reveal `seed`.
#[allow(dead_code)]
fn quorum_commit_hash(env: &Env, oracle: &Address, seed: u64, request_id: u64) -> BytesN<32> {
    let mut preimage = Bytes::new(env);
    preimage.extend_from_array(&seed.to_be_bytes());
    preimage.append(&oracle.clone().to_xdr(env));
    preimage.extend_from_array(&request_id.to_be_bytes());
    env.crypto().sha256(&preimage).into()
}

/// Oracles recorded by [`provide_quorum_commit`], in commit order.
fn committed_oracles(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::QuorumSubmittedOracles)
        .unwrap_or_else(|| Vec::new(env))
}

/// Phase 1 of the quorum protocol (#986): record a blinded seed commitment.
///
/// Only the digest touches storage here, so an oracle submitting later learns
/// nothing about the seeds already committed — which is what used to let the
/// k-th submitter steer the aggregate.
pub(crate) fn provide_quorum_commit(
    env: Env,
    oracle: Address,
    commit: BytesN<32>,
    request_id: u64,
) -> Result<(), Error> {
    oracle.require_auth();

    let raffle = read_raffle(&env)?;
    require_awaiting_randomness(&env, &raffle, request_id)?;

    let (_, oracles) = quorum_config(&raffle)?;
    if !is_registered(&oracles, &oracle) {
        return Err(Error::OracleNotRegistered);
    }

    let commit_key = DataKey::QuorumCommit(oracle.clone());
    if env.storage().persistent().has(&commit_key) {
        return Err(Error::DuplicateOracleSubmission);
    }
    env.storage().persistent().set(&commit_key, &commit);

    let mut submitted = committed_oracles(&env);
    submitted.push_back(oracle);
    env.storage()
        .persistent()
        .set(&DataKey::QuorumSubmittedOracles, &submitted);

    Ok(())
}

/// Phase 2 of the quorum protocol (#986): open a previously committed seed.
///
/// Rejects reveals that do not hash back to the stored commitment, and
/// rejects *any* reveal until `k` commitments are on record so that all
/// submitters commit before anybody sees a pre-image.  The raffle is
/// finalized on the k-th valid reveal.
#[allow(dead_code)]
pub(crate) fn provide_quorum_randomness(
    env: Env,
    oracle: Address,
    random_seed: u64,
    request_id: u64,
) -> Result<(), Error> {
    oracle.require_auth();

    let raffle = read_raffle(&env)?;
    require_awaiting_randomness(&env, &raffle, request_id)?;

    let (k, oracles) = quorum_config(&raffle)?;
    if !is_registered(&oracles, &oracle) {
        return Err(Error::OracleNotRegistered);
    }

    let commit: BytesN<32> = env
        .storage()
        .persistent()
        .get(&DataKey::QuorumCommit(oracle.clone()))
        .ok_or(Error::MissingCommit)?;

    let submitted = committed_oracles(&env);
    if (submitted.len() as u32) < k {
        return Err(Error::TooFewCommits);
    }

    if quorum_commit_hash(&env, &oracle, random_seed, request_id) != commit {
        return Err(Error::CommitMismatch);
    }

    let seed_key = DataKey::QuorumSeed(oracle.clone());
    if env.storage().persistent().has(&seed_key) {
        return Err(Error::DuplicateOracleSubmission);
    }
    env.storage().persistent().set(&seed_key, &random_seed);

    // Reveals in commit order; unrevealed commits are skipped.
    let mut seeds: Vec<(Address, u64)> = Vec::new(&env);
    for i in 0..submitted.len() {
        if let Some(addr) = submitted.get(i) {
            if let Some(seed) = env
                .storage()
                .persistent()
                .get::<_, u64>(&DataKey::QuorumSeed(addr.clone()))
            {
                seeds.push_back((addr.clone(), seed));
            }
        }
    }
    let count = seeds.len() as u32;

    OracleSeedDelivered {
        oracle,
        seed: random_seed,
        request_id,
        current_count: count,
        threshold: k,
        timestamp: env.ledger().timestamp(),
    }
    .publish(&env);

    if count >= k {
        let aggregate = crate::randomness::aggregate_quorum_seeds(&env, request_id, &seeds);
        do_finalize_with_seed(&env, raffle, aggregate, RandomnessType::Quorum, Some(seeds))?;
    }

    Ok(())
}

/// Open a commit-reveal pre-image recorded while the raffle was active.
pub(crate) fn reveal_commit(env: Env, ticket_id: u32, preimage: BytesN<32>) -> Result<(), Error> {
    let raffle = read_raffle(&env)?;
    if raffle.status != RaffleStatus::Active && raffle.status != RaffleStatus::Drawing {
        return Err(Error::InvalidStatus);
    }

    let key = DataKey::CommitEntry(ticket_id);
    let entry: CommitRevealEntry = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::MissingCommit)?;
    if entry.revealed.is_some() {
        return Err(Error::CommitAlreadySubmitted);
    }

    let digest: BytesN<32> = env
        .crypto()
        .sha256(&Bytes::from_array(&env, &preimage.to_array()))
        .into();
    if digest != entry.hash {
        return Err(Error::CommitMismatch);
    }

    env.storage().persistent().set(
        &key,
        &CommitRevealEntry {
            committer: entry.committer,
            hash: entry.hash,
            revealed: Some(preimage),
        },
    );

    Ok(())
}
