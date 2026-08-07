use clap::Parser;
use futures::future::try_join_all;
use reqwest::Client;
use rss::Channel;
use url::Url;

use crate::book::create_book;
use crate::error::{AppError, AppResult};
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
    feeds: Vec<(String, Url)>,
}

#[derive(Debug)]
struct Book {
    categories: Vec<Category>,
}

#[derive(Debug)]
struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

#[derive(Debug)]
struct RssFeed {
    name: String,
    articles: Vec<Article>,
}

#[derive(Debug)]
struct Article {
    images: Vec<Url>,
    html: String,
}

impl BookBuilder {
    fn new() -> Self {
        Self { categories: vec![] }
    }

    fn category(mut self, name: &str, feeds: Vec<(String, Url)>) -> BookBuilder {
        self.categories.push(CategoryInner {
            name: name.to_string(),
            feeds,
        });

        self
    }

    async fn build_old(&self, client: &Client) -> AppResult<Book> {
        let mut categories = vec![];
        for category in &self.categories {
            let mut feeds = vec![];
            for feed in &category.feeds {
                let _channel = parse_feed(feed.1.clone(), client).await?;
                let article = Article {
                    images: vec![],
                    html: "<p>test</p>".to_string(),
                };

                let articles = vec![article];

                feeds.push(RssFeed {
                    name: feed.0.clone(),
                    articles,
                });
            }

            categories.push(Category {
                name: category.name.clone(),
                feeds,
            });
        }

        let book = Book { categories };
        Ok(book)
    }

    async fn build(&self, client: &Client) -> AppResult<Book> {
        let categories = try_join_all(self.categories.iter().map(|category| async move {
            let feeds = try_join_all(category.feeds.iter().map(|feed| async move {
                let channel = parse_feed(feed.1.clone(), client).await?;

                let articles = try_join_all(
                    channel
                        .items
                        .iter()
                        .filter_map(|item| item.link.as_deref())
                        .map(|link| async move {
                            let html = client
                                .get(link)
                                .send()
                                .await?
                                .error_for_status()?
                                .text()
                                .await?;

                            Ok::<Article, AppError>(Article {
                                images: vec![],
                                html,
                            })
                        }),
                )
                .await?;

                Ok::<RssFeed, AppError>(RssFeed {
                    name: feed.0.clone(),
                    articles,
                })
            }))
            .await?;

            Ok::<Category, AppError>(Category {
                name: category.name.clone(),
                feeds,
            })
        }))
        .await?;

        Ok(Book { categories })
    }
}

async fn parse_feed(url: Url, client: &Client) -> AppResult<Channel> {
    let content = client.get(url).send().await?.bytes().await?;
    let channel = Channel::read_from(&content[..])?;
    Ok(channel)
}

async fn run(device_url: Option<String>) -> AppResult<()> {
    let client = Client::new();

    let book = BookBuilder::new()
        .category(
            "News",
            vec![(
                "Udland".to_string(),
                Url::parse("https://www.dr.dk/nyheder/service/feeds/udland")?,
            )],
        )
        .build(&client)
        .await?;

    println!("{:?}", book);

    // parse_book(book, &client).await?;

    let out_path = env::current_dir()?.join("dev.epub");

    // println!("write file to: {}", out_path.display());

    // create_book(&out_path, &client).await?;
    // println!("Sample book generation is done");

    if let Some(device_url) = device_url {
        println!("Uploading");
        let device_url = Url::parse(&device_url)?;
        upload(&out_path, &client, device_url).await?;
        println!("Upload done");
    }

    Ok(())
}

#[derive(Parser)]
#[command(name = "rssbook")]
#[command(about = "Rss feeds into an epub", version)]
struct Args {
    #[clap(long)]
    /// Url for crosspoint device url
    upload: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(err) = run(cli.upload).await {
        eprintln!("{err}");
    }
}
