//! Binary MonthlyHalt mechanism (v3 policy).
//!
//! A latching state machine that tracks whether the system has entered
//! MonthlyHalt after reaching the 4% monthly drawdown limit.
//!
//! # States
//!
//! ```text
//! Active ⟷ MonthlyHalt
//!     │          │
//!     │          └── blocks arm, signal, approval resume
//!     │              triggers immediate close of open positions
//!     │
//!     └── Normal operation
//! ```
//!
//! | State        | blocks_new_entries | blocks_signals | Trailing stops |
//! |--------------|--------------------|----------------|----------------|
//! | Active       | No                 | No             | Yes            |
//! | MonthlyHalt  | Yes                | Yes            | Yes            |
//!
//! # Design Decisions
//!
//! - **Binary, not a ladder.** No escalation levels. The system is either
//!   active or halted. There is no Warning, SoftHalt, or HardHalt.
//! - **MonthlyHalt closes positions.** When the 4% monthly drawdown limit is
//!   reached, all open positions are closed immediately using the existing exit
//!   logic (without re-entering `entry_flow_lock`).
//! - **No automatic reset.** MonthlyHalt persists until next calendar month or
//!   explicit operator acknowledgment. There is no `/monthly-halt/reset`
//!   endpoint — unblocking the month without policy evidence is not permitted.
//! - **Manual trigger allowed.** An operator may trigger MonthlyHalt
//!   conservatively via `POST /monthly-halt` if they judge conditions unsafe.
//! - **Thread-safe.** Inner `RwLock` allows reads from `&self` contexts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// =============================================================================
// Public types
// =============================================================================

/// Binary state: either active or halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltState {
    /// Normal operation.
    Active,
    /// Monthly drawdown limit reached (4%). All new entries blocked; positions
    /// closed.
    MonthlyHalt,
}

impl HaltState {
    /// Returns true if this state prevents new position entries.
    pub fn blocks_new_entries(self) -> bool {
        matches!(self, HaltState::MonthlyHalt)
    }

    /// Returns true if this state prevents detector signal processing.
    pub fn blocks_signals(self) -> bool {
        matches!(self, HaltState::MonthlyHalt)
    }

    /// Human-readable description.
    pub fn description(self) -> &'static str {
        match self {
            HaltState::Active => "Normal operation",
            HaltState::MonthlyHalt => {
                "MonthlyHalt — 4% drawdown limit reached; new entries blocked; positions closed"
            },
        }
    }
}

impl std::fmt::Display for HaltState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HaltState::Active => write!(f, "Active"),
            HaltState::MonthlyHalt => write!(f, "MonthlyHalt"),
        }
    }
}

/// Serializable snapshot returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthlyHaltSnapshot {
    pub state: HaltState,
    pub description: &'static str,
    pub reason: Option<String>,
    pub triggered_at: Option<DateTime<Utc>>,
    /// Whether new position arm/entry is currently blocked.
    pub blocks_new_entries: bool,
    /// Whether detector signal processing is currently blocked.
    pub blocks_signals: bool,
}

// =============================================================================
// Inner state
// =============================================================================

#[derive(Debug, Clone)]
struct State {
    state: HaltState,
    reason: Option<String>,
    triggered_at: Option<DateTime<Utc>>,
}

impl State {
    fn active() -> Self {
        Self {
            state: HaltState::Active,
            reason: None,
            triggered_at: None,
        }
    }

    fn snapshot(&self) -> MonthlyHaltSnapshot {
        MonthlyHaltSnapshot {
            state: self.state,
            description: self.state.description(),
            reason: self.reason.clone(),
            triggered_at: self.triggered_at,
            blocks_new_entries: self.state.blocks_new_entries(),
            blocks_signals: self.state.blocks_signals(),
        }
    }
}

// =============================================================================
// CircuitBreaker (retained name for struct; semantics are binary MonthlyHalt)
// =============================================================================

/// Runtime MonthlyHalt gate.
///
/// Wraps an inner `RwLock<State>` so it can be read/written from `&self`
/// contexts. Intended to be stored as `Arc<CircuitBreaker>` inside
/// `PositionManager`.
#[derive(Debug)]
pub struct CircuitBreaker {
    state: RwLock<State>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self { state: RwLock::new(State::active()) }
    }
}

impl CircuitBreaker {
    /// Current snapshot (non-blocking read).
    pub async fn snapshot(&self) -> MonthlyHaltSnapshot {
        self.state.read().await.snapshot()
    }

    /// Current state (non-blocking read).
    pub async fn level(&self) -> HaltState {
        self.state.read().await.state
    }

    /// Whether new entries are currently blocked.
    pub async fn blocks_new_entries(&self) -> bool {
        self.state.read().await.state.blocks_new_entries()
    }

    /// Whether detector signal processing is currently blocked.
    pub async fn blocks_signals(&self) -> bool {
        self.state.read().await.state.blocks_signals()
    }

    /// Trigger MonthlyHalt.
    ///
    /// Returns `Some(())` if the transition happened (was Active),
    /// `None` if already in MonthlyHalt (idempotent — no mutation, no event).
    pub async fn trigger_halt(&self, reason: String) -> Option<()> {
        let mut state = self.state.write().await;
        if state.state == HaltState::MonthlyHalt {
            return None;
        }
        state.state = HaltState::MonthlyHalt;
        state.reason = Some(reason);
        state.triggered_at = Some(Utc::now());
        Some(())
    }

    /// Reset to Active (operator action).
    ///
    /// Returns `Some(())` if the transition happened (was MonthlyHalt),
    /// `None` if already Active (idempotent).
    pub async fn reset(&self) -> Option<()> {
        let mut state = self.state.write().await;
        if state.state == HaltState::Active {
            return None;
        }
        *state = State::active();
        Some(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_state_is_active() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.level().await, HaltState::Active);
        assert!(!cb.blocks_new_entries().await);
        assert!(!cb.blocks_signals().await);
    }

    #[tokio::test]
    async fn test_trigger_halt_blocks_entries_and_signals() {
        let cb = CircuitBreaker::default();
        let result = cb.trigger_halt("4% monthly limit".into()).await;
        assert_eq!(result, Some(()));
        assert!(cb.blocks_new_entries().await);
        assert!(cb.blocks_signals().await);
    }

    #[tokio::test]
    async fn test_trigger_halt_is_idempotent() {
        let cb = CircuitBreaker::default();
        cb.trigger_halt("first".into()).await;
        let result = cb.trigger_halt("second".into()).await;
        assert_eq!(result, None);
        let snap = cb.snapshot().await;
        assert_eq!(snap.reason.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn test_reset_is_idempotent() {
        let cb = CircuitBreaker::default();
        let result = cb.reset().await;
        assert_eq!(result, None);
        assert_eq!(cb.level().await, HaltState::Active);
    }

    #[tokio::test]
    async fn test_reset_returns_to_active() {
        let cb = CircuitBreaker::default();
        cb.trigger_halt("limit".into()).await;
        let result = cb.reset().await;
        assert_eq!(result, Some(()));
        assert_eq!(cb.level().await, HaltState::Active);
        assert!(!cb.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_snapshot_contains_correct_fields() {
        let cb = CircuitBreaker::default();
        cb.trigger_halt("4% reached".into()).await;

        let snap = cb.snapshot().await;
        assert_eq!(snap.state, HaltState::MonthlyHalt);
        assert_eq!(snap.reason.as_deref(), Some("4% reached"));
        assert!(snap.triggered_at.is_some());
        assert!(snap.blocks_new_entries);
        assert!(snap.blocks_signals);
    }
}
