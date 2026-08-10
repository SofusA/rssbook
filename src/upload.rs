use std::path::Path;

use reqwest::{Client, header::EXPECT, multipart};
use tokio::fs;
use url::Url;

use crate::error::{AppError, AppResult};

pub async fn upload(path: &Path, device_url: &Url) -> AppResult<()> {
    let client = Client::new();
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(AppError::FileNameEncodingError)?;

    let bytes = fs::read(path).await?;

    let file_part = multipart::Part::bytes(bytes)
        .file_name(filename.to_owned())
        .mime_str("application/epub+zip")
        .unwrap();

    let form = multipart::Form::new().part("file", file_part);

    let upload_url = device_url.join("upload")?;
    eprintln!("URL debug: {:?}", upload_url.as_str());

    client
        .post(upload_url)
        .header(EXPECT, "100-continue")
        .multipart(form)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}
