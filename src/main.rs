use reqwest::Client;
use url::Url;

use crate::book::create_book;
use crate::error::AppResult;
use crate::upload::upload;
use std::env;

mod book;
mod error;
mod upload;

async fn run() -> AppResult<()> {
    let client = Client::new();
    let out_path = env::current_dir()?.join("dev.epub");

    println!("write file to: {}", out_path.display());

    create_book(&out_path, &client).await?;
    println!("Sample book generation is done");

    println!("Uploading");
    let device_url = Url::parse("http://10.0.3.86")?;
    upload(&out_path, &client, device_url).await?;
    println!("Upload done");

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
    }
}
