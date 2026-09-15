//! `robson auth login|status|logout|print-token` — Google OAuth Device
//! Authorization Grant for the CLI (ADR-0054). See `crate::auth` for the
//! actual flow; this module is just the `clap` wiring and human-facing
//! output.

use clap::{Args, Subcommand};

use crate::{auth, commands::reconcile_close::EXIT_GENERIC_ERROR};

pub const EXIT_SUCCESS: i32 = 0;

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Sign in with Google via the Device Authorization Grant.
    Login,
    /// Show the cached credential's email and expiry — no network call.
    Status,
    /// Delete the cached credential.
    Logout,
    /// Print the current (transparently refreshed) Google ID token.
    /// Intended for scripting/curl use, e.g.
    /// `curl -H "Authorization: Bearer $(robson auth print-token)" ...`.
    PrintToken,
}

pub async fn run(args: AuthArgs) -> i32 {
    match args.command {
        AuthCommand::Login => run_login().await,
        AuthCommand::Status => run_status(),
        AuthCommand::Logout => run_logout(),
        AuthCommand::PrintToken => run_print_token().await,
    }
}

async fn run_login() -> i32 {
    match auth::login().await {
        Ok(email) => {
            println!("Signed in as {email}");
            EXIT_SUCCESS
        },
        Err(e) => {
            eprintln!("error: {e:#}");
            EXIT_GENERIC_ERROR
        },
    }
}

fn run_status() -> i32 {
    match auth::cached_identity() {
        Ok((email, exp)) => {
            let now = chrono::Utc::now().timestamp();
            let email = email.as_deref().unwrap_or("<unknown email>");
            match chrono::DateTime::from_timestamp(exp, 0) {
                Some(expiry) if exp > now => {
                    println!("Signed in as {email} (id_token expires {})", expiry.to_rfc3339());
                },
                Some(expiry) => {
                    println!(
                        "Signed in as {email}, but the cached id_token expired at {} \
                         (it will be refreshed automatically on next use, if a refresh_token \
                         is cached)",
                        expiry.to_rfc3339()
                    );
                },
                None => {
                    println!("Signed in as {email} (id_token has an unparseable expiry)");
                },
            }
            EXIT_SUCCESS
        },
        Err(e) => {
            eprintln!("error: {e:#}");
            EXIT_GENERIC_ERROR
        },
    }
}

fn run_logout() -> i32 {
    match auth::delete_credentials() {
        Ok(true) => {
            println!("Signed out.");
            EXIT_SUCCESS
        },
        Ok(false) => {
            println!("Not signed in.");
            EXIT_SUCCESS
        },
        Err(e) => {
            eprintln!("error: {e:#}");
            EXIT_GENERIC_ERROR
        },
    }
}

async fn run_print_token() -> i32 {
    match auth::current_id_token().await {
        Ok(token) => {
            println!("{token}");
            EXIT_SUCCESS
        },
        Err(e) => {
            eprintln!("error: {e:#}");
            EXIT_GENERIC_ERROR
        },
    }
}
