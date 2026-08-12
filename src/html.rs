use ego_tree::NodeRef;
use futures::future::try_join_all;
use html5ever::tree_builder::TreeSink;
use lol_html::{RewriteStrSettings, element, rewrite_str};
use scraper::{ElementRef, Html, HtmlTreeSink, Node, Selector};
use url::Url;

use crate::{
    book::{Article, Image},
    error::{AppError, AppResult},
    image_download::ImageDownloader,
};
use std::collections::{HashMap, HashSet};

pub async fn process_article_html(
    page_url: &str,
    source_html: &str,
    image_name_prefix: &str,
    title: String,
    filter: Option<&str>,
    image_downloader: &ImageDownloader,
) -> AppResult<Article> {
    let base_url = Url::parse(page_url)?;
    let document = Html::parse_document(source_html);

    let selected_html = extract_article(&document);
    let wrapped = format!("<div>{}</div>", selected_html.html());
    let selected_html = Html::parse_fragment(&wrapped);

    let image_sources = collect_image_sources_from_document(&selected_html, &base_url);

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
        .map(|(original_src, image)| (original_src.clone(), image.epub_path().to_string()))
        .collect::<HashMap<_, _>>();

    let rewritten_html = rewrite_article_html(&selected_html.html(), image_replacements, filter)?;

    let images = downloaded_images
        .into_iter()
        .map(|(_, image)| image)
        .collect();

    Ok(Article::new(images, rewritten_html, title))
}

fn extract_article(html: &Html) -> Html {
    let selector = Selector::parse("article").expect("valid article selector");

    if let Some(article) = html.select(&selector).next() {
        return Html::parse_fragment(&article.html());
    }

    html.clone()
}

pub fn html_sanitation(html: &str) -> AppResult<String> {
    let html = html.replace("↩", "");
    let settings = RewriteStrSettings::new()
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
                        || name == "alt"
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
        .append_element_content_handler(element!("svg", |element| {
            element.remove();
            Ok(())
        }))
        .append_element_content_handler(element!("span", |element| {
            element.remove_and_keep_content();
            Ok(())
        }))
        .append_element_content_handler(element!("figure", |element| {
            element.remove_and_keep_content();
            Ok(())
        }))
        .append_element_content_handler(element!("picture", |element| {
            element.set_tag_name("p")?;
            Ok(())
        }))
        .append_element_content_handler(element!("figcaption", |element| {
            element.set_tag_name("p")?;
            Ok(())
        }));

    let rewritten = rewrite_str(&html, settings)?;

    let cleaned = cleanup_dom(&rewritten).replace("&nbsp;", " ");

    Ok(serialize_as_xhtml(&cleaned))
}

fn rewrite_article_html(
    html: &str,
    image_replacements: HashMap<String, String>,
    filter: Option<&str>,
) -> AppResult<String> {
    let html = if let Some(filter) = filter {
        let settings =
            RewriteStrSettings::new().append_element_content_handler(element!(filter, |element| {
                element.remove();
                Ok(())
            }));

        rewrite_str(html, settings)?
    } else {
        html.to_string()
    };

    let html = html_sanitation(&html)?;

    let settings =
        RewriteStrSettings::new().append_element_content_handler(element!("img", move |element| {
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
        }));

    Ok(rewrite_str(&html, settings)?)
}

fn cleanup_dom(html: &str) -> String {
    let mut current = html.to_string();

    loop {
        let next = cleanup_dom_single_pass(&current);

        if next == current {
            return next;
        }

        current = next;
    }
}

fn cleanup_dom_single_pass(html: &str) -> String {
    let mut document = Html::parse_fragment(html);

    let comment_ids = document
        .tree
        .nodes()
        .filter(|&node| matches!(node.value(), Node::Comment(_)))
        .map(|node| node.id())
        .collect::<Vec<_>>();

    for id in comment_ids {
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

    let wrapper_selector = Selector::parse("div, span, section").expect("valid wrapper selector");

    let unwrap_ids = document
        .select(&wrapper_selector)
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

    let empty_candidate_selector = Selector::parse(
        "div, span, section, article, main, header, footer, nav, \
         p, blockquote, pre, ul, ol, li, dl, dt, dd, a, figure, \
         figcaption, table, caption, colgroup, thead, tbody, tfoot, \
         tr, th, td, strong, em, b, i, u, s, small, mark, sub, sup, \
         q, cite, abbr",
    )
    .expect("valid empty candidate selector");

    let empty_ids = document
        .select(&empty_candidate_selector)
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

    let meaningful_element_selector = Selector::parse("img[src], br, hr").expect("valid selector");

    element
        .select(&meaningful_element_selector)
        .next()
        .is_none()
}

fn serialize_as_xhtml(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let root = document.root_element();
    let mut output = String::new();

    for child in root.children() {
        serialize_node(child, &mut output);
    }

    output
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

fn serialize_node(node: NodeRef<'_, Node>, output: &mut String) {
    let void_elements = [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];

    match node.value() {
        Node::Text(text) => {
            output.push_str(&escape_text(text.text.as_ref()));
        }
        Node::Element(element) => {
            let tag = element.name();

            output.push('<');
            output.push_str(tag);

            for (name, value) in element.attrs() {
                output.push(' ');
                output.push_str(name);
                output.push_str("=\"");
                output.push_str(&escape_attribute(value));
                output.push('"');
            }

            if void_elements.contains(&tag) {
                output.push_str(" />");
                return;
            }

            output.push('>');

            for child in node.children() {
                serialize_node(child, output);
            }

            output.push_str("</");
            output.push_str(tag);
            output.push('>');
        }
        _ => {
            for child in node.children() {
                serialize_node(child, output);
            }
        }
    }
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(value: &str) -> String {
    escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn collect_image_sources_from_document(document: &Html, base_url: &Url) -> Vec<(String, Url)> {
    let mut seen_urls = HashSet::new();

    let image_selector = Selector::parse("img[src]").expect("valid image selector");

    document
        .select(&image_selector)
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
