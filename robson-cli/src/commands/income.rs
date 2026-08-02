use anyhow::{bail, Result};
use clap::{Args, Subcommand};

use crate::{
    api_client::{self, IncomeAckApiResponse},
    commands::reconcile_close::{
        EXIT_GENERIC_ERROR, EXIT_INCONSISTENT, EXIT_NOT_FOUND, EXIT_SUCCESS, EXIT_UNAUTHORIZED,
        EXIT_USAGE_ERROR,
    },
};

const MAX_EXCHANGE_INCOME_ID_LENGTH: usize = 255;
const MAX_ACK_REASON_LENGTH: usize = 2000;
const MAX_ACK_ACTOR_LENGTH: usize = 255;

#[derive(Args)]
pub struct IncomeArgs {
    #[command(subcommand)]
    pub command: IncomeCommand,
}

#[derive(Subcommand)]
pub enum IncomeCommand {
    /// Acknowledge one unmatched income-ledger item without deleting it.
    Ack(IncomeAckArgs),
}

#[derive(Args)]
pub struct IncomeAckArgs {
    /// Exchange-assigned income identifier.
    pub exchange_income_id: String,

    /// Human-readable reason for the acknowledgement.
    #[arg(long)]
    pub reason: String,

    /// Operator identity stored in the audit record. Defaults to USER.
    #[arg(long, env = "ROBSON_OPERATOR_ID")]
    pub actor: Option<String>,

    /// Base URL of the robsond API.
    #[arg(long, default_value = "http://localhost:8080")]
    pub robsond_url: String,

    /// Bearer token for authentication. Falls back to ROBSON_API_TOKEN.
    #[arg(long, value_name = "TOKEN", env = "ROBSON_API_TOKEN")]
    pub token: Option<String>,
}

pub async fn run(args: IncomeArgs) -> i32 {
    match args.command {
        IncomeCommand::Ack(args) => run_ack(args).await,
    }
}

async fn run_ack(args: IncomeAckArgs) -> i32 {
    match run_ack_inner(args).await {
        Ok(code) | Err(code) => code,
    }
}

async fn run_ack_inner(args: IncomeAckArgs) -> Result<i32, i32> {
    let exchange_income_id = normalize_required(
        &args.exchange_income_id,
        "exchange_income_id",
        MAX_EXCHANGE_INCOME_ID_LENGTH,
    )
    .map_err(|error| {
        eprintln!("error: {error}");
        EXIT_USAGE_ERROR
    })?;
    let reason =
        normalize_required(&args.reason, "reason", MAX_ACK_REASON_LENGTH).map_err(|error| {
            eprintln!("error: {error}");
            EXIT_USAGE_ERROR
        })?;
    let actor = resolve_actor(args.actor, std::env::var("USER").ok()).map_err(|error| {
        eprintln!("error: {error}");
        EXIT_USAGE_ERROR
    })?;

    let client = api_client::ApiClient::new(&args.robsond_url, args.token.as_deref());
    let response = client
        .acknowledge_income(&exchange_income_id, api_client::IncomeAckRequest { reason, actor })
        .await
        .map_err(|error| {
            eprintln!("error: {error:#}");
            EXIT_GENERIC_ERROR
        })?;

    match response {
        IncomeAckApiResponse::Success(response) => {
            println!(
                "income item {} {} at {} by {}: {}",
                response.exchange_income_id,
                response.status,
                response.acked_at.to_rfc3339(),
                response.acked_by,
                response.ack_reason
            );
            Ok(EXIT_SUCCESS)
        },
        IncomeAckApiResponse::Invalid(response) => {
            print_api_error("invalid acknowledgement", &response);
            Err(EXIT_USAGE_ERROR)
        },
        IncomeAckApiResponse::NotFound(response) => {
            print_api_error("income item not found", &response);
            Err(EXIT_NOT_FOUND)
        },
        IncomeAckApiResponse::Conflict(response) => {
            print_api_error("income item cannot be acknowledged", &response);
            Err(EXIT_INCONSISTENT)
        },
        IncomeAckApiResponse::Unauthorized(response) => {
            eprintln!("unauthorized: {}", response.error);
            Err(EXIT_UNAUTHORIZED)
        },
        IncomeAckApiResponse::Unavailable(response) => {
            print_api_error("income ledger unavailable", &response);
            Err(EXIT_GENERIC_ERROR)
        },
    }
}

fn print_api_error(prefix: &str, response: &api_client::ErrorResponse) {
    if let Some(details) = &response.details {
        eprintln!("{prefix}: {}: {details}", response.error);
    } else {
        eprintln!("{prefix}: {}", response.error);
    }
}

fn normalize_required(value: &str, field: &str, max_length: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{field} must not be empty");
    }
    if value.chars().count() > max_length {
        bail!("{field} must be at most {max_length} characters");
    }
    Ok(value.to_string())
}

fn resolve_actor(explicit: Option<String>, user_fallback: Option<String>) -> Result<String> {
    let candidate = explicit.or(user_fallback).ok_or_else(|| {
        anyhow::anyhow!("operator identity is required; pass --actor or set ROBSON_OPERATOR_ID")
    })?;
    normalize_required(&candidate, "actor", MAX_ACK_ACTOR_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ack_metadata() {
        assert_eq!(
            normalize_required("  orphaned item  ", "reason", 100).unwrap(),
            "orphaned item"
        );
        assert_eq!(resolve_actor(Some("  operator-1  ".to_string()), None).unwrap(), "operator-1");
    }

    #[test]
    fn actor_falls_back_to_os_user() {
        assert_eq!(
            resolve_actor(None, Some("service-operator".to_string())).unwrap(),
            "service-operator"
        );
    }

    #[test]
    fn rejects_missing_or_oversized_ack_metadata() {
        assert!(normalize_required(" ", "reason", 100).is_err());
        assert!(normalize_required("too long", "reason", 3).is_err());
        assert!(resolve_actor(None, None).is_err());
    }
}
