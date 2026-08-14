use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use reqwest::Client;
use url::Url;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use tokio::sync::{Mutex, OnceCell, Semaphore};

use crate::AppResult;
use crate::book::parsed::Image;
use crate::error::AppError;

#[derive(Debug)]
struct ProcessedImage {
    bytes: Vec<u8>,
    mime_type: String,
}

type ImageCacheEntry = Arc<OnceCell<Arc<ProcessedImage>>>;

#[derive(Clone)]
pub struct ImageDownloader {
    client: Client,
    cache: Arc<Mutex<HashMap<String, ImageCacheEntry>>>,
    processing_semaphore: Arc<Semaphore>,
}

impl ImageDownloader {
    pub fn new(client: Client) -> Self {
        let processing_limit = std::thread::available_parallelism()
            .map_or(2, std::num::NonZero::get)
            .max(1);

        Self {
            client,
            cache: Arc::new(Mutex::new(HashMap::new())),
            processing_semaphore: Arc::new(Semaphore::new(processing_limit)),
        }
    }

    pub async fn download(&self, url: &Url, epub_name: &str) -> AppResult<Image> {
        let cache_key = url.as_str().to_string();

        let cache_entry = {
            let mut cache = self.cache.lock().await;

            cache
                .entry(cache_key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cache_entry
            .get_or_try_init(|| async {
                let source_bytes = self
                    .client
                    .get(url.clone())
                    .send()
                    .await?
                    .error_for_status()?
                    .bytes()
                    .await?;

                let permit = self.processing_semaphore.clone().acquire_owned().await?;

                let processed = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    process_image(source_bytes)
                })
                .await??;

                Ok::<_, AppError>(Arc::new(processed))
            })
            .await?;

        Ok(Image::new(
            format!("images/{epub_name}.jpg"),
            result.bytes.clone(),
            result.mime_type.clone(),
        ))
    }
}

fn process_image(source_bytes: bytes::Bytes) -> AppResult<ProcessedImage> {
    let reader = image::ImageReader::new(Cursor::new(source_bytes)).with_guessed_format()?;

    let mut image = reader.decode()?;

    if image.width() > 780 {
        image = image.resize(780, 780, FilterType::Triangle);
    }

    let image = image.to_rgb8();
    let mut output = Vec::new();

    JpegEncoder::new_with_quality(&mut output, 45).encode(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;

    Ok(ProcessedImage {
        bytes: output,
        mime_type: "image/jpeg".to_string(),
    })
}
