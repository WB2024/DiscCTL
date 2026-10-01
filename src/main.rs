mod analyzer;
mod backend;
mod commands;
mod error;
mod library;
mod model;
mod parser;
mod perms;
mod planner;
mod rip;
mod stick;
mod web;

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
    /// Run the web UI and API (headless-server friendly)
    Serve(commands::serve::ServeArgs),
    /// Rusty Stick: write music to a USB stick, filed the way you like
    Stick(commands::stick::StickArgs),
    /// Import ripped albums into the music library, named by a Picard naming script
    Import(commands::import::ImportArgs),
    /// Hand ripped albums to Lidarr to import (adds the artist and album if needed)
    ImportLidarr(commands::import_lidarr::ImportLidarrArgs),
    /// Give existing files the configured owner and permissions (RUSTYDISC_PUID, RUSTYDISC_PGID, RUSTYDISC_UMASK)
    FixPermissions(commands::fix_permissions::FixPermissionsArgs),
}

fn main() {
    perms::init();
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Burn(args)     => commands::burn::run(args),
        Cmd::Plan(args)     => commands::plan::run(args),
        Cmd::Validate(args) => commands::validate::run(args),
        Cmd::Recover(args)  => commands::recover::run(args),
        Cmd::Info(args)     => commands::info::run(args),
        Cmd::Rip(args)      => commands::rip::run(args),
        Cmd::Verify(args)   => commands::verify::run(args),
        Cmd::Serve(args)    => commands::serve::run(args),
        Cmd::Stick(args)    => commands::stick::run(args),
        Cmd::Import(args)   => commands::import::run(args),
        Cmd::ImportLidarr(args) => commands::import_lidarr::run(args),
        Cmd::FixPermissions(args) => commands::fix_permissions::run(args),
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
