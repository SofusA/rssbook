use clap::Parser;
use feed_rs::model::Feed;
use feed_rs::parser;
use futures::future::try_join_all;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use lol_html::{RewriteStrSettings, element, rewrite_str};
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use url::Url;

use std::io::Cursor;

use crate::book::create_epubs;
use crate::error::{AppError, AppResult};
use crate::upload::upload;

mod book;
mod error;
mod upload;

use std::collections::{HashMap, HashSet};

#[derive(Debug)]
struct Image {
    epub_path: String,
    bytes: Vec<u8>,
    mime_type: String,
}

#[derive(Debug)]
struct Book {
    categories: Vec<Category>,
}

#[derive(Debug)]
struct Category {
    name: String,
    feeds: Vec<RssFeed>,
}

#[derive(Debug)]
struct RssFeed {
    name: String,
    articles: Vec<Article>,
}

#[derive(Debug)]
struct Article {
    images: Vec<Image>,
    html: String,
    title: String,
}

struct BookBuilder {
    categories: Vec<CategoryInner>,
}

struct CategoryInner {
    name: String,
    feeds: Vec<(String, Url, Option<String>)>,
}

impl BookBuilder {
    fn new() -> Self {
        Self { categories: vec![] }
    }

    fn category(mut self, name: &str, feeds: Vec<(String, Url, Option<String>)>) -> BookBuilder {
        self.categories.push(CategoryInner {
            name: name.to_string(),
            feeds,
        });

        self
    }

    async fn build(&self, client: &Client) -> AppResult<Book> {
        let categories = try_join_all(self.categories.iter().enumerate().map(
            |(category_index, category)| async move {
                let feeds = try_join_all(category.feeds.iter().enumerate().map(
                    |(feed_index, feed)| async move {
                        let channel = parse_feed(feed.1.clone(), client).await?;

                        let articles = try_join_all(
                            channel
                                .entries
                                .iter()
                                .enumerate()
                                .filter_map(|(article_index, entry)| {
                                    let link = entry
                                        .links
                                        .iter()
                                        .find(|link| {
                                            link.rel
                                                .as_deref()
                                                .is_none_or(|rel| rel == "alternate")
                                        })
                                        .or_else(|| entry.links.first())?
                                        .href
                                        .clone();

                                    let title = entry
                                        .title
                                        .as_ref()
                                        .map(|title| title.content.clone())
                                        .filter(|title| !title.trim().is_empty())
                                        .unwrap_or_else(|| {
                                            format!("Article {}", article_index.saturating_add(1))
                                        });

                                    Some((article_index, title, link))
                                })
                                .take(10)
                                .map(|(article_index, title, link)| async move {
                                    let html = client
                                        .get(link.clone())
                                        .send()
                                        .await?
                                        .error_for_status()?
                                        .text()
                                        .await?;

                                    let image_name_prefix = format!("category-{category_index}-feed-{feed_index}-article-{article_index}");

                                    process_article_html(&link, &html, &image_name_prefix, title, feed.2.as_deref(), client )
                                        .await
                                }),
                        )
                        .await?;

                        Ok::<RssFeed, AppError>(RssFeed {
                            name: feed.0.clone(),
                            articles,
                        })
                    },
                ))
                .await?;

                Ok::<Category, AppError>(Category {
                    name: category.name.clone(),
                    feeds,
                })
            },
        ))
        .await?;

        Ok(Book { categories })
    }
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

fn self_close_img_tags(html: &str) -> String {
    let re = Regex::new(r"(?i)<img\b([^<>]*?)(?:\s*/)?\s*>").unwrap();
    re.replace_all(html, "<img$1 />").into_owned()
}

async fn process_article_html(
    page_url: &str,
    source_html: &str,
    image_name_prefix: &str,
    title: String,
    filter: Option<&str>,
    client: &Client,
) -> AppResult<Article> {
    let base_url = Url::parse(page_url)?;
    let main_selector = Selector::parse("main").expect("valid main selector");
    let image_selector = Selector::parse("img[src]").expect("valid image selector");

    let document = Html::parse_document(source_html);

    let selected_html = document
        .select(&main_selector)
        .next()
        .map_or_else(|| source_html.to_string(), |main| main.inner_html());

    let selected_document = Html::parse_fragment(&selected_html);

    let mut seen_sources = HashSet::new();

    let image_sources = selected_document
        .select(&image_selector)
        .filter_map(|element| element.value().attr("src"))
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .filter(|src| seen_sources.insert((*src).to_string()))
        .filter_map(|src| {
            resolve_image_url(&base_url, src).map(|absolute_url| (src.to_string(), absolute_url))
        })
        .collect::<Vec<_>>();

    let downloaded_images = try_join_all(image_sources.into_iter().enumerate().map(
        |(image_index, (original_src, absolute_url))| {
            let epub_name = format!("{image_name_prefix}-image-{image_index}");

            async move {
                match download_image(absolute_url.as_str(), &epub_name, client).await {
                    Ok(image) => {
                        Ok::<Option<(String, Image)>, AppError>(Some((original_src, image)))
                    }
                    Err(error) => {
                        eprintln!("Skipping image {absolute_url}: {error}");
                        Ok(None)
                    }
                }
            }
        },
    ))
    .await?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let image_replacements = downloaded_images
        .iter()
        .map(|(original_src, image)| (original_src.clone(), image.epub_path.clone()))
        .collect::<HashMap<_, _>>();

    let rewritten_html = rewrite_article_html(&selected_html, image_replacements, filter)?
        .replace("&nbsp;", "&#160;");

    let images = downloaded_images
        .into_iter()
        .map(|(_, image)| image)
        .collect();

    Ok(Article {
        images,
        html: rewritten_html,
        title,
    })
}

fn resolve_image_url(base_url: &Url, src: &str) -> Option<Url> {
    let src = src.trim();

    if src.is_empty() || src.starts_with("data:") || src.starts_with("blob:") {
        return None;
    }

    let url = base_url.join(src).ok()?;

    match url.scheme() {
        "http" | "https" => Some(url),
        _ => None,
    }
}

fn rewrite_article_html(
    html: &str,
    image_replacements: HashMap<String, String>,
    filter: Option<&str>,
) -> AppResult<String> {
    let mut settings = RewriteStrSettings::new()
        // Remove elements that should not be included in the EPUB.
        //
        // `remove()` removes both the element and all of its content.
        .append_element_content_handler(element!(
            "script, style, iframe, frame, frameset, object, embed, \
             applet, canvas, noscript, template, form, input, button, \
             select, option, textarea, video, audio, source, track, \
             link, meta, base, aside",
            |element| {
                element.remove();
                Ok(())
            }
        ))
        // Remove styling, metadata, and JavaScript event attributes
        // from every remaining element.
        .append_element_content_handler(element!("*", |element| {
            let attributes_to_remove = element
                .attributes()
                .iter()
                .map(|attribute| attribute.name().clone())
                .filter(|name| {
                    let name = name.to_ascii_lowercase();

                    name == "style"
                        || name == "width"
                        || name == "height"
                        || name == "align"
                        || name == "bgcolor"
                        || name == "background"
                        || name == "border"
                        || name == "cellpadding"
                        || name == "cellspacing"
                        || name == "color"
                        || name == "face"
                        || name == "size"
                        || name == "itemprop"
                        || name == "itemscope"
                        || name == "itemtype"
                        || name.starts_with("data-")
                        || name.starts_with("aria-")
                        || name.starts_with("on")
                })
                .collect::<Vec<_>>();

            for attribute_name in attributes_to_remove {
                element.remove_attribute(&attribute_name);
            }

            Ok(())
        }))
        // Rewrite downloaded image paths and remove web-oriented
        // image attributes.
        .append_element_content_handler(element!("img", move |element| {
            element.remove_attribute("srcset");
            element.remove_attribute("sizes");
            element.remove_attribute("loading");
            element.remove_attribute("decoding");
            element.remove_attribute("fetchpriority");

            let Some(src) = element.get_attribute("src") else {
                return Ok(());
            };

            if let Some(epub_path) = image_replacements.get(src.trim()) {
                element.set_attribute("src", epub_path)?;
            } else {
                element.remove_attribute("src");
            }

            Ok(())
        }))
        .append_element_content_handler(element!("svg", |element| {
            element.remove();
            Ok(())
        }))
        .append_element_content_handler(element!("picture", |element| {
            element.remove_and_keep_content();
            Ok(())
        }));

    if let Some(filter) = filter {
        settings = settings.append_element_content_handler(element!(filter, |element| {
            element.remove();
            Ok(())
        }));
    }

    settings = settings.append_element_content_handler(element!("*", |element| {
        let attributes_to_remove = element
            .attributes()
            .iter()
            .map(|attribute| attribute.name().clone())
            .filter(|name| {
                let name = name.to_ascii_lowercase();

                name == "id" || name == "class"
            })
            .collect::<Vec<_>>();

        for attribute_name in attributes_to_remove {
            element.remove_attribute(&attribute_name);
        }

        Ok(())
    }));

    let rewritten = rewrite_str(html, settings)?;
    Ok(self_close_img_tags(&rewritten))
}

async fn download_image(url: &str, epub_name: &str, client: &Client) -> AppResult<Image> {
    let response = client.get(url).send().await?.error_for_status()?;
    let source_bytes = response.bytes().await?;

    let reader = image::ImageReader::new(Cursor::new(source_bytes)).with_guessed_format()?;

    let mut image = reader.decode()?;

    // Keep dimensions comfortably within CrossPoint's limits.
    if image.width() > 780 {
        image = image.resize(780, 780, FilterType::Lanczos3);
    }

    // Remove alpha and unusual colour formats.
    let image = image.to_rgb8();

    let mut output = Vec::new();

    JpegEncoder::new_with_quality(&mut output, 65).encode(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;

    Ok(Image {
        epub_path: format!("images/{epub_name}.jpg"),
        bytes: output,
        mime_type: "image/jpeg".to_string(),
    })
}

async fn run(device_url: Option<String>) -> AppResult<()> {
    let client = Client::new();

    // remove:
    // .dre-label-text__text
    // .dre-share-link

    let book = BookBuilder::new()
        .category(
            "Blogs",
            vec![
                // (
                //     "Rust".to_string(),
                //     Url::parse("https://blog.rust-lang.org/feed.xml")?,
                // ),
                // (
                //     "Rust inside".to_string(),
                //     Url::parse("https://blog.rust-lang.org/inside-rust/feed.xml")?,
                // ),
                (
                    "Hashimoto".to_string(),
                    Url::parse("https://mitchellh.com/feed.xml")?,
                    None,
                ),
                (
                    "Codeberg".to_string(),
                    Url::parse("https://blog.codeberg.org/feeds/all.atom.xml")?,
                    None,
                ),
                (
                    "Andrew Kelly".to_string(),
                    Url::parse("https://andrewkelley.me/rss.xml")?,
                    None,
                ),
                (
                    "Orhun".to_string(),
                    Url::parse("https://blog.orhun.dev/rss.xml")?,
                    None,
                ),
                (
                    "DHH".to_string(),
                    Url::parse("https://world.hey.com/dhh/feed.atom")?,
                    None,
                ),
            ],
        )
        .category(
            "News",
            vec![
                (
                    "Udland".to_string(),
                    Url::parse("https://www.dr.dk/nyheder/service/feeds/udland")?,
                    Some(".dre-label-text__text, .dre-share-link".to_string()),
                ),
                (
                    "Indland".to_string(),
                    Url::parse("https://www.dr.dk/nyheder/service/feeds/indland")?,
                    Some(".dre-label-text__text, .dre-share-link".to_string()),
                ),
            ],
        )
        .build(&client)
        .await?;

    for cat in &book.categories {
        for reed in &cat.feeds {
            for article in &reed.articles {
                println!("{}", article.title);
                println!("{}", article.html);
            }
        }
    }

    let epubs = create_epubs(&book)?;
    println!("Book generation is done");

    if let Some(device_url) = device_url {
        println!("Starting upload");
        let device_url = Url::parse(&device_url)?;
        for epub in epubs {
            print!("Uploading {}... ", epub.to_string_lossy());
            upload(&epub, &client, &device_url).await?;
            println!("done");
        }
    }

    println!("Complete");

    Ok(())
}

#[derive(Parser)]
#[command(name = "rssbook")]
#[command(about = "Rss feeds into an epub", version)]
struct Args {
    #[clap(long)]
    /// Url for crosspoint device url
    upload: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(err) = run(cli.upload).await {
        eprintln!("{err}");
    }
}
