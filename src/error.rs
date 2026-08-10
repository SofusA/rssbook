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

    #[error(transparent)]
    Scraper(#[from] scraper::error::SelectorErrorKind<'static>),

    #[error(transparent)]
    RewritingError(#[from] lol_html::errors::RewritingError),

    #[error(transparent)]
    FeedError(#[from] feed_rs::parser::ParseFeedError),

    #[error(transparent)]
    Image(#[from] image::ImageError),

    #[error("Error serializing epub file path")]
    FileNameEncodingError,
}
