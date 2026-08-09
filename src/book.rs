use epub_builder::{EpubBuilder, EpubContent, ZipLibrary};

use std::io::Cursor;
use std::{fs::File, path::PathBuf};

use crate::{Book, error::AppResult};

pub fn create_epubs(book: &Book) -> AppResult<Vec<PathBuf>> {
    let mut paths = Vec::new();

    for category in &book.categories {
        let output_path = PathBuf::from(format!("{}.epub", category.name));
        let writer = File::create(&output_path)?;

        let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;

        builder.metadata("title", &category.name)?;

        for (feed_index, feed) in category.feeds.iter().enumerate() {
            builder.add_content(
                EpubContent::new(
                    format!("feed_{feed_index}.html"),
                    Cursor::new(format!("<h1>{}</h1>", feed.name)),
                )
                .title(&feed.name)
                .level(1),
            )?;

            for (article_index, article) in feed.articles.iter().enumerate() {
                for image in &article.images {
                    builder.add_resource(
                        &image.epub_path,
                        Cursor::new(image.bytes.clone()),
                        &image.mime_type,
                    )?;
                }

                builder.add_content(
                    EpubContent::new(
                        format!("feed_{feed_index}_article_{article_index}.html"),
                        Cursor::new(article.html.clone()),
                    )
                    .title(&article.title)
                    .level(2),
                )?;
            }
        }

        builder.generate(writer)?;
        paths.push(output_path);
    }

    Ok(paths)
}
