use std::path::PathBuf;

use chrono::Utc;
use color_print::cprintln;
use feed_rs::model::Feed;
use feed_rs::parser;
use futures::future::try_join_all;
use reqwest::Client;
use reqwest::header::COOKIE;
use url::Url;

use crate::article_select::ReadArticles;
use crate::book::model;
use crate::error::AppResult;
use crate::image_download::ImageDownloader;

use self::epub::create_epubs;
use self::html::{html_sanitation, process_article_html};

mod epub;
mod html;

#[derive(Debug)]
pub struct Book {
    categories: Vec<Category>,
}

impl Book {
    pub fn new(categories: Vec<Category>) -> Self {
        Self { categories }
    }

    pub fn build_epubs(&self) -> AppResult<Vec<PathBuf>> {
        create_epubs(self)
    }
}

#[derive(Debug)]
pub struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

#[derive(Debug)]
pub struct RssFeed {
    name: String,
    articles: Vec<Article>,
    description: String,
}

#[derive(Debug)]
pub struct Article {
    images: Vec<Image>,
    html: String,
    title: String,
}

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
}

pub async fn build_categories(
    categories: &[model::Category],
    read_articles: &ReadArticles,
    client: &Client,
) -> AppResult<Vec<Category>> {
    let image_downloader = ImageDownloader::new(client.clone());

    try_join_all(categories.iter().enumerate().map(|(index, category)| {
        build_category(index, category, read_articles, client, &image_downloader)
    }))
    .await
}

async fn build_category(
    category_index: usize,
    category: &model::Category,
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Category> {
    let feeds = try_join_all(category.feeds().iter().enumerate().map(|(index, feed)| {
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
        name: category.name().to_string(),
        feeds,
    })
}

async fn build_feed(
    category_index: usize,
    feed_index: usize,
    feed: &model::RssFeed,
    read_articles: &ReadArticles,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<RssFeed> {
    let channel = parse_feed(feed.url().clone(), client).await?;
    let description = channel
        .description
        .map(|x| x.content)
        .and_then(|content| html_sanitation(&content).ok())
        .unwrap_or_default();

    let articles = try_join_all(
        channel
            .entries
            .iter()
            .filter(|entry| is_recent_enough(entry, feed.oldest_article()))
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
                    feed.auth(),
                    feed.filter(),
                    client,
                    image_downloader,
                )
            }),
    )
    .await?;

    Ok(RssFeed {
        name: feed.title().to_string(),
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
    auth: Option<&model::Auth>,
    selector: Option<&str>,
    client: &Client,
    image_downloader: &ImageDownloader,
) -> AppResult<Article> {
    cprintln!("Processing  <blue>{title}</>");
    let mut request = client.get(&link);

    if let Some(auth) = auth {
        match auth {
            model::Auth::Cookie(cookie) => {
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
