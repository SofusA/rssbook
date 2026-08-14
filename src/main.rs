use clap::Parser;
use color_print::cprintln;
use reqwest::Client;
use url::Url;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::article_select::{read_read_articles, run_select};
use crate::book::config;
use crate::error::AppResult;
use crate::upload::upload;

mod article_select;
mod book;
mod error;
mod image_download;
mod upload;

async fn run(config_path: &Path, device_url: Option<String>, select: bool) -> AppResult<()> {
    let config = config::Book::from_path(config_path)?.into();

    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_mins(1))
        .build()?;

    let read_articles = if select {
        run_select(&config, &client).await?
    } else {
        read_read_articles()?
    };

    let book = config.parse(&read_articles, &client).await?;
    let epubs = book.build_epubs()?;

    cprintln!("<green>Book generation is done</>");

    if let Some(device_url) = device_url {
        let device_url = Url::parse(&device_url)?;
        upload(&epubs, &device_url).await?;
    }

    cprintln!("<green>Complete</>");

    Ok(())
}

#[derive(Parser)]
#[command(name = "rssbook")]
#[command(about = "RSS feeds into an EPUB", version)]
struct Args {
    #[clap(long, default_value = "./rssbook.toml")]
    config: PathBuf,

    #[clap(short, long)]
    select: bool,

    /// URL for `CrossPoint` device
    #[clap(short, long)]
    upload_crosspoint: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(error) = run(&cli.config, cli.upload_crosspoint, cli.select).await {
        eprintln!("{error}");
    }
}
