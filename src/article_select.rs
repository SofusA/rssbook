use std::{fs, io};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::future::try_join_all;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Margin,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{
        Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};
use reqwest::Client;

use crate::{
    book::{BookInner, article_details, is_recent_enough, parse_feed},
    error::{AppError, AppResult},
};

const READ_ARTICLES_FILE: &str = "read_articles.toml";

pub async fn run_select(book: &BookInner, client: &Client) -> AppResult<()> {
    let mut book = list_articles(book, client).await?;

    if let Ok(contents) = fs::read_to_string(READ_ARTICLES_FILE)
        && let Ok(saved) = toml::from_str::<ReadArticles>(&contents)
    {
        apply_saved_selection(&mut book, &saved);
    }

    run_tui(&mut book)?;

    let saved = ReadArticles::from_book(&book);
    let contents = toml::to_string_pretty(&saved).map_err(AppError::from)?;

    fs::write(READ_ARTICLES_FILE, contents).map_err(AppError::from)?;

    Ok(())
}

fn run_tui(book: &mut [CategoryArticleList]) -> AppResult<()> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = select_articles(&mut terminal, book);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn select_articles(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    book: &mut [CategoryArticleList],
) -> AppResult<()> {
    let selectable = selectable_articles(book);

    if selectable.is_empty() {
        return Ok(());
    }

    let mut cursor = 0;
    let mut list_state = ListState::default();

    if let Some(selected) = selectable.get(cursor) {
        list_state.select(Some(selected.row));
    }

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let list_area = area.inner(Margin {
                vertical: 1,
                horizontal: 1,
            });

            if cursor == 0 {
                *list_state.offset_mut() = 0;
            }

            let list = List::new(build_list_items(book))
                .block(
                    Block::default()
                        .title(" Read articles — ↑/↓ move, Space select, q save/quit ")
                        .borders(Borders::ALL),
                )
                .highlight_symbol("▶ ")
                .highlight_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                );

            frame.render_stateful_widget(list, area, &mut list_state);

            let viewport_length = list_area.height;
            let content_length = total_rows(book);
            let max_scroll_offset = content_length.saturating_sub(usize::from(viewport_length));

            if max_scroll_offset > 0 {
                let mut scrollbar_state =
                    ScrollbarState::new(max_scroll_offset).position(list_state.offset());

                frame.render_stateful_widget(
                    Scrollbar::new(ScrollbarOrientation::VerticalRight),
                    area.inner(Margin {
                        vertical: 1,
                        horizontal: 0,
                    }),
                    &mut scrollbar_state,
                );
            }
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        let previous_cursor = cursor;

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                cursor = cursor.saturating_sub(1);
            }

            KeyCode::Down | KeyCode::Char('j') => {
                cursor = (cursor.saturating_add(1)).min(selectable.len().saturating_sub(1));
            }

            KeyCode::Home => {
                cursor = 0;
            }

            KeyCode::End => {
                cursor = selectable.len().saturating_sub(1);
            }

            KeyCode::Char(' ') => {
                if let Some(selected) = selectable.get(cursor)
                    && let Some(article) = book
                        .get_mut(selected.category_index)
                        .and_then(|category| category.feeds.get_mut(selected.feed_index))
                        .and_then(|feed| feed.articles.get_mut(selected.article_index))
                {
                    article.selected = !article.selected;
                    cursor = (cursor.saturating_add(1)).min(selectable.len().saturating_sub(1));
                }
            }

            KeyCode::Esc | KeyCode::Char('q') => break,

            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                break;
            }

            _ => {}
        }

        if cursor != previous_cursor {
            list_state.select(selectable.get(cursor).map(|x| x.row));

            if cursor == 0 {
                *list_state.offset_mut() = 0;
            }
        }
    }

    Ok(())
}

fn build_list_items(book: &[CategoryArticleList]) -> Vec<ListItem<'_>> {
    let mut items = Vec::new();

    for category in book {
        items.push(
            ListItem::new(Line::from(format!("Category: {}", category.name))).style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
        );

        for feed in &category.feeds {
            items.push(
                ListItem::new(Line::from(format!("  Feed: {}", feed.name))).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            );

            for article in &feed.articles {
                let marker = if article.selected { "[x]" } else { "[ ]" };

                items.push(ListItem::new(Line::from(format!(
                    "    {marker} {}",
                    article.name
                ))));
            }
        }
    }

    items
}

fn selectable_articles(book: &[CategoryArticleList]) -> Vec<SelectableArticle> {
    let mut result = Vec::new();
    let mut row: usize = 0;

    for (category_index, category) in book.iter().enumerate() {
        row = row.saturating_add(1);

        for (feed_index, feed) in category.feeds.iter().enumerate() {
            row = row.saturating_add(1);

            for (article_index, _) in feed.articles.iter().enumerate() {
                result.push(SelectableArticle {
                    category_index,
                    feed_index,
                    article_index,
                    row,
                });

                row = row.saturating_add(1);
            }
        }
    }

    result
}

fn total_rows(book: &[CategoryArticleList]) -> usize {
    book.iter()
        .map(|category| {
            category
                .feeds
                .iter()
                .map(|feed| feed.articles.len().saturating_add(1))
                .sum::<usize>()
                .saturating_add(1)
        })
        .sum()
}

fn apply_saved_selection(book: &mut [CategoryArticleList], saved: &ReadArticles) {
    for category in book {
        let Some(saved_category) = saved
            .categories
            .iter()
            .find(|saved| saved.name == category.name)
        else {
            continue;
        };

        for feed in &mut category.feeds {
            let Some(saved_feed) = saved_category
                .feeds
                .iter()
                .find(|saved| saved.name == feed.name)
            else {
                continue;
            };

            for article in &mut feed.articles {
                article.selected = saved_feed.articles.iter().any(|name| name == &article.name);
            }
        }
    }
}

struct SelectableArticle {
    category_index: usize,
    feed_index: usize,
    article_index: usize,
    row: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CategoryArticleList {
    name: String,
    feeds: Vec<FeedArticleList>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct FeedArticleList {
    name: String,
    articles: Vec<Article>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Article {
    name: String,
    selected: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ReadArticles {
    categories: Vec<ReadCategory>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ReadCategory {
    name: String,
    feeds: Vec<ReadFeed>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ReadFeed {
    name: String,
    articles: Vec<String>,
}

impl ReadArticles {
    fn from_book(book: &[CategoryArticleList]) -> Self {
        let categories = book
            .iter()
            .filter_map(|category| {
                let feeds = category
                    .feeds
                    .iter()
                    .filter_map(|feed| {
                        let articles = feed
                            .articles
                            .iter()
                            .filter(|article| article.selected)
                            .map(|article| article.name.clone())
                            .collect::<Vec<_>>();

                        if articles.is_empty() {
                            None
                        } else {
                            Some(ReadFeed {
                                name: feed.name.clone(),
                                articles,
                            })
                        }
                    })
                    .collect::<Vec<_>>();

                if feeds.is_empty() {
                    None
                } else {
                    Some(ReadCategory {
                        name: category.name.clone(),
                        feeds,
                    })
                }
            })
            .collect();

        Self { categories }
    }
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
                .map(|(_, title, _link)| Article {
                    name: title,
                    selected: false,
                })
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
