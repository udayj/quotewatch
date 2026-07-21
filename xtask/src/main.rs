mod classifier_eval;
mod eval_db;
mod types;

use clap::{Parser, Subcommand};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(about = "Local development tasks for QuoteWatch")]
struct Cli {
    #[command(subcommand)]
    command: CommandArg,
}

#[derive(Subcommand)]
enum CommandArg {
    ClassifierEval(classifier_eval::Args),
}

// Parses the xtask command line and delegates to the selected task module.
fn main() -> Result<()> {
    match Cli::parse().command {
        CommandArg::ClassifierEval(args) => classifier_eval::run(args),
    }
}
