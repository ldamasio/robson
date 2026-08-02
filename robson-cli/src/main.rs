mod api_client;
mod commands;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "robson-cli",
    version,
    about = "Operational CLI for Robson daemon"
)]
enum Cli {
    ReconcileClose(commands::reconcile_close::ReconcileCloseArgs),
    Income(commands::income::IncomeArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli {
        Cli::ReconcileClose(args) => commands::reconcile_close::run(args).await,
        Cli::Income(args) => commands::income::run(args).await,
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_income_ack_command() {
        let cli = Cli::try_parse_from([
            "robson-cli",
            "income",
            "ack",
            "900307906811427",
            "--reason",
            "orphaned external close",
            "--actor",
            "operator-1",
        ])
        .unwrap();

        let Cli::Income(args) = cli else {
            panic!("expected income command");
        };
        let commands::income::IncomeCommand::Ack(args) = args.command;
        assert_eq!(args.exchange_income_id, "900307906811427");
        assert_eq!(args.reason, "orphaned external close");
        assert_eq!(args.actor.as_deref(), Some("operator-1"));
    }
}
