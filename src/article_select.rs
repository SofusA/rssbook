use std::{
    collections::HashSet,
    fs,
    io::{self, ErrorKind},
};

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

pub fn read_read_articles() -> AppResult<ReadArticles> {
    let contents = match fs::read_to_string(READ_ARTICLES_FILE) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(ReadArticles::default());
        }
        Err(error) => return Err(error.into()),
    };

    Ok(toml::from_str(&contents)?)
}

pub async fn run_select(book: &BookInner, client: &Client) -> AppResult<ReadArticles> {
    let mut book = list_articles(book, client).await?;

    if let Ok(saved) = read_read_articles() {
        apply_saved_selection(&mut book, &saved);
    }

    run_tui(&mut book)?;

    let saved = ReadArticles::from_book(&book);
    let contents = toml::to_string_pretty(&saved).map_err(AppError::from)?;

    fs::write(READ_ARTICLES_FILE, contents).map_err(AppError::from)?;

    Ok(saved)
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
    for article in book
        .iter_mut()
        .flat_map(|category| category.feeds.iter_mut())
        .flat_map(|feed| feed.articles.iter_mut())
    {
        article.selected = saved.contains(&article.url);
    }
}

struct SelectableArticle {
    category_index: usize,
    feed_index: usize,
    article_index: usize,
    row: usize,
}

struct CategoryArticleList {
    name: String,
    feeds: Vec<FeedArticleList>,
}

struct FeedArticleList {
    name: String,
    articles: Vec<Article>,
}

struct Article {
    name: String,
    url: String,
    selected: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ReadArticles {
    articles: HashSet<String>,
}

impl ReadArticles {
    fn from_book(book: &[CategoryArticleList]) -> Self {
        let articles = book
            .iter()
            .flat_map(|category| &category.feeds)
            .flat_map(|feed| &feed.articles)
            .filter(|article| article.selected)
            .map(|article| article.url.clone())
            .collect();

        Self { articles }
    }
    pub fn contains(&self, url: &str) -> bool {
        self.articles.contains(url)
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
                .filter_map(|x| article_details(x.0, x.1))
                .map(|article| Article {
                    name: article.title().to_string(),
                    url: article.link().to_string(),
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
