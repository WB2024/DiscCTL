use clap::Args;
use crate::{error::Error, parser, planner};

#[derive(Args, Debug)]
pub struct PlanArgs {
    /// Disc format: redbook, datacd, bluebook
    #[arg(long)]
    pub format: Option<String>,
    /// Load disc graph from JSON file
    #[arg(long)]
    pub input: Option<String>,
    /// Audio track files or glob patterns
    #[arg(long, num_args = 1..)]
    pub audio: Option<Vec<String>>,
    /// Source directory for data session
    #[arg(long)]
    pub data: Option<String>,
    /// Disc label
    #[arg(long, default_value = "Untitled")]
    pub label: String,
}

pub fn run(args: PlanArgs) -> Result<(), Error> {
    let graph = if let Some(ref path) = args.input {
        parser::from_file(path)?
    } else {
        let format = args.format.as_deref().unwrap_or("redbook");
        parser::from_cli(format, args.audio.as_deref(), args.data.as_deref(), &args.label)?
    };

    let plan = planner::plan(&graph)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}
