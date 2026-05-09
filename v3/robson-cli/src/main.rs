mod api_client;
mod commands;

use clap::Parser;

#[derive(Parser)]
#[command(name = "robson-cli", version, about = "Operational CLI for Robson daemon")]
enum Cli {
    ReconcileClose(commands::reconcile_close::ReconcileCloseArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli {
        Cli::ReconcileClose(args) => commands::reconcile_close::run(args).await,
    };
    std::process::exit(code);
}
