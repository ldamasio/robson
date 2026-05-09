use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_client;

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_GENERIC_ERROR: i32 = 1;
pub const EXIT_USAGE_ERROR: i32 = 2;
pub const EXIT_NOT_FOUND: i32 = 3;
pub const EXIT_NOT_ACTIVE: i32 = 4;
pub const EXIT_INCONSISTENT: i32 = 5;
pub const EXIT_UNAUTHORIZED: i32 = 6;

#[derive(Args)]
pub struct ReconcileCloseArgs {
    /// UUID of the position to reconcile-close.
    #[arg(long)]
    pub position_id: Uuid,

    /// Path to a JSON file containing the reconciliation evidence.
    #[arg(long)]
    pub evidence_file: PathBuf,

    /// Base URL of the robsond API.
    #[arg(long, default_value = "http://localhost:8080")]
    pub robsond_url: String,

    /// Bearer token for authentication. Falls back to ROBSON_API_TOKEN env var.
    #[arg(long, value_name = "TOKEN", env = "ROBSON_API_TOKEN")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", content = "data", rename_all = "snake_case")]
pub enum EvidenceJson {
    OrderFillRecord(OrderFillData),
    UserTradeRecord(UserTradeData),
    AccountSnapshot(serde_json::Value),
    Estimated(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderFillData {
    pub exchange_order_id: String,
    pub fill_price: String,
    pub filled_quantity: String,
    pub fee: String,
    pub fee_asset: String,
    pub filled_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserTradeData {
    pub exchange_order_id: String,
    pub exchange_trade_id: String,
    pub fill_price: String,
    pub filled_quantity: String,
    pub fee: String,
    pub fee_asset: String,
    pub filled_at: String,
}

pub fn validate_order_fill(data: &OrderFillData) -> Result<()> {
    let price: Decimal = data
        .fill_price
        .parse()
        .ok()
        .context("fill_price is not a valid decimal")?;
    if price <= Decimal::ZERO {
        bail!("fill_price must be > 0");
    }

    let qty: Decimal = data
        .filled_quantity
        .parse()
        .ok()
        .context("filled_quantity is not a valid decimal")?;
    if qty <= Decimal::ZERO {
        bail!("filled_quantity must be > 0");
    }

    let fee: Decimal = data
        .fee
        .parse()
        .ok()
        .context("fee is not a valid decimal")?;
    if fee < Decimal::ZERO {
        bail!("fee must be >= 0");
    }

    if data.fee_asset.is_empty() {
        bail!("fee_asset must not be empty");
    }
    if data.exchange_order_id.is_empty() {
        bail!("exchange_order_id must not be empty");
    }

    let _: DateTime<Utc> = DateTime::parse_from_rfc3339(&data.filled_at)
        .map(|dt| dt.to_utc())
        .context("filled_at is not a valid ISO-8601 timestamp")?;

    Ok(())
}

pub fn validate_user_trade(data: &UserTradeData) -> Result<()> {
    if data.exchange_trade_id.is_empty() {
        bail!("exchange_trade_id must not be empty");
    }

    validate_order_fill(&OrderFillData {
        exchange_order_id: data.exchange_order_id.clone(),
        fill_price: data.fill_price.clone(),
        filled_quantity: data.filled_quantity.clone(),
        fee: data.fee.clone(),
        fee_asset: data.fee_asset.clone(),
        filled_at: data.filled_at.clone(),
    })
}

pub async fn run(args: ReconcileCloseArgs) -> i32 {
    match run_inner(args).await {
        Ok(code) => code,
        Err(code) => code,
    }
}

async fn run_inner(args: ReconcileCloseArgs) -> Result<i32, i32> {
    let raw = std::fs::read_to_string(&args.evidence_file).map_err(|e| {
        eprintln!("error: failed to read {}: {e}", args.evidence_file.display());
        EXIT_GENERIC_ERROR
    })?;
    let evidence: EvidenceJson =
        serde_json::from_str(&raw).map_err(|_| {
            eprintln!("error: invalid evidence JSON");
            EXIT_USAGE_ERROR
        })?;

    match &evidence {
        EvidenceJson::AccountSnapshot(_) => {
            eprintln!(
                "account_snapshot evidence is not supported in Slice 5B1. \
                 Only order_fill_record and user_trade_record are accepted."
            );
            return Err(EXIT_USAGE_ERROR);
        },
        EvidenceJson::Estimated(_) => {
            eprintln!(
                "estimated evidence is not supported in Slice 5B1. \
                 Only order_fill_record and user_trade_record are accepted."
            );
            return Err(EXIT_USAGE_ERROR);
        },
        EvidenceJson::OrderFillRecord(data) => {
            if let Err(e) = validate_order_fill(data) {
                eprintln!("error: {e:#}");
                return Err(EXIT_USAGE_ERROR);
            }
        },
        EvidenceJson::UserTradeRecord(data) => {
            if let Err(e) = validate_user_trade(data) {
                eprintln!("error: {e:#}");
                return Err(EXIT_USAGE_ERROR);
            }
        },
    }

    let request_body = api_client::ReconcileCloseRequest {
        position_id: args.position_id,
        evidence,
    };

    let client = api_client::ApiClient::new(&args.robsond_url, args.token.as_deref());
    let response = client.reconcile_close(request_body).await.map_err(|e| {
        eprintln!("error: {e:#}");
        EXIT_GENERIC_ERROR
    })?;

    match response {
        api_client::ReconcileCloseResponse::Success(resp) => {
            println!(
                "position {} closed: realized_pnl={}, exit_price={}",
                resp.position_id, resp.realized_pnl, resp.exit_price
            );
            Ok(EXIT_SUCCESS)
        },
        api_client::ReconcileCloseResponse::NotFound(resp) => {
            eprintln!("position not found: {}", resp.position_id);
            Err(EXIT_NOT_FOUND)
        },
        api_client::ReconcileCloseResponse::NotActive(resp) => {
            eprintln!(
                "position not active: {} (current_state={})",
                resp.error, resp.current_state
            );
            Err(EXIT_NOT_ACTIVE)
        },
        api_client::ReconcileCloseResponse::Inconsistent(resp) => {
            eprintln!(
                "inconsistent evidence: {}{}",
                resp.error,
                resp.details
                    .as_ref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default()
            );
            Err(EXIT_INCONSISTENT)
        },
        api_client::ReconcileCloseResponse::Unsupported(resp) => {
            eprintln!(
                "unsupported evidence: {}{}",
                resp.error,
                resp.details
                    .as_ref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default()
            );
            Err(EXIT_USAGE_ERROR)
        },
        api_client::ReconcileCloseResponse::Unauthorized(resp) => {
            eprintln!("unauthorized: {}", resp.error);
            Err(EXIT_UNAUTHORIZED)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;

    fn write_evidence_file(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{content}").unwrap();
        f
    }

    #[test]
    fn test_cli_rejects_estimated_evidence() {
        let f = write_evidence_file(
            r#"{"source":"estimated","data":{"estimation_basis":"trailing_stop_at_detection","exit_price":"95000.00","evaluator":"op:ldamasio","detected_at":"2026-05-09T14:30:00Z"}}"#,
        );
        let raw = std::fs::read_to_string(f.path()).unwrap();
        let evidence: EvidenceJson = serde_json::from_str(&raw).unwrap();
        assert!(matches!(evidence, EvidenceJson::Estimated(_)));
    }

    #[test]
    fn test_cli_rejects_account_snapshot_evidence() {
        let f = write_evidence_file(
            r#"{"source":"account_snapshot","data":{"first_observed_missing_at":"2026-05-09T14:00:00Z","confirmed_missing_at":"2026-05-09T14:01:00Z"}}"#,
        );
        let raw = std::fs::read_to_string(f.path()).unwrap();
        let evidence: EvidenceJson = serde_json::from_str(&raw).unwrap();
        assert!(matches!(evidence, EvidenceJson::AccountSnapshot(_)));
    }

    #[test]
    fn test_cli_serializes_order_fill_record() {
        let data = OrderFillData {
            exchange_order_id: "12345678".to_string(),
            fill_price: "95000.50".to_string(),
            filled_quantity: "0.010".to_string(),
            fee: "0.95".to_string(),
            fee_asset: "USDT".to_string(),
            filled_at: "2026-05-09T14:30:00Z".to_string(),
        };
        let evidence = EvidenceJson::OrderFillRecord(data);
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("\"source\":\"order_fill_record\""));
        assert!(json.contains("\"exchange_order_id\":\"12345678\""));
        assert!(json.contains("\"fill_price\":\"95000.50\""));
        assert!(json.contains("\"filled_quantity\":\"0.010\""));
    }

    #[test]
    fn test_cli_serializes_user_trade_record() {
        let data = UserTradeData {
            exchange_order_id: "12345678".to_string(),
            exchange_trade_id: "87654321".to_string(),
            fill_price: "95000.50".to_string(),
            filled_quantity: "0.010".to_string(),
            fee: "0.95".to_string(),
            fee_asset: "USDT".to_string(),
            filled_at: "2026-05-09T14:30:00Z".to_string(),
        };
        let evidence = EvidenceJson::UserTradeRecord(data);
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("\"source\":\"user_trade_record\""));
        assert!(json.contains("\"exchange_trade_id\":\"87654321\""));
        assert!(json.contains("\"exchange_order_id\":\"12345678\""));
    }

    #[test]
    fn test_cli_rejects_invalid_json() {
        let result = serde_json::from_str::<EvidenceJson>("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_validates_order_fill_fields() {
        let valid = OrderFillData {
            exchange_order_id: "123".to_string(),
            fill_price: "95000.50".to_string(),
            filled_quantity: "0.010".to_string(),
            fee: "0.95".to_string(),
            fee_asset: "USDT".to_string(),
            filled_at: "2026-05-09T14:30:00Z".to_string(),
        };
        assert!(validate_order_fill(&valid).is_ok());

        let mut bad = valid.clone();
        bad.fill_price = "-1.0".to_string();
        assert!(validate_order_fill(&bad).is_err());

        let mut bad2 = valid.clone();
        bad2.fee_asset = "".to_string();
        assert!(validate_order_fill(&bad2).is_err());

        let mut bad3 = valid.clone();
        bad3.filled_at = "not-a-date".to_string();
        assert!(validate_order_fill(&bad3).is_err());

        let mut bad4 = valid.clone();
        bad4.exchange_order_id = "".to_string();
        assert!(validate_order_fill(&bad4).is_err());
    }
}
