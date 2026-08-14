use reqwest::Client;
use url::Url;

use crate::{article_select::ReadArticles, book::parsed, error::AppResult};

pub struct Book {
    categories: Vec<Category>,
}

impl Book {
    pub fn new(categories: Vec<Category>) -> Self {
        Self { categories }
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    pub async fn parse(
        &self,
        read_articles: &ReadArticles,
        client: &Client,
    ) -> AppResult<parsed::Book> {
        let categories = parsed::build_categories(&self.categories, read_articles, client).await?;

        Ok(parsed::Book::new(categories))
    }
}
pub struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

impl Category {
    pub fn new(name: String, feeds: Vec<RssFeed>) -> Self {
        Self { name, feeds }
    }

    pub fn feeds(&self) -> &[RssFeed] {
        &self.feeds
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct RssFeed {
    title: String,
    url: Url,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<Auth>,
    article_selector: String,
}

impl RssFeed {
    pub fn new(
        title: String,
        url: Url,
        oldest_article: Option<u64>,
        filter: Option<String>,
        auth: Option<Auth>,
        article_selector: Option<String>,
    ) -> Self {
        Self {
            title,
            url,
            oldest_article,
            filter,
            auth,
            article_selector: article_selector.unwrap_or("article".to_string()),
        }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn oldest_article(&self) -> Option<u64> {
        self.oldest_article
    }

    pub fn auth(&self) -> Option<&Auth> {
        self.auth.as_ref()
    }

    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    pub fn article_selector(&self) -> &str {
        &self.article_selector
    }
}

pub enum Auth {
    Cookie(String),
}
