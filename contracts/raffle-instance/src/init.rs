//! Raffle-instance initialisation and prize-deposit logic.
//!
//! This module contains the two functions that move a raffle from *nothing*
//! into a state where ticket sales can begin:
//!
//! 1. [`init`] — called once by the factory immediately after deployment.
//!    Validates every field of [`RaffleConfig`], writes the [`Raffle`] to
//!    instance storage, and emits [`events::RaffleCreated`].
//!
//! 2. [`deposit_prize`] — called by the creator after `init`.  Transfers the
//!    prize amount from the creator's wallet into the contract and transitions
//!    the raffle from [`RaffleStatus::PendingPrize`] to
//!    [`RaffleStatus::Active`], opening ticket sales.
//!
//! ## Raffle lifecycle
//!
//! ```text
//! [deploy] ──init()──► PendingPrize ──deposit_prize()──► Active
//!                                                           │
//!                                                  ticket sales open
//! ```
//!
//! See [`docs/RANDOMNESS.md`](../../../docs/RANDOMNESS.md) for a full
//! explanation of the three randomness modes that can be configured here, and
//! [`docs/EVENTS.md`](../../../docs/EVENTS.md) for the events emitted by
//! these functions.

use soroban_sdk::{token, Address, Env};

use raffle_shared::config_builder::{validate_config, ConfigValidationError};
use raffle_shared::constants::{
    DEFAULT_CLAIM_EXPIRY_SECONDS, DEFAULT_CLAIM_LOCKUP_SECONDS, DEFAULT_SWAP_DEADLINE_SECONDS,
};
use raffle_shared::RaffleConfig;

use crate::events::{PrizeDeposited, RaffleCreated};
use crate::{
    helpers::{read_raffle, require_not_paused, transition_status},
    write_raffle, DataKey, Error, Raffle, RaffleStatus,
};

/// Initialise a freshly-deployed raffle-instance contract.
///
/// Called exclusively by the factory as the final step of
/// [`create_raffle`](raffle_factory::RaffleFactory::create_raffle).  It must
/// never be called more than once — a second call returns
/// [`Error::AlreadyInitialized`].
///
/// # Validation performed
///
/// | Field | Rule |
/// |---|---|
/// | `description` | `len ≤ MAX_DESCRIPTION_LENGTH` (1 000 bytes) |
/// | `end_time` | Must be in the future unless `no_deadline = true`; `no_deadline` requires `end_time == 0` |
/// | `max_tickets` | `1 ≤ max_tickets ≤ MAX_TICKETS_LIMIT` (100 000) |
/// | `max_tickets_per_tx` | `1 ≤ max_tickets_per_tx ≤ max_tickets` |
/// | `max_tickets_per_address` | `≤ max_tickets`; `0` = unlimited. Enforced per-address at purchase time (both `buy_tickets` and `buy_tickets_for`) via `Error::ExceedsMaxTicketsPerAddress` |
/// | `min_tickets` | `min_tickets ≤ max_tickets` |
/// | `ticket_price` | `≥ MIN_TICKET_PRICE` (10 000 stroops) |
/// | `prize_amount` | `ticket_price ≤ prize_amount ≤ MAX_PRIZE_AMOUNT` |
/// | `prizes` | Non-empty, `len ≤ MAX_PRIZES` (100), basis-points sum == 10 000 |
/// | `protocol_fee_bp` | `≤ 10 000`; applied to ticket purchases and prize claims |
/// | `oracle_address` | Required (and not self) when `randomness_source == External`; forbidden otherwise |
/// | `metadata_hash` | Must not be the all-zero 32-byte value |
/// | `category` | See [`validate_category`] |
/// | `payment_token` | Must be a valid SAC (queried via `try_decimals`) |
/// | `claim_lockup_seconds` | `≤ MAX_CLAIM_LOCKUP_SECONDS` (7 days) after `resolve_defaults` |
/// | `claim_expiry_seconds` | `≥ MIN_CLAIM_EXPIRY_SECONDS` and greater than the claim lockup after `resolve_defaults` |
/// | `swap_deadline_seconds` | `≤ MAX_SWAP_DEADLINE_SECONDS` (3 600 s) after `resolve_defaults` |
///
/// # Parameters
///
/// - `factory` — The factory contract address, stored for relay-call
///   authorisation (`pause`, `wipe_storage`, etc.).
/// - `admin` — Privileged address stored for admin-only operations on this
///   instance.
/// - `creator` — Raffle creator; stored as the owner who must call
///   [`deposit_prize`] and may call [`cancel_raffle`].
/// - `config` — Full validated configuration.  `protocol_fee_bp` and
///   `treasury_address` will already have been overwritten by the factory.
///
/// # Errors
///
/// - [`Error::AlreadyInitialized`] — contract already has raffle state.
/// - [`Error::InvalidParameters`] — any validation rule above is violated.
/// - [`Error::InvalidTicketRange`] — `min_tickets > max_tickets`.
/// - [`Error::InvalidEndTime`] — `end_time` is non-zero but in the past.
/// - [`Error::TooManyPrizes`] — `prizes.len() > MAX_PRIZES`.
/// - [`Error::InvalidTokenAddress`] — `payment_token` is not a valid SAC.
///
/// # Events
///
/// Emits [`events::RaffleCreated`].
///
/// See also: [`docs/EVENTS.md`](../../../../docs/EVENTS.md) — `RaffleCreated`.
#[allow(dead_code)]
pub(crate) fn init(
    env: Env,
    factory: Address,
    admin: Address,
    creator: Address,
    mut config: RaffleConfig,
) -> Result<(), Error> {
    if env.storage().instance().has(&DataKey::Raffle) {
        return Err(Error::AlreadyInitialized);
    }

    let instance_address = env.current_contract_address();
    validate_config(&env, &mut config, Some(&instance_address)).map_err(|error| match error {
        ConfigValidationError::InvalidParameters => Error::InvalidParameters,
        ConfigValidationError::InvalidTicketRange => Error::InvalidTicketRange,
        ConfigValidationError::InvalidEndTime => Error::InvalidEndTime,
        ConfigValidationError::TooManyPrizes => Error::TooManyPrizes,
        ConfigValidationError::InvalidTokenAddress => Error::InvalidTokenAddress,
    })?;
    let claim_expiry = config
        .claim_expiry_seconds
        .unwrap_or(DEFAULT_CLAIM_EXPIRY_SECONDS);
    let claim_lockup = config
        .claim_lockup_seconds
        .unwrap_or(DEFAULT_CLAIM_LOCKUP_SECONDS);
    let swap_deadline = config
        .swap_deadline_seconds
        .unwrap_or(DEFAULT_SWAP_DEADLINE_SECONDS);

    let prize_token = config
        .prize_token
        .clone()
        .unwrap_or_else(|| config.payment_token.clone());

    let raffle = Raffle {
        creator: creator.clone(),
        description: config.description.clone(),
        end_time: config.end_time,
        no_deadline: config.no_deadline,
        max_tickets: config.max_tickets,
        max_tickets_per_tx: config.max_tickets_per_tx,
        min_tickets: config.min_tickets,
        allow_multiple: config.allow_multiple,
        max_tickets_per_address: config.max_tickets_per_address,
        ticket_price: config.ticket_price,
        payment_token: config.payment_token.clone(),
        prize_token: prize_token.clone(),
        prize_amount: config.prize_amount,
        prizes: config.prizes.clone(),
        tickets_sold: 0,
        status: RaffleStatus::PendingPrize,
        prize_deposited: false,
        winners: soroban_sdk::Vec::new(&env),
        claim_expiry_seconds: claim_expiry,
        randomness_source: config.randomness_source.clone(),
        oracle_address: config.oracle_address,
        oracle_public_key: config.oracle_public_key,
        protocol_fee_bp: config.protocol_fee_bp,
        treasury_address: config.treasury_address,
        swap_router: config.swap_router,
        tikka_token: config.tikka_token,
        finalized_at: None,
        claim_lockup_seconds: claim_lockup,
        swap_deadline_seconds: swap_deadline,
        ticket_sales_paused: false,
        early_bird_ticket_percentage: config.early_bird_ticket_percentage,
        early_bird_discount_bp: config.early_bird_discount_bp,
        metadata_hash: config.metadata_hash.clone(),
        unique_winners: config.unique_winners,
        nft_contract: config.nft_contract,
        bundles: config.bundles.clone(),
    };
    write_raffle(&env, &raffle);
    env.storage().instance().set(&DataKey::Factory, &factory);
    env.storage().instance().set(&DataKey::Admin, &admin);

    RaffleCreated {
        raffle_id: env.current_contract_address(),
        creator,
        end_time: config.end_time,
        max_tickets: config.max_tickets,
        ticket_price: config.ticket_price,
        payment_token: config.payment_token,
        prize_amount: config.prize_amount,
        prizes: config.prizes,
        description: config.description,
        randomness_source: config.randomness_source,
        metadata_hash: config.metadata_hash,
        unique_winners: config.unique_winners,
        claim_expiry_seconds: claim_expiry,
    }
    .publish(&env);

    Ok(())
}

/// Transfer the prize amount from the creator into the contract and open ticket
/// sales.
///
/// This is the second mandatory setup step after [`init`].  Until this
/// function succeeds, [`buy_tickets`] will return
/// [`Error::InvalidStateTransition`] because `prize_deposited` is `false`.
///
/// ## What this function does
///
/// 1. Checks the contract is not paused.
/// 2. Requires authorization from `raffle.creator`.
/// 3. Guards against a second deposit (`prize_deposited == true`).
/// 4. Calls `try_transfer` on the prize token to pull `prize_amount` from
///    the creator into this contract address.
/// 5. Sets `prize_deposited = true` and transitions status from
///    [`RaffleStatus::PendingPrize`] → [`RaffleStatus::Active`].
///
/// After this call succeeds, `buy_tickets` accepts purchases.
///
/// # Auth
///
/// Requires authorization from `raffle.creator`.
///
/// # Errors
///
/// - [`Error::ContractPaused`] — contract is paused.
/// - [`Error::NotInitialized`] — `init` has not been called yet.
/// - [`Error::PrizeAlreadyDeposited`] — prize was already deposited; calling
///   again is a no-op error to prevent double-funding.
/// - [`Error::TokenTransferFailed`] — the token `try_transfer` failed (e.g.
///   insufficient creator balance or missing allowance).
///
/// # Events
///
/// - [`events::PrizeDeposited`] — confirms the amount and token.
/// - [`events::RaffleStatusChanged`] — records the `PendingPrize → Active`
///   transition.
///
/// See also: [`docs/EVENTS.md`](../../../docs/EVENTS.md) — `PrizeDeposited`,
/// `RaffleStatusChanged`.
pub(crate) fn deposit_prize(env: Env) -> Result<(), Error> {
    require_not_paused(&env)?;
    let mut raffle = read_raffle(&env)?;
    raffle.creator.require_auth();

    if raffle.prize_deposited {
        return Err(Error::PrizeAlreadyDeposited);
    }

    let token_client = token::Client::new(&env, &raffle.prize_token);
    let _ = token_client
        .try_transfer(
            &raffle.creator,
            env.current_contract_address(),
            &raffle.prize_amount,
        )
        .map_err(|_| Error::TokenTransferFailed)?;

    raffle.prize_deposited = true;
    let ts = env.ledger().timestamp();
    transition_status(&env, &mut raffle, RaffleStatus::Active, ts)?;

    PrizeDeposited {
        creator: raffle.creator.clone(),
        amount: raffle.prize_amount,
        token: raffle.prize_token.clone(),
        timestamp: ts,
    }
    .publish(&env);

    Ok(())
}
