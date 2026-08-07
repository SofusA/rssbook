use reqwest::Client;
use rss::Channel;
use url::Url;

use crate::book::create_book;
use crate::error::AppResult;
use crate::upload::upload;
use std::env;
use std::ops::Deref;

mod book;
mod error;
mod upload;

struct BookBuilder {
    categories: Vec<CategoryInner>,
}

struct CategoryInner {
    name: String,
    feeds: Vec<RssFeedInner>,
}

struct RssFeedInner {
    name: String,
    url: Url,
}

struct Book {
    categories: Vec<Category>,
}

struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

struct RssFeed {
    name: String,
    article: Vec<Article>,
}

struct Article {
    images: Vec<Url>,
    html: String,
}

impl BookBuilder {
    fn new() -> Self {
        Self { categories: vec![] }
    }
    fn category(mut self, name: &str, feeds: Vec<RssFeedInner>) -> BookBuilder {
        self.categories.push(CategoryInner {
            name: name.to_string(),
            feeds,
        });

        self
    }

    fn build(&self) -> Book {
        Book { categories: vec![] }
    }
}

// async fn parse_feed(url: Url, client: &Client) -> AppResult<Channel> {
//     let content = client.get(url).send().await?.bytes().await?;
//     let channel = Channel::read_from(&content[..])?;
//     Ok(channel)
// }

// async fn parse_book(book: BookBuilder, client: &Client) -> AppResult<()> {
//     for category in book.categories {
//         for feed in category.feeds {
//             let parsed_feed = parse_feed(feed.url, client).await?;

//             for item in parsed_feed.items {
//                 if let Some(title) = item.title
//                     && let Some(link) = item.link
//                 {
//                     println!("{title}");
//                     println!("{link}");
//                 }
//             }
//         }
//     }

//     Ok(())
// }

async fn run() -> AppResult<()> {
    let client = Client::new();

    let book = BookBuilder::new()
        .category(
            "News",
            vec![RssFeedInner {
                name: "Udland".to_string(),
                url: Url::parse("https://www.dr.dk/nyheder/service/feeds/udland")?,
            }],
        )
        .build();

    // parse_book(book, &client).await?;

    // let out_path = env::current_dir()?.join("dev.epub");

    // println!("write file to: {}", out_path.display());

    // create_book(&out_path, &client).await?;
    // println!("Sample book generation is done");

    // println!("Uploading");
    // let device_url = Url::parse("http://10.0.3.86")?;
    // upload(&out_path, &client, device_url).await?;
    // println!("Upload done");

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
    }
}
