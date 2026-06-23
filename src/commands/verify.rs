use clap::Args;
use crate::{error::Error, rip::metadata};

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Directory containing the ripped archive (must have metadata/checksums.json)
    pub directory: String,
}

pub fn run(args: VerifyArgs) -> Result<(), Error> {
    let meta_dir = format!("{}/metadata", args.directory);
    let manifest_path = format!("{}/checksums.json", meta_dir);

    // Support both <dir>/metadata/checksums.json and <dir>/checksums.json
    let search_dir = if std::path::Path::new(&manifest_path).exists() {
        meta_dir.clone()
    } else {
        args.directory.clone()
    };

    eprintln!("Verifying archive: {}", args.directory);

    let result = metadata::verify_checksums(&search_dir)?;

    if !result.missing.is_empty() {
        eprintln!("MISSING ({}):", result.missing.len());
        for path in &result.missing {
            eprintln!("  ✗  {}", path);
        }
    }

    if !result.failed.is_empty() {
        eprintln!("FAILED ({}):", result.failed.len());
        for path in &result.failed {
            eprintln!("  ✗  {}", path);
        }
    }

    eprintln!("Passed: {}", result.passed);

    if result.is_ok() {
        eprintln!("OK — all {} files verified.", result.passed);
        Ok(())
    } else {
        Err(Error::validation(format!(
            "Verification failed: {} missing, {} corrupted",
            result.missing.len(),
            result.failed.len()
        )))
    }
}
