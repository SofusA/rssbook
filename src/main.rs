use clap::Parser;
use reqwest::Client;
use url::Url;

use std::time::Duration;

use crate::book::BookBuilder;
use crate::epub::create_epubs;
use crate::error::AppResult;
use crate::image_download::ImageDownloader;
use crate::upload::upload;

mod book;
mod epub;
mod error;
mod html;
mod image_download;
mod upload;

async fn run(device_url: Option<String>) -> AppResult<()> {
    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_mins(1))
        .build()?;

    let image_downloader = ImageDownloader::new(client.clone());

    let book = BookBuilder::new()
        .category(
            "Blogs",
            vec![
                (
                    "Hashimoto".to_string(),
                    Url::parse("https://mitchellh.com/feed.xml")?,
                    Some("footer, title".to_string()),
                ),
                (
                    "Codeberg".to_string(),
                    Url::parse("https://blog.codeberg.org/feeds/all.atom.xml")?,
                    None,
                ),
                (
                    "Andrew Kelly".to_string(),
                    Url::parse("https://andrewkelley.me/rss.xml")?,
                    None,
                ),
                (
                    "Orhun".to_string(),
                    Url::parse("https://blog.orhun.dev/rss.xml")?,
                    None,
                ),
                (
                    "DHH".to_string(),
                    Url::parse("https://world.hey.com/dhh/feed.atom")?,
                    None,
                ),
            ],
            Some(30),
        )
        .category(
            "News",
            vec![
                (
                    "Udland".to_string(),
                    Url::parse("https://www.dr.dk/nyheder/service/feeds/udland")?,
                    Some(".dre-label-text__text, .dre-share-link, .dre-byline, title, [class^=\"BrandLabel\"], [class*=\"truncated-text\"], [class*=\"title-container\"], [class*=\"progress-bar-container\"], [class*=\"read-more\"]".to_string()),
                ),
                (
                    "Indland".to_string(),
                    Url::parse("https://www.dr.dk/nyheder/service/feeds/indland")?,
                    Some(".dre-label-text__text, .dre-share-link, .dre-byline, title, [class^=\"BrandLabel\"], [class*=\"truncated-text\"], [class*=\"title-container\"], [class*=\"progress-bar-container\"], [class*=\"read-more\"]".to_string()),
                ),
            ],
            Some(3)
        )
        .build(&client, &image_downloader)
        .await?;

    let epubs = create_epubs(&book)?;

    println!("Book generation is done");

    if let Some(device_url) = device_url {
        println!("Starting upload");

        let device_url = Url::parse(&device_url)?;

        for epub in epubs {
            print!("Uploading {}... ", epub.to_string_lossy());
            upload(&epub, &device_url).await?;
            println!("done");
        }
    }

    println!("Complete");

    Ok(())
}

#[derive(Parser)]
#[command(name = "rssbook")]
#[command(about = "RSS feeds into an EPUB", version)]
struct Args {
    /// URL for `CrossPoint` device
    #[clap(long)]
    upload: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(error) = run(cli.upload).await {
        eprintln!("{error}");
    }
}
