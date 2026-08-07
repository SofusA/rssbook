use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};

use reqwest::Client;

use std::fs::File;
use std::io::Cursor;
use std::path::Path;

use crate::error::{AppError, AppResult};

struct Image {
    epub_path: String,
    bytes: Vec<u8>,
    mime_type: String,
}

async fn download_image(url: &str, client: &Client) -> AppResult<Image> {
    let response = client.get(url).send().await?;
    let bytes = response.bytes().await?;

    let kind = infer::get(&bytes).ok_or(AppError::MissingMimeType)?;

    let mime_type = kind.mime_type().to_string();
    let extension = kind.extension();

    let epub_path = format!("images/image.{extension}");

    Ok(Image {
        epub_path,
        bytes: bytes.to_vec(),
        mime_type,
    })
}

pub async fn create_book(out_path: &Path, client: &Client) -> AppResult<()> {
    let writer = File::create(out_path)?;

    let image_url = "https://asset.dr.dk/drdk/umbraco-images/11wfchxn/20260801-095612-l.jpg?im=AspectCrop%3D%28720%2C480%29%2CxPosition%3D.5%2CyPosition%3D.5%3BResize%3D%28720%2C480%29";

    let image = download_image(image_url, client).await?;

    let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;

    builder
        .metadata("author", "Wikipedia Contributors")?
        .metadata("title", "Ada Lovelace: first programmer")?
        .inline_toc()
        .add_content(
            EpubContent::new("chapter_1.xhtml", File::open("example1.html")?)
                .title("First Programmer")
                .reftype(ReferenceType::Text),
        )?
        .add_resource(&image.epub_path, Cursor::new(image.bytes), &image.mime_type)?
        .add_content(
            EpubContent::new("chapter_2.xhtml", File::open("example2.html")?)
                .title("First computer program")
                .reftype(ReferenceType::Text),
        )?;

    builder.generate(writer)?;

    Ok(())
}
