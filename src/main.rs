mod backend;
mod commands;
mod error;
mod model;
mod parser;
mod planner;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "discctl", about = "Unified CD authoring CLI tool", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Burn a disc from flags or a disc graph JSON
    Burn(commands::burn::BurnArgs),
    /// Print the burn execution plan without burning
    Plan(commands::plan::PlanArgs),
    /// Validate a disc graph JSON file
    Validate(commands::validate::ValidateArgs),
    /// Inspect disc state and recover from interrupted burns
    Recover(commands::recover::RecoverArgs),
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Burn(args) => commands::burn::run(args),
        Cmd::Plan(args) => commands::plan::run(args),
        Cmd::Validate(args) => commands::validate::run(args),
        Cmd::Recover(args) => commands::recover::run(args),
    };
    if let Err(e) = result {
        let disc_err = e.to_disc_error();
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&disc_err).unwrap_or_else(|_| e.to_string())
        );
        std::process::exit(1);
    }
}
