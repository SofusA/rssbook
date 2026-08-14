use chrono::Utc;
use color_print::cprintln;
use feed_rs::model::Feed;
use feed_rs::parser;
use futures::future::try_join_all;
use reqwest::Client;
use reqwest::header::COOKIE;
use url::Url;

use crate::article_select::ReadArticles;
use crate::error::AppResult;
use crate::html::{html_sanitation, process_article_html};
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
    description: String,
}

impl RssFeed {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn articles(&self) -> &[Article] {
        &self.articles
    }

    pub fn description(&self) -> &str {
        &self.description
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

pub struct BookInner {
    categories: Vec<CategoryInner>,
}

impl BookInner {
    pub fn categories(&self) -> &[CategoryInner] {
        &self.categories
    }
}
pub struct CategoryInner {
    name: String,
    feeds: Vec<RssFeedInner>,
}

impl CategoryInner {
    pub fn feeds(&self) -> &[RssFeedInner] {
        &self.feeds
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct RssFeedInner {
    title: String,
    url: Url,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<Auth>,
}

impl RssFeedInner {
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn oldest_article(&self) -> Option<u64> {
        self.oldest_article
    }
}

enum Auth {
    Cookie(String),
}

impl Auth {
    fn from_deserialized(auth: AuthDeserializedChange) -> Option<Self> {
        auth.cookie.map(Auth::Cookie)
    }
}

#[derive(serde::Deserialize)]
pub struct BookDeserializedChange {
    categories: Vec<CategoryDeserializedChange>,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<AuthDeserializedChange>,
}

#[derive(serde::Deserialize)]
pub struct CategoryDeserializedChange {
    name: String,
    feeds: Vec<RssFeedDeserializedChange>,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<AuthDeserializedChange>,
}

#[derive(serde::Deserialize)]
pub struct RssFeedDeserializedChange {
    title: String,
    url: Url,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<AuthDeserializedChange>,
}

#[derive(Clone, serde::Deserialize)]
struct AuthDeserializedChange {
    cookie: Option<String>,
}

impl From<BookDeserializedChange> for BookInner {
    fn from(book: BookDeserializedChange) -> Self {
        let categories = book
            .categories
            .into_iter()
            .map(|category| {
                let oldest_article = category.oldest_article.or(book.oldest_article);
                let filter = category.filter.or_else(|| book.filter.clone());
                let auth = category.auth.or_else(|| book.auth.clone());

                let feeds = category
                    .feeds
                    .into_iter()
                    .map(|feed| RssFeedInner {
                        title: feed.title,
                        url: feed.url,
                        oldest_article: feed.oldest_article.or(oldest_article),
                        filter: feed.filter.or_else(|| filter.clone()),

                        auth: feed
                            .auth
                            .or_else(|| auth.clone())
                            .and_then(Auth::from_deserialized),
                    })
                    .collect();

                CategoryInner {
                    name: category.name,
                    feeds,
                }
            })
            .collect();

        BookInner { categories }
    }
}

pub async fn build_book(
    book: &BookInner,
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Book> {
    let categories =
        build_categories(&book.categories, read_articles, client, image_downloader).await?;

    Ok(Book { categories })
}

async fn build_categories(
    categories: &[CategoryInner],
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Vec<Category>> {
    try_join_all(categories.iter().enumerate().map(|(index, category)| {
        build_category(index, category, read_articles, client, image_downloader)
    }))
    .await
}

async fn build_category(
    category_index: usize,
    category: &CategoryInner,
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Category> {
    let feeds = try_join_all(category.feeds.iter().enumerate().map(|(index, feed)| {
        build_feed(
            category_index,
            index,
            feed,
            read_articles,
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
    feed: &RssFeedInner,
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<RssFeed> {
    let channel = parse_feed(feed.url.clone(), client).await?;
    let description = channel
        .description
        .map(|x| x.content)
        .and_then(|content| html_sanitation(&content).ok())
        .unwrap_or_default();

    let articles = try_join_all(
        channel
            .entries
            .iter()
            .filter(|entry| is_recent_enough(entry, feed.oldest_article))
            .enumerate()
            .filter_map(|x| article_details(x.0, x.1))
            .filter(|article| !read_articles.contains(&article.link))
            .map(|article| {
                build_article(
                    category_index,
                    feed_index,
                    article.index,
                    article.title,
                    article.link,
                    feed.auth.as_ref(),
                    feed.filter.as_deref(),
                    client,
                    image_downloader,
                )
            }),
    )
    .await?;

    Ok(RssFeed {
        name: feed.title.clone(),
        articles,
        description,
    })
}

pub fn is_recent_enough(entry: &feed_rs::model::Entry, oldest_article: Option<u64>) -> bool {
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

pub struct ArticleDetails {
    title: String,
    link: String,
    index: usize,
}

impl ArticleDetails {
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn link(&self) -> &str {
        &self.link
    }
}

pub fn article_details(index: usize, entry: &feed_rs::model::Entry) -> Option<ArticleDetails> {
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
        .unwrap_or_else(|| format!("Article {}", index.saturating_add(1)));

    Some(ArticleDetails { title, link, index })
}

async fn build_article(
    category_index: usize,
    feed_index: usize,
    article_index: usize,
    title: String,
    link: String,
    auth: Option<&Auth>,
    selector: Option<&str>,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Article> {
    cprintln!("Processing  <blue>{title}</>");
    let mut request = client.get(&link);

    if let Some(auth) = auth {
        match auth {
            Auth::Cookie(cookie) => {
                request = request.header(COOKIE, cookie);
            }
        }
    }

    let html = request.send().await?.error_for_status()?.text().await?;

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

pub async fn parse_feed(url: Url, client: &Client) -> AppResult<Feed> {
    let content = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    Ok(parser::parse(&content[..])?)
}
