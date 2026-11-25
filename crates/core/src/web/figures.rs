//! A web page's pictures: which ones stand in the text with a caption, and
//! fetching them under the same limits as the page itself.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt as _;

use super::extract::extract;
use super::{Page, Reach, body, client, host_allowed};
use crate::doc::{self, CAPTION_CHARS, Doc, FIGURE_MIN_HEIGHT, FIGURE_MIN_WIDTH, Figure};

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
/// without it. The page chose these addresses, so only the public internet
/// is reached, except on the page's own host when `reach`, the page's own
/// limit, allowed it to be anywhere, as for a page on this machine.
pub async fn fetch_images(
    srcs: &[String],
    page_url: &str,
    reach: Reach,
) -> Vec<(String, &'static str, Vec<u8>)> {
    let (Ok(public), Ok(any)) = (
        client(Reach::Public).build(),
        client(Reach::Any)
            .redirect(reqwest::redirect::Policy::none())
            .build(),
    ) else {
        return Vec::new();
    };
    let page_host = reqwest::Url::parse(page_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string));
    let deadline = tokio::time::Instant::now() + IMAGES_TIME;
    futures::stream::iter(srcs.iter().take(IMAGES_MAX).cloned())
        .map(|src| {
            let own_host = page_host.clone().filter(|_| reach == Reach::Any);
            let (public, any) = (public.clone(), any.clone());
            let fetch = async move {
                let response = picture_response(&src, own_host.as_deref(), &public, &any)
                    .await?
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
                let bytes = body(response, IMAGE_BYTES_MAX).await.ok()??;
                Some((src, ext, bytes))
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

/// Most redirects followed for one picture on its page's own host.
const IMAGE_REDIRECTS_MAX: usize = 10;

/// Requests a picture. Only `own_host` may be off the public internet: `any`
/// follows no redirects, so each is looked at here, and one leaving that host
/// goes through `public`, which checks the rest of the way.
async fn picture_response(
    src: &str,
    own_host: Option<&str>,
    public: &reqwest::Client,
    any: &reqwest::Client,
) -> Option<reqwest::Response> {
    let mut url = reqwest::Url::parse(src).ok()?;
    for _ in 0..IMAGE_REDIRECTS_MAX {
        if url.host_str().is_none() || url.host_str() != own_host {
            if !host_allowed(url.as_str(), Reach::Public) {
                return None;
            }
            return public.get(url).send().await.ok();
        }
        let response = any.get(url.clone()).send().await.ok()?;
        if !response.status().is_redirection() {
            return Some(response);
        }
        let location = response.headers().get(reqwest::header::LOCATION)?;
        url = url.join(location.to_str().ok()?).ok()?;
    }
    None
}

/// A picture as extracted Markdown names it.
pub(super) struct Image {
    pub(super) alt: String,
    pub(super) src: String,
}

/// The pictures big enough to show, in text order. Extraction keeps some
/// images inline, where their marks place them, and drops most captioned
/// ones (Wikipedia's `<figure>` thumbnails) while keeping the caption, which
/// then places the picture. Markdown has no sizes, so they come from the
/// page's `<img>` tags; a picture without a known size is left out.
pub(super) fn figures(
    doc: &mut Doc,
    images: &[Image],
    html: &str,
    url: Option<&str>,
) -> Vec<Figure> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::MARK;

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
}
