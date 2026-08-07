use std::path::Path;

use reqwest::{Client, multipart};
use tokio::fs;
use url::Url;

use crate::error::{AppError, AppResult};

pub async fn upload(path: &Path, client: &Client, device_url: Url) -> AppResult<()> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(AppError::FileNameEncodingError)?;

    let bytes = fs::read(path).await?;

    let file_part = multipart::Part::bytes(bytes)
        .file_name(filename.to_owned())
        .mime_str("application/epub+zip")?;

    let form = multipart::Form::new().part("file", file_part);

    let upload_url = device_url.join("upload")?;

    client
        .post(upload_url)
        .multipart(form)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}
