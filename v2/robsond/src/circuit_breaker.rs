//! Circuit breaker escalation ladder (MIG-v2.5#5).
//!
//! A latching state machine that blocks new position entries when risk thresholds
//! are breached. Unlike the per-trade `RiskGate` check (which evaluates each trade
//! independently), the circuit breaker **persists across control loop cycles** and
//! remains active until an operator explicitly resets it.
//!
//! # Escalation Levels
//!
//! ```text
//!                              (operator only)
//!                                    ↓
//! Inactive → Warning → SoftHalt → HardHalt
//!                ↑          ↑
//!           (reserved,  (auto: daily
//!            not active  loss limit hit)
//!            yet)
//!
//! Any level → Inactive  (via /circuit-breaker/reset, operator only)
//! ```
//!
//! | Level    | Trigger                      | blocks_new_entries | blocks_signals | Trailing stops |
//! |----------|------------------------------|-------------------|----------------|----------------|
//! | Inactive | —                            | No                | No             | Yes            |
//! | Warning  | reserved — not yet triggered | No                | No             | Yes            |
//! | SoftHalt | Daily loss limit exceeded    | Yes               | Yes            | Yes            |
//! | HardHalt | Operator escalation only     | Yes               | Yes            | Yes            |
//!
//! # Design Decisions
//!
//! - **Automatic escalation: Inactive→SoftHalt only.** The system auto-escalates to
//!   SoftHalt when the `DailyLossLimit` risk check fires. The Warning level is reserved
//!   for a future 70%-threshold trigger and is not auto-triggered in the current tree.
//!   HardHalt is operator-only via `POST /circuit-breaker/escalate`.
//! - **Downward transitions require explicit operator reset** (`POST /circuit-breaker/reset`).
//! - **SoftHalt does not close positions.** Existing positions continue to trail stops
//!   and can exit normally. New arm, signal, and approval-resume are all blocked.
//! - **HardHalt blocks new entries and signal processing**, but does NOT block
//!   market-data driven trailing-stop updates and exits on existing positions.
//!   This is intentional: stopping trailing stops under HardHalt would leave open
//!   positions unprotected. To close existing positions, the operator must call
//!   `/panic` explicitly; circuit breaker reset alone does not close positions.
//! - **Thread-safe.** Inner `RwLock` allows reads from `&self` contexts, writes only
//!   during escalation and reset.
//! - **Idempotent operator actions.** `escalate_to_hard_halt` and `reset` are no-ops
//!   (and do not emit events) when already at the target level.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// The `RiskCheck::name()` string for the daily loss limit check.
///
/// Used in `PositionManager` to identify `DailyLossLimit` denials without a
/// direct dependency on the `robson-engine` crate from within `circuit_breaker.rs`.
/// If `RiskCheck::name()` is ever renamed in `robson-engine`, this constant must
/// be updated to match.
pub const DAILY_LOSS_LIMIT_CHECK_NAME: &str = "daily_loss_limit";

// =============================================================================
// Public types
// =============================================================================

/// The four escalation levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerLevel {
    /// Normal operation.
    Inactive,
    /// Approaching daily loss limit (≥70%). Alerting only — trading continues.
    Warning,
    /// Daily loss limit exceeded. New entries blocked; existing positions managed.
    SoftHalt,
    /// Operator-escalated. All trading blocked. Requires explicit reset.
    HardHalt,
}

impl CircuitBreakerLevel {
    /// Returns true if the level prevents new position entries.
    pub fn blocks_new_entries(self) -> bool {
        matches!(self, CircuitBreakerLevel::SoftHalt | CircuitBreakerLevel::HardHalt)
    }

    /// Returns true if the level prevents detector signal processing.
    ///
    /// True for `SoftHalt` and `HardHalt`: a detector signal that would transition
    /// an Armed position to Entering is a new entry, and is blocked at the same
    /// levels as `blocks_new_entries`. Market-data driven trailing-stop updates and
    /// exits on existing positions are NOT blocked at any level — use `/panic` to
    /// close existing positions explicitly.
    pub fn blocks_signals(self) -> bool {
        matches!(self, CircuitBreakerLevel::SoftHalt | CircuitBreakerLevel::HardHalt)
    }

    /// Human-readable description.
    pub fn description(self) -> &'static str {
        match self {
            CircuitBreakerLevel::Inactive => "Normal operation",
            CircuitBreakerLevel::Warning => "Approaching daily loss limit — new entries still allowed",
            CircuitBreakerLevel::SoftHalt => "Daily loss limit exceeded — new entries blocked",
            CircuitBreakerLevel::HardHalt => "Hard halt — new entries and signals blocked; trailing stops continue; operator reset required",
        }
    }
}

impl std::fmt::Display for CircuitBreakerLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerLevel::Inactive => write!(f, "Inactive"),
            CircuitBreakerLevel::Warning => write!(f, "Warning"),
            CircuitBreakerLevel::SoftHalt => write!(f, "SoftHalt"),
            CircuitBreakerLevel::HardHalt => write!(f, "HardHalt"),
        }
    }
}

/// Serializable snapshot returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerSnapshot {
    pub level: CircuitBreakerLevel,
    pub description: &'static str,
    pub reason: Option<String>,
    pub triggered_at: Option<DateTime<Utc>>,
    /// Whether new position arm/entry is currently blocked.
    pub blocks_new_entries: bool,
    /// Whether all trading activity is currently blocked.
    pub blocks_signals: bool,
}

// =============================================================================
// Inner state
// =============================================================================

#[derive(Debug, Clone)]
struct State {
    level: CircuitBreakerLevel,
    reason: Option<String>,
    triggered_at: Option<DateTime<Utc>>,
}

impl State {
    fn inactive() -> Self {
        Self { level: CircuitBreakerLevel::Inactive, reason: None, triggered_at: None }
    }

    fn snapshot(&self) -> CircuitBreakerSnapshot {
        CircuitBreakerSnapshot {
            level: self.level,
            description: self.level.description(),
            reason: self.reason.clone(),
            triggered_at: self.triggered_at,
            blocks_new_entries: self.level.blocks_new_entries(),
            blocks_signals: self.level.blocks_signals(),
        }
    }
}

// =============================================================================
// CircuitBreaker
// =============================================================================

/// Runtime circuit breaker.
///
/// Wraps an inner `RwLock<State>` so it can be read/written from `&self` contexts.
/// Intended to be stored as `Arc<CircuitBreaker>` inside `PositionManager`.
#[derive(Debug)]
pub struct CircuitBreaker {
    state: RwLock<State>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self { state: RwLock::new(State::inactive()) }
    }
}

impl CircuitBreaker {
    /// Current snapshot (non-blocking read).
    pub async fn snapshot(&self) -> CircuitBreakerSnapshot {
        self.state.read().await.snapshot()
    }

    /// Current level (non-blocking read).
    pub async fn level(&self) -> CircuitBreakerLevel {
        self.state.read().await.level
    }

    /// Whether new entries are currently blocked.
    pub async fn blocks_new_entries(&self) -> bool {
        self.state.read().await.level.blocks_new_entries()
    }

    /// Whether all trading is currently blocked.
    pub async fn blocks_signals(&self) -> bool {
        self.state.read().await.level.blocks_signals()
    }

    /// Try to escalate to `target_level` if it is higher than the current level.
    ///
    /// Returns `Some(previous_level)` if escalation happened, `None` if already at or above.
    pub async fn try_escalate(
        &self,
        target: CircuitBreakerLevel,
        reason: String,
    ) -> Option<CircuitBreakerLevel> {
        let mut state = self.state.write().await;
        if target > state.level {
            let previous = state.level;
            state.level = target;
            state.reason = Some(reason);
            state.triggered_at = Some(Utc::now());
            Some(previous)
        } else {
            None
        }
    }

    /// Force-escalate to `HardHalt` (operator action).
    ///
    /// Returns `Some(previous_level)` if the level changed, `None` if already at `HardHalt`
    /// (idempotent — no state mutation, no event emitted on repeated calls).
    pub async fn escalate_to_hard_halt(&self, reason: String) -> Option<CircuitBreakerLevel> {
        let mut state = self.state.write().await;
        if state.level == CircuitBreakerLevel::HardHalt {
            return None;
        }
        let previous = state.level;
        state.level = CircuitBreakerLevel::HardHalt;
        state.reason = Some(reason);
        state.triggered_at = Some(Utc::now());
        Some(previous)
    }

    /// Reset to `Inactive` (operator action).
    ///
    /// Returns `Some(previous_level)` if the level changed, `None` if already `Inactive`
    /// (idempotent — no state mutation, no event emitted on repeated calls).
    pub async fn reset(&self) -> Option<CircuitBreakerLevel> {
        let mut state = self.state.write().await;
        if state.level == CircuitBreakerLevel::Inactive {
            return None;
        }
        let previous = state.level;
        *state = State::inactive();
        Some(previous)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_state_is_inactive() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.level().await, CircuitBreakerLevel::Inactive);
        assert!(!cb.blocks_new_entries().await);
        assert!(!cb.blocks_signals().await);
    }

    #[tokio::test]
    async fn test_escalate_to_soft_halt_blocks_entries_and_signals() {
        let cb = CircuitBreaker::default();
        let prev = cb.try_escalate(CircuitBreakerLevel::SoftHalt, "daily limit".into()).await;
        assert_eq!(prev, Some(CircuitBreakerLevel::Inactive));
        assert!(cb.blocks_new_entries().await);
        assert!(cb.blocks_signals().await, "SoftHalt must also block signals");
    }

    #[tokio::test]
    async fn test_escalate_to_hard_halt_blocks_all() {
        let cb = CircuitBreaker::default();
        let prev = cb.escalate_to_hard_halt("operator".into()).await;
        assert_eq!(prev, Some(CircuitBreakerLevel::Inactive));
        assert!(cb.blocks_new_entries().await);
        assert!(cb.blocks_signals().await);
    }

    #[tokio::test]
    async fn test_escalate_to_hard_halt_is_idempotent() {
        let cb = CircuitBreaker::default();
        cb.escalate_to_hard_halt("first".into()).await;
        // Second call when already HardHalt — must return None (no mutation)
        let result = cb.escalate_to_hard_halt("second".into()).await;
        assert_eq!(result, None);
        // Reason should still be from the first call
        let snap = cb.snapshot().await;
        assert_eq!(snap.reason.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn test_reset_is_idempotent() {
        let cb = CircuitBreaker::default();
        // Already Inactive — reset should be a no-op
        let result = cb.reset().await;
        assert_eq!(result, None);
        assert_eq!(cb.level().await, CircuitBreakerLevel::Inactive);
    }

    #[tokio::test]
    async fn test_no_downgrade_via_try_escalate() {
        let cb = CircuitBreaker::default();
        cb.try_escalate(CircuitBreakerLevel::HardHalt, "hard".into()).await;
        // Warning is below HardHalt — should not downgrade
        let result =
            cb.try_escalate(CircuitBreakerLevel::Warning, "warning".into()).await;
        assert_eq!(result, None);
        assert_eq!(cb.level().await, CircuitBreakerLevel::HardHalt);
    }

    #[tokio::test]
    async fn test_reset_returns_to_inactive() {
        let cb = CircuitBreaker::default();
        cb.try_escalate(CircuitBreakerLevel::SoftHalt, "limit".into()).await;
        let prev = cb.reset().await;
        assert_eq!(prev, Some(CircuitBreakerLevel::SoftHalt));
        assert_eq!(cb.level().await, CircuitBreakerLevel::Inactive);
        assert!(!cb.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_snapshot_contains_correct_fields() {
        let cb = CircuitBreaker::default();
        cb.try_escalate(
            CircuitBreakerLevel::Warning,
            "approaching limit".into(),
        )
        .await;

        let snap = cb.snapshot().await;
        assert_eq!(snap.level, CircuitBreakerLevel::Warning);
        assert_eq!(snap.reason.as_deref(), Some("approaching limit"));
        assert!(snap.triggered_at.is_some());
        assert!(!snap.blocks_new_entries);
        assert!(!snap.blocks_signals);
    }

    #[tokio::test]
    async fn test_level_ordering() {
        assert!(CircuitBreakerLevel::Inactive < CircuitBreakerLevel::Warning);
        assert!(CircuitBreakerLevel::Warning < CircuitBreakerLevel::SoftHalt);
        assert!(CircuitBreakerLevel::SoftHalt < CircuitBreakerLevel::HardHalt);
    }
}
