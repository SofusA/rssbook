use color_print::cprintln;
use futures::future::try_join_all;
use reqwest::Client;

use crate::{
    book::{BookInner, article_details, is_recent_enough, parse_feed},
    error::{AppError, AppResult},
};

pub async fn run_select(book: &BookInner, client: &Client) -> AppResult<()> {
    let book = list_articles(book, client).await?;

    for c in book {
        cprintln!("<blue>Category: {}</>", c.name);

        for f in c.feeds {
            cprintln!("<green>Feed: {}</>", f.name);

            for a in f.articles {
                println!("{a}");
            }
        }
    }

    Ok(())
}

struct CategoryArticleList {
    name: String,
    feeds: Vec<FeedArticleList>,
}

struct FeedArticleList {
    name: String,
    articles: Vec<String>,
}

async fn list_articles(book: &BookInner, client: &Client) -> AppResult<Vec<CategoryArticleList>> {
    try_join_all(book.categories().iter().map(|category| async move {
        let feeds = try_join_all(category.feeds().iter().map(|feed| async move {
            let channel = parse_feed(feed.url().clone(), client).await?;

            let articles = channel
                .entries
                .iter()
                .filter(|entry| is_recent_enough(entry, category.oldest_article()))
                .enumerate()
                .filter_map(article_details)
                .map(|(_, title, _link)| title)
                .collect();

            Ok::<FeedArticleList, AppError>(FeedArticleList {
                name: feed.title().to_string(),
                articles,
            })
        }))
        .await?;

        Ok(CategoryArticleList {
            name: category.name().to_string(),
            feeds,
        })
    }))
    .await
}
