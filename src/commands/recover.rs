use clap::Args;
use crate::{backend::device, error::Error};

#[derive(Args, Debug)]
pub struct RecoverArgs {
    /// Optical drive device to inspect and recover
    #[arg(long, default_value = "/dev/sr0")]
    pub device: String,
    /// Blank a CD-RW disc instead of attempting session recovery.
    /// Use "fast" (erase lead-in only) or "all" (full erase).
    #[arg(long, value_name = "fast|all")]
    pub blank: Option<String>,
    /// Print debug information
    #[arg(long)]
    pub debug: bool,
}

pub fn run(args: RecoverArgs) -> Result<(), Error> {
    device::check_device(&args.device)?;

    if let Some(ref mode) = args.blank {
        if mode != "fast" && mode != "all" {
            return Err(Error::validation(format!(
                "Invalid blank mode '{}'. Use 'fast' or 'all'.",
                mode
            )));
        }
        println!("Blanking CD-RW on {} (mode: {})...", args.device, mode);
        device::blank_cdrw(&args.device, mode, args.debug)?;
        println!("Disc blanked successfully.");
        return Ok(());
    }

    // Report disc state then attempt recovery if needed
    let state = device::query_disc_state(&args.device)?;
    match &state {
        device::DiscState::Blank => {
            println!("Disc on {} is blank. No recovery needed.", args.device);
        }
        device::DiscState::Finalized => {
            println!("Disc on {} is finalized. Cannot write further.", args.device);
            println!("If this is a CD-RW, use --blank fast to erase it.");
        }
        device::DiscState::Appendable { msinfo } => {
            println!(
                "Disc on {} is appendable (msinfo: {}). No recovery needed.",
                args.device, msinfo
            );
        }
        device::DiscState::OpenSession => {
            println!(
                "Disc on {} has an open (interrupted) session. Attempting recovery...",
                args.device
            );
            device::recover_open_session(&args.device, args.debug)?;
            println!("Recovery complete. Check disc state again before burning.");
        }
        device::DiscState::Unknown(detail) => {
            println!("Could not determine disc state on {}.", args.device);
            if !detail.is_empty() {
                println!("Drive output: {}", detail);
            }
            println!("Try ejecting and reinserting the disc, then run recover again.");
        }
    }

    Ok(())
}
