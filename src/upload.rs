use color_print::cprintln;
use curl::easy::{Easy, Form};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
use url::Url;

use crate::error::{AppError, AppResult};

pub async fn upload(paths: &[PathBuf], device_url: &Url) -> AppResult<()> {
    println!("Starting upload");
    create_directory(device_url)?;

    for epub in paths {
        cprintln!("Uploading <blue>{}</>... ", epub.to_string_lossy());
        upload_epub(epub, device_url).await?;
        fs::remove_file(epub)?;
        cprintln!("<green>done</>");
    }

    Ok(())
}

async fn upload_epub(path: &Path, device_url: &Url) -> AppResult<()> {
    let path = PathBuf::from(path);

    let mut upload_url = device_url.join("upload")?;
    upload_url.query_pairs_mut().append_pair("path", "/Rss");

    let device_url = device_url.clone();

    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let retries = 20;

        for attempt in 0..=retries {
            let result = upload_once(&path, &upload_url);

            match result {
                Ok(()) => return Ok(()),

                Err(error) if attempt < retries => {
                    if attempt == 0 {
                        delete_remote_path(&path, &device_url)?;
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }

                    println!("Error uploading. Will retry. Error: {error}");
                    thread::sleep(Duration::from_secs(5));
                }

                Err(error) => return Err(error),
            }
        }

        Ok(())
    })
    .await??;

    Ok(())
}

fn delete_remote_path(path: &Path, device_url: &Url) -> AppResult<()> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::InvalidEpubName(path.to_path_buf()))?;

    let remote_path = format!("/Rss/{filename}");
    let delete_url = device_url.join("delete")?;

    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("path", &remote_path)
        .finish();

    let mut handle = Easy::new();
    handle.url(delete_url.as_str())?;
    handle.post(true)?;
    handle.post_fields_copy(body.as_bytes())?;
    handle.fail_on_error(true)?;
    handle.perform()?;

    Ok(())
}

fn upload_once(path: &Path, upload_url: &Url) -> AppResult<()> {
    let mut form = Form::new();

    form.part("file")
        .file(&path)
        .content_type("application/epub+zip")
        .add()?;

    let mut handle = Easy::new();
    handle.url(upload_url.as_str())?;
    handle.httppost(form)?;
    handle.fail_on_error(true)?;
    handle.perform()?;

    Ok(())
}

fn create_directory(device_url: &Url) -> AppResult<()> {
    let mkdir_url = device_url.join("mkdir")?;

    let mut handle = Easy::new();
    handle.url(mkdir_url.as_str())?;
    handle.post(true)?;
    handle.post_fields_copy(b"name=Rss&path=/")?;
    handle.perform()?;

    let status = handle.response_code()?;

    // The device returns 400 if /Rss already exists.
    if status != 200 && status != 400 {
        return Err(AppError::CreateDirectory(status));
    }

    Ok(())
}
