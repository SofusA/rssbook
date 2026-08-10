use clap::Parser;
use feed_rs::model::Feed;
use feed_rs::parser;
use futures::future::try_join_all;
use html5ever::tree_builder::TreeSink;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use lol_html::{RewriteStrSettings, element, rewrite_str};
use regex::Regex;
use reqwest::Client;
use scraper::{ElementRef, Html, HtmlTreeSink, Selector};
use url::Url;

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::sync::{Mutex, OnceCell, Semaphore};

use crate::book::create_epubs;
use crate::error::{AppError, AppResult};
use crate::upload::upload;

mod book;
mod error;
mod upload;

static ARTICLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("article").expect("valid article selector"));

static IMAGE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("img[src]").expect("valid image selector"));

static WRAPPER_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div, span, section").expect("valid wrapper selector"));

static EMPTY_CANDIDATE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "div, span, section, article, main, header, footer, nav, \
         p, blockquote, pre, ul, ol, li, dl, dt, dd, a, figure, \
         figcaption, table, caption, colgroup, thead, tbody, tfoot, \
         tr, th, td, strong, em, b, i, u, s, small, mark, sub, sup, \
         q, cite, abbr",
    )
    .expect("valid empty candidate selector")
});

static MEANINGFUL_ELEMENT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("img[src], br, hr").expect("valid selector"));

static IMG_TAG_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<img\b([^<>]*?)(?:\s*/)?\s*>").expect("valid img regex"));

#[derive(Debug, Clone)]
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

#[derive(Debug)]
struct ProcessedImage {
    bytes: Vec<u8>,
    mime_type: String,
}

type CachedImageResult = Result<Arc<ProcessedImage>, String>;
type ImageCacheEntry = Arc<OnceCell<CachedImageResult>>;

#[derive(Clone)]
struct ImageDownloader {
    client: Client,
    cache: Arc<Mutex<HashMap<String, ImageCacheEntry>>>,
    processing_semaphore: Arc<Semaphore>,
}

impl ImageDownloader {
    fn new(client: Client) -> Self {
        let processing_limit = std::thread::available_parallelism()
            .map_or(2, std::num::NonZero::get)
            .max(1);

        Self {
            client,
            cache: Arc::new(Mutex::new(HashMap::new())),
            processing_semaphore: Arc::new(Semaphore::new(processing_limit)),
        }
    }

    async fn download(&self, url: &Url, epub_name: &str) -> Result<Image, String> {
        let cache_key = url.as_str().to_string();

        let cache_entry = {
            let mut cache = self.cache.lock().await;

            cache
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cache_entry
            .get_or_init(|| async {
                let source_bytes = self
                    .client
                    .get(url.clone())
                    .send()
                    .await
                    .map_err(|error| error.to_string())?
                    .error_for_status()
                    .map_err(|error| error.to_string())?
                    .bytes()
                    .await
                    .map_err(|error| error.to_string())?;

                let permit = self
                    .processing_semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|error| error.to_string())?;

                let processed = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    process_image(source_bytes)
                })
                .await
                .map_err(|error| error.to_string())??;

                Ok(Arc::new(processed))
            })
            .await
            .clone()?;

        Ok(Image {
            epub_path: format!("images/{epub_name}.jpg"),
            bytes: result.bytes.clone(),
            mime_type: result.mime_type.clone(),
        })
    }
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

    async fn build(&self, client: &Client, image_downloader: &ImageDownloader) -> AppResult<Book> {
        let categories = try_join_all(
            self.categories
                .iter()
                .enumerate()
                .map(|(category_index, category)| async move {
                    let feeds = try_join_all(
                        category
                            .feeds
                            .iter()
                            .enumerate()
                            .map(|(feed_index, feed)| async move {
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
                                                    format!(
                                                        "Article {}",
                                                        article_index.saturating_add(1)
                                                    )
                                                });

                                            Some((article_index, title, link))
                                        })
                                        .take(10)
                                        .map(|(article_index, title, link)| async move {
                                            let html = client
                                                .get(&link)
                                                .send()
                                                .await?
                                                .error_for_status()?
                                                .text()
                                                .await?;

                                            let image_name_prefix = format!(
                                                "category-{category_index}-feed-{feed_index}-article-{article_index}"
                                            );

                                            process_article_html(
                                                &link,
                                                &html,
                                                &image_name_prefix,
                                                title,
                                                feed.2.as_deref(),
                                                image_downloader,
                                            )
                                            .await
                                        }),
                                )
                                .await?;

                                Ok::<RssFeed, AppError>(RssFeed {
                                    name: feed.0.clone(),
                                    articles,
                                })
                            }),
                    )
                    .await?;

                    Ok::<Category, AppError>(Category {
                        name: category.name.clone(),
                        feeds,
                    })
                }),
        )
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
    IMG_TAG_REGEX.replace_all(html, "<img$1 />").into_owned()
}

async fn process_article_html(
    page_url: &str,
    source_html: &str,
    image_name_prefix: &str,
    title: String,
    filter: Option<&str>,
    image_downloader: &ImageDownloader,
) -> AppResult<Article> {
    let base_url = Url::parse(page_url)?;
    let document = Html::parse_document(source_html);

    /*
     * Reuse the initial parsed DOM for both article extraction and image
     * discovery. This avoids parsing selected_html as a second fragment.
     */
    let (selected_html, image_sources) =
        if let Some(article) = document.select(&ARTICLE_SELECTOR).next() {
            (
                article.inner_html(),
                collect_image_sources_from_element(&article, &base_url),
            )
        } else {
            (
                source_html.to_string(),
                collect_image_sources_from_document(&document, &base_url),
            )
        };

    let downloaded_images = try_join_all(image_sources.into_iter().enumerate().map(
        |(image_index, (original_src, absolute_url))| {
            let epub_name = format!("{image_name_prefix}-image-{image_index}");

            async move {
                match image_downloader.download(&absolute_url, &epub_name).await {
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

    let rewritten_html =
        rewrite_article_html(&selected_html, image_replacements, filter)?.replace("&nbsp;", " ");

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

fn collect_image_sources_from_element(
    element: &ElementRef<'_>,
    base_url: &Url,
) -> Vec<(String, Url)> {
    let mut seen_urls = HashSet::new();

    element
        .select(&IMAGE_SELECTOR)
        .filter_map(|element| element.value().attr("src"))
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .filter_map(|src| {
            let absolute_url = resolve_image_url(base_url, src)?;
            let cache_key = absolute_url.as_str().to_string();

            seen_urls
                .insert(cache_key)
                .then(|| (src.to_string(), absolute_url))
        })
        .collect()
}

fn collect_image_sources_from_document(document: &Html, base_url: &Url) -> Vec<(String, Url)> {
    let mut seen_urls = HashSet::new();

    document
        .select(&IMAGE_SELECTOR)
        .filter_map(|element| element.value().attr("src"))
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .filter_map(|src| {
            let absolute_url = resolve_image_url(base_url, src)?;
            let cache_key = absolute_url.as_str().to_string();

            seen_urls
                .insert(cache_key)
                .then(|| (src.to_string(), absolute_url))
        })
        .collect()
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
                        || name == "role"
                        || name == "tabindex"
                        || name == "id"
                        || name == "class"
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

    let rewritten = rewrite_str(html, settings)?;

    /*
     * Perform one cleanup parse/pass rather than repeatedly reparsing or
     * repeatedly traversing until convergence.
     */
    let cleaned = cleanup_dom_single_pass(&rewritten);

    Ok(self_close_img_tags(&cleaned))
}

fn can_unwrap(element: &ElementRef<'_>) -> bool {
    matches!(element.value().name(), "div" | "span" | "section")
        && element.value().attrs().next().is_none()
        && element
            .children()
            .filter(|node| node.value().is_element())
            .count()
            == 1
        && !element.children().any(|node| {
            node.value()
                .as_text()
                .is_some_and(|text| !text.trim().is_empty())
        })
}

fn cleanup_dom_single_pass(html: &str) -> String {
    let mut document = Html::parse_fragment(html);

    let unwrap_ids = document
        .select(&WRAPPER_SELECTOR)
        .filter(can_unwrap)
        .map(|element| element.id())
        .collect::<Vec<_>>();

    for id in unwrap_ids {
        let Some(mut wrapper) = document.tree.get_mut(id) else {
            continue;
        };

        while let Some(mut child) = wrapper.first_child() {
            let child_id = child.id();
            child.detach();
            wrapper.insert_id_before(child_id);
        }

        wrapper.detach();
    }

    let empty_ids = document
        .select(&EMPTY_CANDIDATE_SELECTOR)
        .filter(element_is_empty)
        .map(|element| element.id())
        .collect::<Vec<_>>();

    if !empty_ids.is_empty() {
        let tree = HtmlTreeSink::new(document);

        for id in empty_ids {
            tree.remove_from_parent(&id);
        }

        document = tree.finish();
    }

    document.root_element().inner_html()
}

fn element_is_empty(element: &ElementRef<'_>) -> bool {
    if element.text().any(|text| !text.trim().is_empty()) {
        return false;
    }

    element
        .select(&MEANINGFUL_ELEMENT_SELECTOR)
        .next()
        .is_none()
}

fn process_image(source_bytes: bytes::Bytes) -> Result<ProcessedImage, String> {
    let reader = image::ImageReader::new(Cursor::new(source_bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;

    let mut image = reader.decode().map_err(|error| error.to_string())?;

    if image.width() > 780 {
        /*
         * Triangle is substantially faster than Lanczos3 and generally
         * sufficient for e-reader-sized images.
         */
        image = image.resize(780, 780, FilterType::Triangle);
    }

    let image = image.to_rgb8();
    let mut output = Vec::new();

    JpegEncoder::new_with_quality(&mut output, 65)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|error| error.to_string())?;

    Ok(ProcessedImage {
        bytes: output,
        mime_type: "image/jpeg".to_string(),
    })
}

async fn run(device_url: Option<String>) -> AppResult<()> {
    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_mins(1))
        .build()?;

    let image_downloader = ImageDownloader::new(client.clone());

    let book = BookBuilder::new()
        .category(
            "Blogs",
            vec![
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
        .build(&client, &image_downloader)
        .await?;

    for category in &book.categories {
        for feed in &category.feeds {
            for article in &feed.articles {
                println!("{}", article.title);
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
            upload(&epub, &device_url).await?;
            println!("done");
        }
    }

    println!("Complete");

    Ok(())
}

#[derive(Parser)]
#[command(name = "rssbook")]
#[command(about = "RSS feeds into an EPUB", version)]
struct Args {
    /// URL for `CrossPoint` device
    #[clap(long)]
    upload: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Args::parse();

    if let Err(error) = run(cli.upload).await {
        eprintln!("{error}");
    }
}
