use chrono::Utc;
use color_print::cprintln;
use feed_rs::model::Feed;
use feed_rs::parser;
use futures::future::try_join_all;
use reqwest::Client;
use url::Url;

use crate::error::AppResult;
use crate::html::process_article_html;
use crate::image_download::ImageDownloader;

#[derive(Debug, Clone)]
pub struct Image {
    epub_path: String,
    bytes: Vec<u8>,
    mime_type: String,
}

impl Image {
    pub fn new(epub_path: String, bytes: Vec<u8>, mime_type: String) -> Self {
        Self {
            epub_path,
            bytes,
            mime_type,
        }
    }

    pub fn epub_path(&self) -> &str {
        &self.epub_path
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }
}

#[derive(Debug)]
pub struct Book {
    categories: Vec<Category>,
}

impl Book {
    pub fn categories(&self) -> &[Category] {
        &self.categories
    }
}

#[derive(Debug)]
pub struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

impl Category {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn feeds(&self) -> &[RssFeed] {
        &self.feeds
    }
}

#[derive(Debug)]
pub struct RssFeed {
    name: String,
    articles: Vec<Article>,
}

impl RssFeed {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn articles(&self) -> &[Article] {
        &self.articles
    }
}

#[derive(Debug)]
pub struct Article {
    images: Vec<Image>,
    html: String,
    title: String,
}

impl Article {
    pub fn new(images: Vec<Image>, html: String, title: String) -> Self {
        Self {
            images,
            html,
            title,
        }
    }

    pub fn images(&self) -> &[Image] {
        &self.images
    }

    pub fn html(&self) -> &str {
        &self.html
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

pub struct BookBuilder {
    categories: Vec<CategoryInner>,
}

struct CategoryInner {
    name: String,
    feeds: Vec<(String, Url, Option<String>)>,
    oldest_article: Option<u64>,
}

impl BookBuilder {
    pub fn new() -> Self {
        Self { categories: vec![] }
    }

    pub fn category(
        mut self,
        name: &str,
        feeds: Vec<(String, Url, Option<String>)>,
        oldest_article: Option<u64>,
    ) -> Self {
        self.categories.push(CategoryInner {
            name: name.to_string(),
            feeds,
            oldest_article,
        });

        self
    }

    pub async fn build(
        &self,
        client: &Client,
        image_downloader: &ImageDownloader,
    ) -> AppResult<Book> {
        let categories = build_categories(&self.categories, client, image_downloader).await?;

        Ok(Book { categories })
    }
}

async fn build_categories(
    categories: &[CategoryInner],
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Vec<Category>> {
    try_join_all(
        categories
            .iter()
            .enumerate()
            .map(|(index, category)| build_category(index, category, client, image_downloader)),
    )
    .await
}

async fn build_category(
    category_index: usize,
    category: &CategoryInner,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Category> {
    let feeds = try_join_all(category.feeds.iter().enumerate().map(|(index, feed)| {
        build_feed(
            category_index,
            index,
            feed,
            category.oldest_article,
            client,
            image_downloader,
        )
    }))
    .await?;

    Ok(Category {
        name: category.name.clone(),
        feeds,
    })
}

async fn build_feed(
    category_index: usize,
    feed_index: usize,
    feed: &(String, Url, Option<String>),
    oldest_article: Option<u64>,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<RssFeed> {
    let channel = parse_feed(feed.1.clone(), client).await?;

    let articles = try_join_all(
        channel
            .entries
            .iter()
            .filter(|entry| is_recent_enough(entry, oldest_article))
            .enumerate()
            .filter_map(article_details)
            .map(|(article_index, title, link)| {
                build_article(
                    category_index,
                    feed_index,
                    article_index,
                    title,
                    link,
                    feed.2.as_deref(),
                    client,
                    image_downloader,
                )
            }),
    )
    .await?;

    Ok(RssFeed {
        name: feed.0.clone(),
        articles,
    })
}

fn is_recent_enough(entry: &feed_rs::model::Entry, oldest_article: Option<u64>) -> bool {
    if oldest_article.is_none() {
        return true;
    }

    let cutoff =
        oldest_article.and_then(|days| Utc::now().checked_sub_days(chrono::Days::new(days)));

    entry
        .published
        .or(entry.updated)
        .zip(cutoff)
        .is_some_and(|(date, cutoff)| date > cutoff)
}

fn article_details(
    (article_index, entry): (usize, &feed_rs::model::Entry),
) -> Option<(usize, String, String)> {
    let link = entry
        .links
        .iter()
        .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or_else(|| entry.links.first())?
        .href
        .clone();

    let title = entry
        .title
        .as_ref()
        .map(|title| title.content.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| format!("Article {}", article_index.saturating_add(1)));

    Some((article_index, title, link))
}

async fn build_article(
    category_index: usize,
    feed_index: usize,
    article_index: usize,
    title: String,
    link: String,
    selector: Option<&str>,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Article> {
    cprintln!("Processing  <blue>{title}</>");

    let html = client
        .get(&link)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let image_name_prefix =
        format!("category-{category_index}-feed-{feed_index}-article-{article_index}");

    process_article_html(
        &link,
        &html,
        &image_name_prefix,
        title,
        selector,
        image_downloader,
    )
    .await
}

async fn parse_feed(url: Url, client: &Client) -> AppResult<Feed> {
    let content = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    Ok(parser::parse(&content[..])?)
}
