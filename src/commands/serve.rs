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
        mock: args.mock,
    };
    std::fs::create_dir_all(&cfg.media_dir)?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(web::serve(cfg, args.bind))
}
