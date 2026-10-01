use clap::Args;

use crate::{error::Error, perms};

#[derive(Args, Debug)]
pub struct FixPermissionsArgs {
    /// Folders to fix (every file and folder under each)
    #[arg(required = true)]
    pub paths: Vec<String>,
}

pub fn run(args: FixPermissionsArgs) -> Result<(), Error> {
    let c = perms::config();
    if !c.is_set() {
        return Err(Error::validation("Set RUSTYDISC_PUID, RUSTYDISC_PGID and/or RUSTYDISC_UMASK first, so there is something to apply."));
    }
    let mut total = 0;
    for p in &args.paths {
        let n = perms::fix_tree(std::path::Path::new(p));
        eprintln!("{p}: {n} change{}", if n == 1 { "" } else { "s" });
        total += n;
    }
    eprintln!("Done: {total} change{}.", if total == 1 { "" } else { "s" });
    Ok(())
}
