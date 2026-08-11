use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Epub(#[from] epub_builder::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Url(#[from] url::ParseError),

    #[error(transparent)]
    RssParsing(#[from] rss::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("Selector error: {0}")]
    Scraper(String),

    #[error(transparent)]
    RewritingError(#[from] lol_html::errors::RewritingError),

    #[error(transparent)]
    FeedError(#[from] feed_rs::parser::ParseFeedError),

    #[error(transparent)]
    Image(#[from] image::ImageError),

    #[error(transparent)]
    Deserialize(#[from] toml::de::Error),

    #[error(transparent)]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error(transparent)]
    Semaphore(#[from] tokio::sync::AcquireError),

    #[error("Error serializing epub file path")]
    FileNameEncodingError,
}

impl<'a> From<scraper::error::SelectorErrorKind<'a>> for AppError {
    fn from(error: scraper::error::SelectorErrorKind<'a>) -> Self {
        Self::Scraper(error.to_string())
    }
}
