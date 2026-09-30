use std::net::SocketAddr;
use clap::Args;
use crate::{error::Error, web};

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Address to listen on
    #[arg(long, env = "RUSTYDISC_BIND", default_value = "0.0.0.0:8080")]
    pub bind: SocketAddr,

    /// Default optical drive
    #[arg(long, env = "RUSTYDISC_DEVICE", default_value = "/dev/sr0")]
    pub device: String,

    /// Directory rips are written to and the library is read from
    #[arg(long, env = "RUSTYDISC_RIPS_DIR", default_value = "./rips")]
    pub rips_dir: String,

    /// Directory burn sources (audio files, data folders, playlists) are picked from
    #[arg(long, env = "RUSTYDISC_MEDIA_DIR", default_value = "./media")]
    pub media_dir: String,

    /// Where settings are stored (mount a volume here in Docker)
    #[arg(long, env = "RUSTYDISC_CONFIG_DIR", default_value = "./config")]
    pub config_dir: String,

    /// Folders Rusty Stick may write to besides detected USB sticks (separate several with ':').
    /// Sticks mounted inside them are listed too.
    #[arg(long, env = "RUSTYDISC_STICK_DIRS", value_delimiter = ':')]
    pub stick_dir: Vec<String>,

    /// Where converted files are kept when Settings says to keep them (default: `cache` in the config folder)
    #[arg(long, env = "RUSTYDISC_CACHE_DIR")]
    pub cache_dir: Option<String>,

    /// The music library folder rips are imported into (Settings can override it)
    #[arg(long, env = "RUSTYDISC_LIBRARY_DIR")]
    pub library_dir: Option<String>,

    /// Require a login (set together with --auth-password). Otherwise the login can be set in Settings.
    #[arg(long, env = "RUSTYDISC_AUTH_USER")]
    pub auth_user: Option<String>,

    /// Password for --auth-user
    #[arg(long, env = "RUSTYDISC_AUTH_PASSWORD", hide_env_values = true)]
    pub auth_password: Option<String>,

    /// Simulate the drive (fake disc, fake rips/burns) — for trying the UI without hardware
    #[arg(long, env = "RUSTYDISC_MOCK", value_parser = clap::builder::BoolishValueParser::new())]
    pub mock: bool,
}

pub fn run(args: ServeArgs) -> Result<(), Error> {
    let cfg = web::Config {
        exe: std::env::current_exe()?,
        device: args.device,
        rips_dir: std::path::PathBuf::from(args.rips_dir),
        media_dir: std::path::PathBuf::from(args.media_dir),
        config_dir: std::path::PathBuf::from(args.config_dir),
        stick_dirs: args.stick_dir.iter().filter(|s| !s.trim().is_empty()).map(std::path::PathBuf::from).collect(),
        cache_dir: args.cache_dir.filter(|d| !d.trim().is_empty()).map(std::path::PathBuf::from),
        library_dir: args.library_dir.filter(|d| !d.trim().is_empty()).map(std::path::PathBuf::from),
        mock: args.mock,
        auth: match (args.auth_user, args.auth_password) {
            (None, None) => None,
            (u, p) => Some((u.unwrap_or_default(), p.unwrap_or_default())),
        },
    };
    std::fs::create_dir_all(&cfg.media_dir)?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(web::serve(cfg, args.bind))
}
