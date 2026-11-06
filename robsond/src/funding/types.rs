use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How the funding worker treats unfinished sagas on the route it runs on
/// (ADR-0056 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingRouteMode {
    /// The selected venue exposes funding: resume every non-terminal saga.
    Selected,
    /// The selected venue has no funding; the worker runs on the saga's
    /// original route. Started sagas finish there, an unstarted quote only
    /// expires, and the selected venue's engine capital is left alone.
    Recovery,
    /// No route has a funding surface (the original credentials are missing):
    /// nothing is sent and nothing is written. Started sagas stay non-terminal
    /// with their liabilities retained, and the reason is recomputed from the
    /// route on every start; an unstarted quote waits for a route to expire it.
    Blocked,
}

/// Why a [`FundingRouteMode::Blocked`] route holds a saga.
pub const FUNDING_ROUTE_UNAVAILABLE: &str = "funding_route_unavailable";

impl FundingRouteMode {
    /// Why this route holds a saga in `state`, if it does. A saga that never
    /// started or already finished carries no liability to hold.
    pub fn blocking_reason(self, state: &str) -> Option<&'static str> {
        let holds_liabilities = state != FundingState::Quoted.as_str()
            && state != FundingState::Refreshed.as_str()
            && state != FundingState::Failed.as_str();
        (self == Self::Blocked && holds_liabilities).then_some(FUNDING_ROUTE_UNAVAILABLE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FundingState {
    Quoted,
    Converting,
    Converted,
    Transferring,
    Settled,
    Refreshed,
    Failed,
}

impl FundingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Quoted => "QUOTED",
            Self::Converting => "CONVERTING",
            Self::Converted => "CONVERTED",
            Self::Transferring => "TRANSFERRING",
            Self::Settled => "SETTLED",
            Self::Refreshed => "REFRESHED",
            Self::Failed => "FAILED",
        }
    }
}

impl std::str::FromStr for FundingState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "QUOTED" => Ok(Self::Quoted),
            "CONVERTING" => Ok(Self::Converting),
            "CONVERTED" => Ok(Self::Converted),
            "TRANSFERRING" => Ok(Self::Transferring),
            "SETTLED" => Ok(Self::Settled),
            "REFRESHED" => Ok(Self::Refreshed),
            "FAILED" => Ok(Self::Failed),
            other => Err(format!("unknown funding state {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingQuoteItem {
    pub asset: String,
    pub qty: Decimal,
    pub est_usdt: Decimal,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingQuote {
    pub quote_id: Uuid,
    pub items: Vec<FundingQuoteItem>,
    pub estimated_usdt: Decimal,
    pub fees: Decimal,
    pub slippage_bps: u32,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingEventView {
    #[serde(rename = "type")]
    pub event_type: String,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingSagaView {
    pub saga_id: Uuid,
    pub state: String,
    /// Why the saga cannot progress on the current route. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub items: Vec<FundingQuoteItem>,
    pub events: Vec<FundingEventView>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingSagaSummary {
    pub saga_id: Uuid,
    pub state: String,
    /// Why the saga cannot progress on the current route. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub estimated_usdt: Decimal,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteFundingRequest {
    pub quote_id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecuteFundingResponse {
    pub saga_id: Uuid,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapitalRefreshResponse {
    pub capital: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_blocked_route_holds_a_started_unfinished_saga() {
        let started = [
            FundingState::Converting,
            FundingState::Converted,
            FundingState::Transferring,
            FundingState::Settled,
        ];
        for state in started {
            assert_eq!(
                FundingRouteMode::Blocked.blocking_reason(state.as_str()),
                Some(FUNDING_ROUTE_UNAVAILABLE),
                "{state:?}"
            );
            assert_eq!(FundingRouteMode::Selected.blocking_reason(state.as_str()), None);
            assert_eq!(FundingRouteMode::Recovery.blocking_reason(state.as_str()), None);
        }
        for state in [
            FundingState::Quoted,
            FundingState::Refreshed,
            FundingState::Failed,
        ] {
            assert_eq!(
                FundingRouteMode::Blocked.blocking_reason(state.as_str()),
                None,
                "{state:?}"
            );
        }
    }

    #[test]
    fn the_blocking_reason_is_additive_on_the_funding_views() {
        let mut summary = FundingSagaSummary {
            saga_id: Uuid::nil(),
            state: FundingState::Transferring.as_str().to_string(),
            blocked_reason: None,
            estimated_usdt: Decimal::ONE,
            created_at: DateTime::<Utc>::MIN_UTC,
        };

        // An unblocked saga serializes exactly as it did before the field.
        let unblocked = serde_json::to_value(&summary).unwrap();
        assert!(unblocked.get("blocked_reason").is_none());
        let keys = ["created_at", "estimated_usdt", "saga_id", "state"];
        let mut found: Vec<&str> =
            unblocked.as_object().unwrap().keys().map(String::as_str).collect();
        found.sort_unstable();
        assert_eq!(found, keys);

        summary.blocked_reason = Some(FUNDING_ROUTE_UNAVAILABLE.to_string());
        let blocked = serde_json::to_value(&summary).unwrap();
        assert_eq!(blocked["blocked_reason"], FUNDING_ROUTE_UNAVAILABLE);
        assert_eq!(blocked["state"], "TRANSFERRING", "the state itself does not change");

        // Older payloads still parse.
        let parsed: FundingSagaSummary = serde_json::from_value(unblocked).unwrap();
        assert_eq!(parsed.blocked_reason, None);
    }
}
