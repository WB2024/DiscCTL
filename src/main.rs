mod analyzer;
mod backend;
mod commands;
mod error;
mod model;
mod parser;
mod planner;
mod rip;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "rustydisc",
    about = "Optical disc toolkit — burn, rip, archive, verify",
    version
)]
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
    /// Inspect a disc: detect format, list sessions and tracks, show CD-Text
    Info(commands::info::InfoArgs),
    /// Rip a disc to files (audio and/or data)
    Rip(commands::rip::RipArgs),
    /// Verify a ripped archive against its checksums.json
    Verify(commands::verify::VerifyArgs),
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Burn(args)     => commands::burn::run(args),
        Cmd::Plan(args)     => commands::plan::run(args),
        Cmd::Validate(args) => commands::validate::run(args),
        Cmd::Recover(args)  => commands::recover::run(args),
        Cmd::Info(args)     => commands::info::run(args),
        Cmd::Rip(args)      => commands::rip::run(args),
        Cmd::Verify(args)   => commands::verify::run(args),
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
