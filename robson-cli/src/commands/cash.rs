//! `robson-cli cash reanchor --reason "<what moved>"` (ADR-0056 step 8l-5;
//! v16 §C8): calls `POST /venue/cash-reanchor` and prints the daemon's
//! answer. At rest, outside ±1 h of a UTC month boundary.
use clap::{Args, Subcommand};

use crate::{
    api_client::ApiClient,
    commands::reconcile_close::{
        EXIT_GENERIC_ERROR, EXIT_INCONSISTENT, EXIT_SUCCESS, EXIT_UNAUTHORIZED, EXIT_USAGE_ERROR,
    },
};

const MAX_REASON_LENGTH: usize = 2000;

#[derive(Args)]
pub struct CashArgs {
    #[command(subcommand)]
    pub command: CashCommand,
}

#[derive(Subcommand)]
pub enum CashCommand {
    /// Re-anchor the Spot cash ledger to the wallet after an external
    /// movement: two agreeing observations, then one audited transaction.
    Reanchor(ReanchorArgs),
}

#[derive(Args)]
pub struct ReanchorArgs {
    /// What moved, for the audit record.
    #[arg(long)]
    pub reason: String,

    /// Base URL of the robsond API.
    #[arg(long, default_value = "http://localhost:8080")]
    pub robsond_url: String,
}

pub async fn run(args: CashArgs) -> i32 {
    let CashCommand::Reanchor(args) = args.command;
    let reason = args.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_LENGTH {
        eprintln!("error: reason must be 1 to {MAX_REASON_LENGTH} characters");
        return EXIT_USAGE_ERROR;
    }
    let token = crate::auth::current_id_token().await;
    let Ok(token) = token.map_err(|error| eprintln!("error: {error:#}")) else {
        return EXIT_UNAUTHORIZED;
    };
    match ApiClient::new(&args.robsond_url, &token).cash_reanchor(reason).await {
        Ok((200, body)) => {
            println!("{body}");
            EXIT_SUCCESS
        },
        Ok((status, body)) => {
            eprintln!("robsond answered {status}: {body}");
            match status {
                400 => EXIT_USAGE_ERROR,
                401 => EXIT_UNAUTHORIZED,
                409 => EXIT_INCONSISTENT,
                _ => EXIT_GENERIC_ERROR,
            }
        },
        Err(error) => {
            eprintln!("error: {error:#}");
            EXIT_GENERIC_ERROR
        },
    }
}

#[cfg(test)]
#[path = "cash_tests.rs"]
mod tests;
