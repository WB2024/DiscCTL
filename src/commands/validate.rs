use clap::Args;
use crate::{error::Error, parser, planner};

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Disc graph JSON file to validate
    pub input: String,
}

pub fn run(args: ValidateArgs) -> Result<(), Error> {
    let graph = parser::from_file(&args.input)?;
    planner::validate(&graph)?;
    println!("Validation passed.");
    println!("{}", serde_json::to_string_pretty(&graph)?);
    Ok(())
}
