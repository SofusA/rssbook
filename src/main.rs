use clap::Parser;
use color_print::cprintln;
use reqwest::Client;
use url::Url;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::article_select::run_select;
use crate::book::{BookInner, build_book};
use crate::epub::create_epubs;
use crate::error::AppResult;
use crate::image_download::ImageDownloader;
use crate::upload::upload;

mod article_select;
mod book;
mod epub;
mod error;
mod html;
mod image_download;
mod upload;

async fn run(config_path: &Path, device_url: Option<String>, select: bool) -> AppResult<()> {
    let config_contents = fs::read_to_string(config_path)?;
    let config: BookInner = toml::from_str(&config_contents)?;

    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_mins(1))
        .build()?;

    if select {
        run_select(&config, &client).await?;

        // TODO: Remove
        return Ok(());
    }

    let image_downloader = ImageDownloader::new(client.clone());

    let book = build_book(&config, &client, &image_downloader).await?;

    let epubs = create_epubs(&book)?;

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
    upload: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(error) = run(&cli.config, cli.upload, cli.select).await {
        eprintln!("{error}");
    }
}
