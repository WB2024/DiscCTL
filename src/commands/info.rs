use clap::Args;
use crate::{analyzer, error::Error};

#[derive(Args, Debug)]
pub struct InfoArgs {
    /// Optical drive to inspect
    #[arg(long, default_value = "/dev/sr0")]
    pub device: String,
    /// Output as JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: InfoArgs) -> Result<(), Error> {
    let info = analyzer::analyze(&args.device)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        analyzer::display(&info);
    }

    Ok(())
}
