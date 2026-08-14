use std::{fs, path::Path};

use url::Url;

use crate::{book::model, error::AppResult};

fn auth_from_deserialized(auth: Auth) -> Option<model::Auth> {
    auth.cookie.map(model::Auth::Cookie)
}

#[derive(serde::Deserialize)]
pub struct Book {
    categories: Vec<Category>,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<Auth>,
}

impl Book {
    pub fn from_path(path: &Path) -> AppResult<Self> {
        let config_contents = fs::read_to_string(path)?;
        let config = toml::from_str(&config_contents)?;

        Ok(config)
    }
}

#[derive(serde::Deserialize)]
pub struct Category {
    name: String,
    feeds: Vec<RssFeed>,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<Auth>,
}

#[derive(serde::Deserialize)]
pub struct RssFeed {
    title: String,
    url: Url,
    oldest_article: Option<u64>,
    filter: Option<String>,
    auth: Option<Auth>,
}

#[derive(Clone, serde::Deserialize)]
pub struct Auth {
    cookie: Option<String>,
}

impl From<Book> for model::Book {
    fn from(book: Book) -> model::Book {
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
                    .map(|feed| {
                        model::RssFeed::new(
                            feed.title,
                            feed.url,
                            feed.oldest_article.or(oldest_article),
                            feed.filter.or_else(|| filter.clone()),
                            feed.auth
                                .or_else(|| auth.clone())
                                .and_then(auth_from_deserialized),
                        )
                    })
                    .collect();

                model::Category::new(category.name, feeds)
            })
            .collect();

        model::Book::new(categories)
    }
}
