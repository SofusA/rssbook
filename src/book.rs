use epub_builder::{EpubBuilder, EpubContent, ZipLibrary};

use std::fs::File;
use std::io::Cursor;
use std::path::Path;

use crate::{Book, error::AppResult};

pub fn create_book(out_path: &Path, book: &Book) -> AppResult<()> {
    let writer = File::create(out_path)?;
    let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;

    builder.metadata("title", "RSS Book")?.inline_toc();

    for category in &book.categories {
        for (feed_index, feed) in category.feeds.iter().enumerate() {
            let feed_path = format!("feed_{feed_index}.html");
            let feed_html = format!("<h1>{}</h1>", feed.name);

            builder.add_content(
                EpubContent::new(feed_path, Cursor::new(feed_html))
                    .title(&feed.name)
                    .level(1),
            )?;

            for (article_index, article) in feed.articles.iter().enumerate() {
                for image in &article.images {
                    builder.add_resource(
                        &image.epub_path,
                        Cursor::new(&image.bytes),
                        &image.mime_type,
                    )?;
                }

                let article_path = format!("feed_{feed_index}_article_{article_index}.html");

                builder.add_content(
                    EpubContent::new(article_path, Cursor::new(&article.html))
                        .title(&article.title)
                        .level(2),
                )?;
            }
        }
    }

    builder.generate(writer)?;
    Ok(())
}
