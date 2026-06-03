use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub items: Vec<FundingQuoteItem>,
    pub events: Vec<FundingEventView>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingSagaSummary {
    pub saga_id: Uuid,
    pub state: String,
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
