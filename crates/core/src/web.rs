//! Web pages: fetch the HTML once, keep it as the source file, and extract the
//! readable article as Markdown, without link targets and images.

use std::collections::HashMap;
use std::time::Duration;

use dom_smoothie::{Config, Readability, TextMode};
use futures::StreamExt as _;

use crate::doc::{self, CAPTION_CHARS, Doc, FIGURE_MIN_HEIGHT, FIGURE_MIN_WIDTH, Figure, MARK};
use crate::{Error, Result, youtube};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A fetched page, kept as bytes since a link may point at a PDF.
pub struct Page {
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Set instead of the page for a YouTube video: its captions.
    pub transcript: Option<youtube::Transcript>,
}

impl Page {
    pub fn is_pdf(&self) -> bool {
        self.content_type.starts_with("application/pdf") || self.bytes.starts_with(b"%PDF-")
    }

    pub fn html(&self) -> Result<String> {
        let ct = &self.content_type;
        if ct.is_empty() || ct.starts_with("text/") || ct.contains("html") || ct.contains("xml") {
            Ok(String::from_utf8_lossy(&self.bytes).into_owned())
        } else {
            Err(Error::Invalid(format!(
                "The link points at a {ct} file. Bowerbird reads web pages and PDFs."
            )))
        }
    }
}

/// Last path segment of a URL, as a fallback title for a linked file.
pub fn file_name(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()?
                .rev()
                .find(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| url.to_string())
}

pub async fn fetch(url: &str) -> Result<Page> {
    if let Some(id) = youtube::video_id(url) {
        return Ok(Page {
            content_type: String::new(),
            bytes: Vec::new(),
            transcript: Some(youtube::transcript(&id).await?),
        });
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("Bowerbird/", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    let response = crate::retry::with_retries(
        || async {
            client
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
        },
        |e: &reqwest::Error| crate::retry::transient_status(e.status().map(|s| s.as_u16())),
    )
    .await
    .map_err(|e| fetch_error(url, &e))?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let bytes = response.bytes().await.map_err(|e| fetch_error(url, &e))?;
    Ok(Page {
        content_type,
        bytes: bytes.to_vec(),
        transcript: None,
    })
}

/// Most pictures saved from one page.
const IMAGES_MAX: usize = 40;
/// Largest picture saved.
const IMAGE_BYTES_MAX: usize = 4 * 1024 * 1024;
/// Time for all of a page's pictures together, so they never hold up adding
/// the page for long.
const IMAGES_TIME: Duration = Duration::from_secs(20);

/// Addresses of the pictures `extract` will show from this page.
pub fn figure_sources(page: &Page, url: &str) -> Vec<String> {
    if page.transcript.is_some() || page.is_pdf() {
        return Vec::new();
    }
    page.html()
        .and_then(|html| extract(&html, Some(url)))
        .map(|(_, doc)| doc.figures.into_iter().map(|f| f.src).collect())
        .unwrap_or_default()
}

/// Fetches pictures to save beside a page, a few at a time: each as its
/// address, file extension and bytes. A picture that fails, isn't a format
/// browsers show, or is too large is left out; the page is worth adding
/// without it.
pub async fn fetch_images(srcs: &[String]) -> Vec<(String, &'static str, Vec<u8>)> {
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("Bowerbird/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return Vec::new();
    };
    let deadline = tokio::time::Instant::now() + IMAGES_TIME;
    futures::stream::iter(srcs.iter().take(IMAGES_MAX).cloned())
        .map(|src| {
            let client = client.clone();
            let fetch = async move {
                let response = client
                    .get(&src)
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?;
                let kind = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)?
                    .to_str()
                    .ok()?
                    .split(';')
                    .next()?
                    .trim()
                    .to_ascii_lowercase();
                let ext = match kind.as_str() {
                    "image/jpeg" => "jpg",
                    "image/png" => "png",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    _ => return None,
                };
                if response
                    .content_length()
                    .is_some_and(|n| n > IMAGE_BYTES_MAX as u64)
                {
                    return None;
                }
                let bytes = response.bytes().await.ok()?;
                (bytes.len() <= IMAGE_BYTES_MAX).then(|| (src, ext, bytes.to_vec()))
            };
            async move {
                tokio::time::timeout_at(deadline, fetch)
                    .await
                    .ok()
                    .flatten()
            }
        })
        .buffer_unordered(6)
        .filter_map(std::future::ready)
        .collect()
        .await
}

/// Says what went wrong in words a reader can act on, naming the site.
fn fetch_error(url: &str, error: &reqwest::Error) -> Error {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    let message = if error.is_builder() {
        format!("“{url}” isn't a web address Bowerbird can open.")
    } else if error.is_timeout() {
        format!(
            "{host} took longer than {} seconds to answer.",
            TIMEOUT.as_secs()
        )
    } else if let Some(status) = error.status() {
        match status.as_u16() {
            401 | 403 => format!("{host} refused access to this page ({status})."),
            404 | 410 => format!("{host} has no page at this address ({status})."),
            code => format!("{host} answered with an error ({code})."),
        }
    } else if error.is_connect() {
        format!("Couldn't connect to {host}. Check the address and your internet connection.")
    } else {
        format!("Couldn't read the page from {host}: {error}")
    };
    Error::Http(message)
}

/// Returns the article title and its text as a Markdown `Doc`, with the
/// pictures worth showing as figures whose files aren't saved yet.
pub fn extract(html: &str, url: Option<&str>) -> Result<(String, Doc)> {
    let config = Config {
        text_mode: TextMode::Markdown,
        ..Default::default()
    };
    let article = Readability::new(html, url, Some(config))
        .and_then(|mut r| r.parse())
        .map_err(|e| Error::Invalid(format!("Couldn't find readable text on this page: {e}")))?;
    let mut images = Vec::new();
    let mut doc = doc::markdown(clean(&article.text_content, &mut images));
    doc.figures = figures(&mut doc, &images, html, url);
    Ok((article.title, doc))
}

/// Extracted Markdown made fit for reading and prompting. Infoboxes are read
/// after `tidy`, which turns the `<br>` inside their cells into line breaks.
fn clean(md: &str, images: &mut Vec<Image>) -> String {
    infoboxes(&tidy(md, images))
}

/// A picture as extracted Markdown names it.
struct Image {
    alt: String,
    src: String,
}

/// The pictures big enough to show, in text order. Extraction keeps some
/// images inline, where their marks place them, and drops most captioned
/// ones (Wikipedia's `<figure>` thumbnails) while keeping the caption, which
/// then places the picture. Markdown has no sizes, so they come from the
/// page's `<img>` tags; a picture without a known size is left out.
fn figures(doc: &mut Doc, images: &[Image], html: &str, url: Option<&str>) -> Vec<Figure> {
    let tags = img_tags(html, url);
    let shown = |tag: &ImgTag| {
        tag.width >= FIGURE_MIN_WIDTH && tag.height >= FIGURE_MIN_HEIGHT && !is_svg(&tag.best)
    };
    let mut out: Vec<Figure> = Vec::new();
    for (number, start) in doc.take_marks() {
        let Some(image) = images.get(number) else {
            continue;
        };
        let Some(tag) = resolve(&image.src, url).and_then(|src| tags.get(&src)) else {
            continue;
        };
        if !shown(tag) || out.iter().any(|f| f.src == tag.best) {
            continue;
        }
        let caption = doc::caption(&image.alt, &doc.text[start..]);
        out.push(Figure {
            start,
            src: tag.best.clone(),
            file: String::new(),
            caption,
            width: tag.width,
            height: tag.height,
        });
    }
    for (tag, caption) in captioned_images(html, url) {
        if !shown(&tag) || out.iter().any(|f| f.src == tag.best) {
            continue;
        }
        let Some(start) = find_caption(&doc.text, &caption) else {
            continue;
        };
        let caption = caption.lines().next().unwrap_or_default();
        out.push(Figure {
            start,
            src: tag.best,
            file: String::new(),
            caption: match caption.char_indices().nth(CAPTION_CHARS) {
                Some((cut, _)) => format!("{}…", caption[..cut].trim_end()),
                None => caption.to_string(),
            },
            width: tag.width,
            height: tag.height,
        });
    }
    out.sort_by_key(|f| f.start);
    out
}

/// Pictures followed by their caption in the page, before the next picture:
/// a `<figure>`'s `<figcaption>`, an infobox image's caption, or else the
/// text after the picture in its own table cell, where some infoboxes set it.
fn captioned_images(html: &str, base: Option<&str>) -> Vec<(ImgTag, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = html[at..].find("<img") {
        let start = at + found + 4;
        let end = html[start..].find('>').map_or(html.len(), |e| start + e);
        at = end;
        let next_image = html[end..].find("<img").map_or(html.len(), |n| end + n);
        let window = &html[end..next_image];
        let caption = [
            "<figcaption",
            "class=\"infobox-caption",
            "class=\"thumbcaption",
        ]
        .iter()
        .filter_map(|marker| window.find(marker))
        .min()
        .and_then(|c| {
            // The caption element: the tag that opens at or before the marker.
            let tag_at = window[..=c].rfind('<')?;
            let name: String = window[tag_at + 1..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            let open = c + window[c..].find('>')? + 1;
            let close = open + window[open..].find(&format!("</{name}>"))?;
            Some(inner_text(&window[open..close]))
        })
        .or_else(|| {
            window
                .find("</td>")
                .map(|close| inner_text(&window[..close]))
        });
        if let (Some(caption), Some((_, tag))) = (caption, img_tag(&html[start..end], base))
            && !caption.is_empty()
        {
            out.push((tag, caption));
        }
    }
    out
}

/// Text inside an HTML fragment: tags removed, line breaks made spaces,
/// common entities decoded, whitespace collapsed.
fn inner_text(fragment: &str) -> String {
    let mut text = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for (i, c) in fragment.char_indices() {
        match c {
            '<' if fragment[i..].starts_with("<br") => {
                in_tag = true;
                text.push(' ');
            }
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the line holding `caption` starts in `text`, matching its first
/// words while ignoring Markdown emphasis and line breaks. Captions of
/// fewer than three words are too common to place, and ones under five
/// words must start their line, as a caption does, rather than sit inside a
/// list or a sentence that names the same thing.
fn find_caption(text: &str, caption: &str) -> Option<usize> {
    const WORDS: usize = 8;
    let words: Vec<&str> = caption.split_whitespace().take(WORDS).collect();
    if words.len() < 3 {
        return None;
    }
    let wanted = words.join(" ");
    // Squeeze the text the same way, remembering where each byte came from.
    let mut squeezed = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    let mut space = false;
    for (i, c) in text.char_indices() {
        if matches!(c, '*' | '_') {
            continue;
        }
        if c.is_whitespace() {
            space = !squeezed.is_empty();
            continue;
        }
        if space {
            squeezed.push(' ');
            origin.push(i);
            space = false;
        }
        squeezed.push(c);
        origin.extend(std::iter::repeat_n(i, c.len_utf8()));
    }
    squeezed.match_indices(&wanted).find_map(|(at, _)| {
        let found = origin[at];
        let line = text[..found].rfind('\n').map_or(0, |n| n + 1);
        (words.len() >= 5 || text[line..found].trim().is_empty()).then_some(line)
    })
}

/// What an `<img>` tag says about its picture.
struct ImgTag {
    width: u32,
    height: u32,
    /// The largest candidate in `srcset`, else `src`.
    best: String,
}

/// The page's `<img>` tags by resolved `src`.
fn img_tags(html: &str, base: Option<&str>) -> HashMap<String, ImgTag> {
    let mut out = HashMap::new();
    let mut rest = html;
    while let Some(at) = rest.find("<img") {
        rest = &rest[at + 4..];
        let end = rest.find('>').unwrap_or(rest.len());
        if let Some((src, tag)) = img_tag(&rest[..end], base) {
            out.entry(src).or_insert(tag);
        }
        rest = &rest[end..];
    }
    out
}

/// The resolved `src` of an `<img>` tag, given what is inside it, and what
/// the tag says about the picture.
fn img_tag(inside: &str, base: Option<&str>) -> Option<(String, ImgTag)> {
    let src = attribute(inside, "src").and_then(|s| resolve(&s, base))?;
    let number = |name| {
        attribute(inside, name)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let best = attribute(inside, "srcset")
        .and_then(|set| largest_candidate(&set))
        .and_then(|u| resolve(&u, base))
        .unwrap_or_else(|| src.clone());
    let tag = ImgTag {
        width: number("width"),
        height: number("height"),
        best,
    };
    Some((src, tag))
}

/// The value of attribute `name` in the inside of an HTML tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let at = rest.find(name)?;
        let before = rest[..at].chars().next_back();
        rest = &rest[at + name.len()..];
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value = &value[1..];
        let end = value.find(quote)?;
        return Some(value[..end].replace("&amp;", "&"));
    }
}

/// The candidate in a `srcset` with the largest width or density.
fn largest_candidate(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .filter_map(|candidate| {
            let mut parts = candidate.split_whitespace();
            let url = parts.next()?;
            let size = parts
                .next()
                .and_then(|d| d.trim_end_matches(['x', 'w']).parse::<f64>().ok())
                .unwrap_or(1.0);
            Some((size, url))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, url)| url.to_string())
}

/// `src` as an absolute http(s) address.
fn resolve(src: &str, base: Option<&str>) -> Option<String> {
    let url = match base.and_then(|b| reqwest::Url::parse(b).ok()) {
        Some(base) => base.join(src).ok()?,
        None => reqwest::Url::parse(src).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn is_svg(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(".svg"))
}

/// Wikipedia's infoboxes come out of extraction as a table whose header row
/// holds every label and whose rows each hold one value, in the same order,
/// so nothing reads as paired. Each becomes "label: value" lines, with the
/// image caption, the one value more than there are labels, on its own line.
/// Tables with several cells to a row are left as they are.
fn infoboxes(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        match infobox(&lines, i) {
            Some((block, next)) => {
                out.push(block);
                i = next;
            }
            None => {
                out.push(lines[i].to_string());
                i += 1;
            }
        }
    }
    out.join("\n")
}

/// The infobox whose header row starts at line `at`, rewritten, and the
/// index of the line after it.
fn infobox(lines: &[&str], at: usize) -> Option<(String, usize)> {
    let (header, after_header) = table_row(lines, at)?;
    let divider = lines.get(after_header)?.trim();
    if !divider.starts_with('|')
        || !divider.contains('-')
        || !divider.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
    {
        return None;
    }
    let labels: Vec<String> = header.split('|').map(one_line).collect();
    let mut values = Vec::new();
    let mut at = after_header + 1;
    while let Some((cell, next)) = table_row(lines, at) {
        if cell.contains('|') {
            return None;
        }
        values.push(one_line(&cell));
        at = next;
    }
    let caption = match values.len().checked_sub(labels.len())? {
        0 => None,
        1 => Some(values.remove(0)),
        _ => return None,
    };
    let mut block = String::new();
    if let Some(caption) = caption.filter(|c| !c.is_empty()) {
        block.push_str(&caption);
        block.push_str("\n\n");
    }
    for (label, value) in labels.iter().zip(&values) {
        block.push_str(&format!("- {label}: {value}\n"));
    }
    Some((block.trim_end().to_string(), at))
}

/// The cells of a table row starting at line `at`, without its outer pipes,
/// and the index of the line after it. A cell may run over several lines.
fn table_row(lines: &[&str], at: usize) -> Option<(String, usize)> {
    let mut row = lines.get(at)?.trim_start().strip_prefix('|')?.to_string();
    let mut next = at + 1;
    while !row.trim_end().ends_with('|') {
        row.push('\n');
        row.push_str(lines.get(next)?);
        next += 1;
    }
    let row = row.trim_end();
    Some((row[..row.len() - 1].to_string(), next))
}

/// A cell's lines joined; the items of a "+ " list inside it are separated
/// with semicolons, other line breaks become spaces.
fn one_line(cell: &str) -> String {
    let lines: Vec<&str> = cell
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let bullet = |l: &&str| l.starts_with(['+', '-', '*']);
    if lines.iter().any(bullet) {
        lines
            .iter()
            .map(|l| l.trim_start_matches(['+', '-', '*']).trim())
            .collect::<Vec<_>>()
            .join("; ")
    } else {
        lines.join(" ")
    }
}

/// Keeps the words of extracted Markdown and drops what only a browser needs:
/// link targets, footnote links, backslash escapes, and `<br>`. Left in, they
/// clutter passages, add tokens to every prompt, and split quotes so they no
/// longer match the text the model saw. Each image becomes a mark (see
/// `doc::MARK`) numbering it in `images`.
fn tidy(md: &str, images: &mut Vec<Image>) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(at) = rest.find(['[', '!', '\\', '<']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(tail) = edit_link(rest) {
            rest = tail.strip_prefix("\n\n").unwrap_or(tail);
        } else if let Some(after) = rest.strip_prefix('!')
            && let Some((alt, target, tail)) = link(after)
        {
            let src = target.split_whitespace().next().unwrap_or_default();
            out.push_str(&format!("{MARK}{}{MARK}", images.len()));
            images.push(Image {
                alt: tidy(alt, &mut Vec::new()),
                src: src.to_string(),
            });
            rest = tail;
        } else if let Some((text, target, tail)) = link(rest) {
            if !target.starts_with("#cite_") {
                out.push_str(&tidy(text, images));
            }
            rest = tail;
        } else if let Some(c) = rest
            .strip_prefix('\\')
            .and_then(|r| r.chars().next())
            .filter(char::is_ascii_punctuation)
        {
            out.push(c);
            rest = &rest[1 + c.len_utf8()..];
        } else if let Some(tail) = ["<br>", "<br/>", "<br />"]
            .iter()
            .find_map(|tag| rest.strip_prefix(tag))
        {
            out.push('\n');
            rest = tail;
        } else {
            let c = rest.chars().next().expect("found a character");
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.push_str(rest);
    out
}

/// What follows a MediaWiki section's `[edit]` link at the start of `s`,
/// which extraction writes with its brackets escaped.
fn edit_link(s: &str) -> Option<&str> {
    let (_, target, tail) = link(s.strip_prefix("\\[")?)?;
    target
        .contains("action=edit")
        .then(|| tail.strip_prefix("\\]"))
        .flatten()
}

/// A Markdown link at the start of `s`: its text, its target, and what follows.
/// Brackets in the text may be escaped, and parentheses in the target nest, as
/// in Wikipedia's `Foo_(bar)` pages.
fn link(s: &str) -> Option<(&str, &str, &str)> {
    let body = s.strip_prefix('[')?;
    let mut depth = 1;
    let mut escaped = false;
    let mut close = None;
    for (i, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    let close = close?;
    let target_part = body[close + 1..].strip_prefix('(')?;
    let mut depth = 1;
    for (i, c) in target_part.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let target = target_part[..i].split_whitespace().next().unwrap_or("");
                    return Some((&body[..close], target, &target_part[i + 1..]));
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_keeps_link_words_and_drops_link_targets() {
        let md = "Built after [Berlin](https://en.wikipedia.org/wiki/Berlin \"Berlin\") was split\\.[\\[28\\]](#cite_note-28)\n\
                  ![](https://upload.example/flag.gif) [French Guards](https://en.wikipedia.org/wiki/Gardes_Fran%C3%A7aises \"Gardes\")<br>\
                  [Mercury (planet)](https://en.wikipedia.org/wiki/Mercury_(planet)) and [not a link] (1961)";
        let mut images = Vec::new();
        assert_eq!(
            tidy(md, &mut images),
            format!(
                "Built after Berlin was split.\n{MARK}0{MARK} French Guards\nMercury (planet) and [not a link] (1961)"
            )
        );
        assert_eq!(images[0].src, "https://upload.example/flag.gif");
        assert_eq!(
            tidy("[![logo](https://x/logo.png)](https://x)", &mut images),
            format!("{MARK}1{MARK}"),
            "a linked image leaves only its mark"
        );
        assert_eq!(images[1].alt, "logo");
    }

    #[test]
    fn pictures_become_sized_figures_where_they_stood() {
        let words = "The city held out through the spring while the army dug in around its walls. "
            .repeat(12);
        let html = format!(
            r#"<html><head><title>Siege of Magdeburg</title></head><body><article>
<h1>Siege of Magdeburg</h1><p>{words}</p>
<figure><a href="/wiki/File:Sack.jpg"><img src="//upload.example/thumb/Sack.jpg/250px-Sack.jpg"
 srcset="//upload.example/thumb/Sack.jpg/375px-Sack.jpg 1.5x, //upload.example/thumb/Sack.jpg/500px-Sack.jpg 2x"
 width="250" height="180" alt=""></a><figcaption>The sack of Magdeburg in 1631</figcaption></figure>
<p>{words} <img src="/static/icon.png" width="20" height="20" alt="icon"> {words}</p>
<p><img src="/maps/Europe.svg" width="400" height="300" alt="Map of Europe"></p>
</article></body></html>"#
        );
        let (_, doc) = extract(&html, Some("https://en.example.org/wiki/Siege")).unwrap();
        assert!(
            !doc.text.contains(MARK) && !doc.text.contains("!["),
            "{}",
            doc.text
        );
        let [figure] = doc.figures.as_slice() else {
            panic!("one figure: {:?}", doc.figures);
        };
        assert_eq!(
            figure.src,
            "https://upload.example/thumb/Sack.jpg/500px-Sack.jpg"
        );
        assert_eq!((figure.width, figure.height), (250, 180));
        assert_eq!(figure.caption, "The sack of Magdeburg in 1631");
        assert!(
            doc.text[figure.start..]
                .trim_start()
                .starts_with("The sack of Magdeburg")
        );
    }

    #[test]
    fn an_infobox_picture_is_placed_by_the_caption_in_its_cell() {
        let words =
            "The war began as a revolt in Bohemia and spread across the Empire. ".repeat(12);
        let html = format!(
            r#"<html><head><title>Thirty Years' War</title></head><body><article>
<h1>Thirty Years' War</h1>
<table class="infobox"><tbody><tr><th colspan="2">Thirty Years' War</th></tr>
<tr><td colspan="2">Part of the European wars of religion</td></tr>
<tr><td colspan="2"><span typeof="mw:File/Frameless"><a href="/wiki/File:Siege.jpg"><img
 src="//upload.example/thumb/Siege.jpg/330px-Siege.jpg" srcset="//upload.example/thumb/Siege.jpg/960px-Siege.jpg 2x"
 width="300" height="198" alt=""></a></span><br/><i>The Siege (Defence of a Church Courtyard During the
 Thirty Years' War)</i><br/><a href="/wiki/Lessing">Karl Friedrich Lessing</a>, 1848</td></tr>
<tr><th>Date</th><td>23 May 1618 – 24 October 1648</td></tr></tbody></table>
<p>{words}</p><p>{words}</p>
</article></body></html>"#
        );
        let (_, doc) = extract(&html, Some("https://en.example.org/wiki/War")).unwrap();
        let [figure] = doc.figures.as_slice() else {
            panic!("one figure: {:?}\n{}", doc.figures, doc.text);
        };
        assert_eq!(
            figure.src,
            "https://upload.example/thumb/Siege.jpg/960px-Siege.jpg"
        );
        assert_eq!(
            figure.caption,
            "The Siege (Defence of a Church Courtyard During the Thirty Years' War) Karl Friedrich Lessing, 1848"
        );
        let line = doc.text[figure.start..].lines().next().unwrap_or_default();
        assert!(line.contains("The Siege (Defence"), "{}", doc.text);
    }

    #[test]
    fn infoboxes_pair_each_label_with_its_value() {
        let md = "Intro.\n\n| Date | Location | Territorial  \nchanges |\n| ---- | -------- | ---- |\n\
                  | 23 May 1618 – 24 October 1648 |\n| Central Europe |\n| + France annexes\n+ Sweden gains\n |\n\nBelligerents";
        assert_eq!(
            infoboxes(md),
            "Intro.\n\n- Date: 23 May 1618 – 24 October 1648\n- Location: Central Europe\n\
             - Territorial changes: France annexes; Sweden gains\n\nBelligerents"
        );
        assert_eq!(
            clean(
                "| Date | Territorial <br>changes |\n| -- | -- |\n| [1648](https://x/1648) |\n| + Alsace<br>+ Wismar |",
                &mut Vec::new()
            ),
            "- Date: 1648\n- Territorial changes: Alsace; Wismar"
        );
        let captioned =
            "| Date |\n| ---- |\n| A leaflet of 1618 |\n| + 30 July 1419\n+ 23 May 1618\n |\n";
        assert_eq!(
            infoboxes(captioned),
            "A leaflet of 1618\n\n- Date: 30 July 1419; 23 May 1618\n"
        );
        for table in [
            "| Year | Event |\n| --- | --- |\n| 1618 | Revolt |\n| 1648 | Peace |",
            "| Name |\n| --- |\n| a |\n| b |\n| c |",
            "| not | a table |\nbecause no divider",
        ] {
            assert_eq!(infoboxes(table), table);
        }
    }

    #[test]
    fn mediawiki_edit_links_are_left_out() {
        let md = "## Background: 1556 to 1618\n\n\\[[edit](https://en.wikipedia.org/w/index.php?title=Thirty_Years%27_War&action=edit&section=2 \"Edit section: Background: 1556 to 1618\")\\]\n\nMajor developments in the war.";
        assert_eq!(
            tidy(md, &mut Vec::new()),
            "## Background: 1556 to 1618\n\nMajor developments in the war."
        );
    }
}
