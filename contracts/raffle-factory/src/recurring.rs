//! # Recurring (Subscription) Raffle Subsystem
//!
//! This module manages recurring (subscription-based) raffle schedules.
//! It allows creators to configure automated series of raffles with a fixed
//! base configuration, interval, and round count without requiring repeated manual
//! configuration.
//!
//! ## State Machine
//!
//! ```text
//!                  create_recurring_raffle
//!                            │
//!                            ▼
//!               ┌─────────────────────────┐
//!               │         ACTIVE          │◄───────────────────────┐
//!               │    (active = true)      │                        │
//!               └────────────┬────────────┘                        │
//!                            │                                     │
//!           ┌────────────────┴────────────────┐                    │
//!           │                                 │                    │
//!           │ [ledger.timestamp >= next_due]  │ [creator or admin] │
//!           │ [current_round < max_rounds     │ cancel_recurring_  │
//!           │  OR max_rounds == 0]            │ raffle             │
//!           │                                 │                    │
//!           ▼                                 ▼                    │
//!   trigger_next_round               ┌─────────────────┐           │
//!   • Deploy new raffle instance     │    CANCELLED    │           │
//!   • Increment current_round        │ (active = false)│           │
//!   • Update next_due = now+interval └─────────────────┘           │
//!   • Emits RecurringRoundTriggered          │ (terminal)          │
//!           │                                                      │
//!           └──────────────────────────────────────────────────────┘
//! ```
//!
//! ### Round Lifecycle & Scheduling Semantics
//!
//! 1. **Creation (`create_recurring_raffle`)**:
//!    - A creator provides a [`RecurringRaffleConfig`].
//!    - The interval is strictly validated against protocol bounds:
//!      `MIN_RECURRING_INTERVAL_SECONDS` (1 hour) <= `interval_seconds` <= `MAX_RECURRING_INTERVAL_SECONDS` (365 days).
//!    - The schedule is initialized with `active = true`, `current_round = 0`, and
//!      `next_due = now + interval_seconds`.
//!    - Emits [`events::RecurringRaffleCreated`].
//!
//! 2. **Round Triggering (`trigger_next_round`)**:
//!    - **Authorisation: Permissionless.** Anyone (creators, bots, keepers, users)
//!      may trigger the next round once the interval has elapsed (`ledger.timestamp >= next_due`).
//!      This guarantees that recurring schedules do not stall if a creator goes offline.
//!    - Deploys a new raffle instance using the configured `base_config`.
//!    - Advances `current_round += 1` and schedules the next round at `next_due = now + interval_seconds`.
//!    - Appends the deployed raffle address to [`RecurringDataKey::RecurringRaffleInstances`].
//!    - Updates partner statistics if the creator is a whitelisted partner.
//!    - Emits [`events::RecurringRoundTriggered`].
//!
//! 3. **Max Rounds Semantics**:
//!    - `max_rounds == 0`: Represents an **infinite** recurring raffle. Rounds can be
//!      triggered indefinitely as long as the schedule remains active.
//!    - `max_rounds > 0`: Capped series. Once `current_round >= max_rounds`, subsequent
//!      calls to `trigger_next_round` return `ContractError::MaxRoundsReached`.
//!
//! 4. **Prize Funding**:
//!    - Prize funding is **not automatic**. When `trigger_next_round` deploys a new
//!      raffle instance, the instance begins in the `PendingPrize` status.
//!    - The creator (or authorized prize funder) is expected to call `deposit_prize`
//!      on the newly deployed raffle instance contract to transition it to `Active`
//!      and open ticket sales.
//!
//! 5. **Cancellation (`cancel_recurring_raffle`)**:
//!    - **Authorisation: Creator or Factory Admin.**
//!    - Transitions the schedule to `active = false`.
//!    - Future calls to `trigger_next_round` will fail with `ContractError::RecurringInactive`.
//!    - Existing raffle instances already deployed across past rounds remain unaffected.
//!    - Emits [`events::RecurringRaffleCancelled`].

use soroban_sdk::{contractimpl, contracttype, Address, Env, Vec};

use raffle_shared::constants::{MAX_RECURRING_INTERVAL_SECONDS, MIN_RECURRING_INTERVAL_SECONDS};
use raffle_shared::{RaffleConfigBuilder, RecurringRaffleConfig};

use crate::events;
use crate::registry::PartnerStats;
use crate::{ContractError, DataKey, RaffleFactory, RaffleFactoryArgs, RaffleFactoryClient};

/// On-chain state for a recurring raffle schedule.
#[derive(Clone)]
#[contracttype]
pub struct RecurringRaffleEntry {
    /// Creator and owner of the recurring schedule.
    pub creator: Address,
    /// Configuration template used for every round.
    pub config: RecurringRaffleConfig,
    /// Unix timestamp when the next round becomes eligible to trigger.
    pub next_due: u64,
    /// Number of rounds triggered so far.
    pub current_round: u32,
    /// Whether the schedule is active (`true`) or cancelled (`false`).
    pub active: bool,
    /// Address of the most recently deployed raffle instance, if any.
    pub last_raffle_address: Option<Address>,
}

/// Storage keys dedicated to the recurring raffle subsystem.
#[derive(Clone)]
#[contracttype]
pub enum RecurringDataKey {
    /// Recurring (subscription) raffle state by ID: `u32 -> RecurringRaffleEntry`.
    RecurringRaffle(u32),
    /// Monotonic counter assigned to the next recurring raffle ID.
    NextRecurringId,
    /// ID -> list of raffle instance addresses created across all rounds so far.
    RecurringRaffleInstances(u32),
}

#[contractimpl]
impl RaffleFactory {
    /// Create a new recurring raffle schedule.
    ///
    /// # Auth
    /// Requires authorization from `creator`.
    ///
    /// # Errors
    /// - [`ContractError::ContractPaused`] if the factory is paused.
    /// - [`ContractError::InvalidParameters`] if `interval_seconds` is outside
    ///   [`MIN_RECURRING_INTERVAL_SECONDS`]..=[`MAX_RECURRING_INTERVAL_SECONDS`].
    pub fn create_recurring_raffle(
        env: Env,
        creator: Address,
        config: RecurringRaffleConfig,
    ) -> Result<u32, ContractError> {
        creator.require_auth();
        crate::require_factory_not_paused(&env)?;

        if config.interval_seconds < MIN_RECURRING_INTERVAL_SECONDS
            || config.interval_seconds > MAX_RECURRING_INTERVAL_SECONDS
        {
            return Err(ContractError::InvalidParameters);
        }

        let recurring_id: u32 = env
            .storage()
            .persistent()
            .get(&RecurringDataKey::NextRecurringId)
            .unwrap_or(0u32);

        let now = env.ledger().timestamp();
        let interval = config.interval_seconds;
        let entry = RecurringRaffleEntry {
            creator: creator.clone(),
            config,
            next_due: now.saturating_add(interval),
            current_round: 0,
            active: true,
            last_raffle_address: None,
        };

        env.storage()
            .persistent()
            .set(&RecurringDataKey::RecurringRaffle(recurring_id), &entry);
        env.storage().persistent().set(
            &RecurringDataKey::NextRecurringId,
            &(recurring_id.saturating_add(1)),
        );
        env.storage().persistent().set(
            &RecurringDataKey::RecurringRaffleInstances(recurring_id),
            &Vec::<Address>::new(&env),
        );

        events::RecurringRaffleCreated {
            recurring_id,
            creator,
            interval_seconds: entry.config.interval_seconds,
            max_rounds: entry.config.max_rounds,
            next_due: entry.next_due,
            timestamp: now,
        }
        .publish(&env);

        Ok(recurring_id)
    }

    /// Advance a recurring raffle schedule by deploying the instance for the next round.
    ///
    /// # Auth
    /// **Permissionless.** Any caller (keeper, bot, participant, or creator) may trigger
    /// the next round once `now >= next_due`.
    ///
    /// # Errors
    /// - [`ContractError::ContractPaused`] if the factory is paused.
    /// - [`ContractError::RecurringNotFound`] if `recurring_id` does not exist.
    /// - [`ContractError::RecurringInactive`] if the schedule was cancelled.
    /// - [`ContractError::IntervalNotElapsed`] if the current timestamp is before `next_due`.
    /// - [`ContractError::MaxRoundsReached`] if `max_rounds > 0` and `current_round >= max_rounds`.
    pub fn trigger_next_round(env: Env, recurring_id: u32) -> Result<Address, ContractError> {
        crate::require_factory_not_paused(&env)?;

        let mut entry: RecurringRaffleEntry = env
            .storage()
            .persistent()
            .get(&RecurringDataKey::RecurringRaffle(recurring_id))
            .ok_or(ContractError::RecurringNotFound)?;

        if !entry.active {
            return Err(ContractError::RecurringInactive);
        }

        let now = env.ledger().timestamp();
        if now < entry.next_due {
            return Err(ContractError::IntervalNotElapsed);
        }

        if entry.config.max_rounds > 0 && entry.current_round >= entry.config.max_rounds {
            return Err(ContractError::MaxRoundsReached);
        }

        let config = RaffleConfigBuilder::from_config(&env, entry.config.base_config.clone())
            .build()
            .map_err(|_| ContractError::InvalidParameters)?;
        let raffle_address = crate::create_raffle_internal(&env, entry.creator.clone(), config)?;

        entry.current_round = entry.current_round.saturating_add(1);
        entry.next_due = now.saturating_add(entry.config.interval_seconds);
        entry.last_raffle_address = Some(raffle_address.clone());

        let mut instances: Vec<Address> = env
            .storage()
            .persistent()
            .get(&RecurringDataKey::RecurringRaffleInstances(recurring_id))
            .unwrap_or_else(|| Vec::new(&env));
        instances.push_back(raffle_address.clone());
        env.storage().persistent().set(
            &RecurringDataKey::RecurringRaffleInstances(recurring_id),
            &instances,
        );

        env.storage()
            .persistent()
            .set(&RecurringDataKey::RecurringRaffle(recurring_id), &entry);

        events::RecurringRoundTriggered {
            recurring_id,
            round: entry.current_round,
            raffle_address: raffle_address.clone(),
            next_due: entry.next_due,
            timestamp: now,
        }
        .publish(&env);

        // --- partner dashboard stats (#488) ---
        let creator = entry.creator.clone();
        let is_whitelisted = env
            .storage()
            .persistent()
            .has(&DataKey::WhitelistedPartner(creator.clone()));
        if is_whitelisted {
            let now = env.ledger().timestamp();
            let mut stats: PartnerStats = env
                .storage()
                .persistent()
                .get(&DataKey::PartnerStats(creator.clone()))
                .unwrap_or(PartnerStats {
                    total_raffles: 0,
                    total_volume: 0,
                    total_fees_generated: 0,
                    first_raffle_at: now,
                    latest_raffle_at: 0,
                });
            if stats.total_raffles == 0 {
                stats.first_raffle_at = now;
            }
            stats.total_raffles = stats.total_raffles.saturating_add(1);
            stats.latest_raffle_at = now;
            env.storage()
                .persistent()
                .set(&DataKey::PartnerStats(creator), &stats);
        }

        Ok(raffle_address)
    }

    /// Cancel an active recurring raffle schedule.
    ///
    /// # Auth
    /// Requires authorization from `caller`. `caller` must be either the schedule `creator`
    /// or the factory `admin`.
    ///
    /// # Errors
    /// - [`ContractError::RecurringNotFound`] if `recurring_id` does not exist.
    /// - [`ContractError::NotAuthorized`] if `caller` is neither the creator nor the admin.
    pub fn cancel_recurring_raffle(
        env: Env,
        recurring_id: u32,
        caller: Address,
    ) -> Result<(), ContractError> {
        let entry: RecurringRaffleEntry = env
            .storage()
            .persistent()
            .get(&RecurringDataKey::RecurringRaffle(recurring_id))
            .ok_or(ContractError::RecurringNotFound)?;

        if caller != entry.creator {
            let admin = crate::require_admin(&env)?;
            if caller != admin {
                return Err(ContractError::NotAuthorized);
            }
        }
        caller.require_auth();

        env.storage().persistent().set(
            &RecurringDataKey::RecurringRaffle(recurring_id),
            &RecurringRaffleEntry {
                active: false,
                ..entry
            },
        );

        events::RecurringRaffleCancelled {
            recurring_id,
            cancelled_by: caller,
            rounds_completed: entry.current_round,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(())
    }

    /// Retrieve the current configuration and progress of a recurring raffle schedule.
    pub fn get_recurring_raffle(env: Env, recurring_id: u32) -> Option<RecurringRaffleEntry> {
        env.storage()
            .persistent()
            .get(&RecurringDataKey::RecurringRaffle(recurring_id))
    }

    /// Retrieve all raffle instance addresses deployed by a recurring schedule.
    pub fn get_recurring_instances(env: Env, recurring_id: u32) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&RecurringDataKey::RecurringRaffleInstances(recurring_id))
            .unwrap_or_else(|| Vec::new(&env))
    }
}
