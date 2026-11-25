//! EPUB books: the chapters named by the package's spine, read in order.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{
    Blocks, Converted, INFLATE_MAX, Package, attr, entity, first_attr, first_text, invalid, join,
    local, percent_decode, xml_error,
};
use crate::Result;

pub fn epub(bytes: &[u8]) -> Result<Converted> {
    let mut package = Package::open(bytes, "EPUB", INFLATE_MAX)?;
    let container = package
        .part("META-INF/container.xml")?
        .ok_or_else(|| invalid("EPUB", "it has no META-INF/container.xml"))?;
    let opf_path = first_attr(&container, "rootfile", "full-path")?
        .ok_or_else(|| invalid("EPUB", "its container names no package file"))?;
    let opf = package
        .part(&opf_path)?
        .ok_or_else(|| invalid("EPUB", &format!("it has no {opf_path}")))?;
    let base = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir);

    let mut manifest = HashMap::new();
    let mut spine = Vec::new();
    let mut reader = Reader::from_str(&opf);
    loop {
        match reader.read_event().map_err(|e| xml_error(&opf_path, e))? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => match local(&e) {
                "item" => {
                    if let (Some(id), Some(href), Some(media)) =
                        (attr(&e, "id"), attr(&e, "href"), attr(&e, "media-type"))
                    {
                        manifest.insert(id, (href, media));
                    }
                }
                "itemref" => spine.extend(attr(&e, "idref")),
                _ => {}
            },
            _ => {}
        }
    }

    let mut blocks = Blocks::default();
    for idref in &spine {
        let Some((href, media)) = manifest.get(idref) else {
            continue;
        };
        if !matches!(media.as_str(), "application/xhtml+xml" | "text/html") {
            continue;
        }
        let path = join(base, &percent_decode(href.split('#').next().unwrap_or("")));
        if let Some(chapter) = package.part(&path)? {
            xhtml(&chapter, &path, &mut blocks)?;
        }
    }
    Ok(Converted {
        title: first_text(&opf, "title"),
        markdown: blocks.out,
        images: blocks.images,
    })
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];

/// Appends one EPUB chapter. `check_end_names` is off because some books
/// ship XHTML with mismatched tags that reading systems accept anyway.
fn xhtml(xml: &str, path: &str, blocks: &mut Blocks) -> Result<()> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let folder = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    // `<img src>`, or `<image href>` inside an SVG, as is common for covers
    // and plates.
    let image = |e: &BytesStart, blocks: &mut Blocks| {
        let src = match local(e).to_ascii_lowercase().as_str() {
            "img" => attr(e, "src"),
            "image" => attr(e, "href"),
            _ => None,
        };
        if let Some(src) = src.filter(|s| !s.contains(':')) {
            let path = join(folder, &percent_decode(src.split('#').next().unwrap_or("")));
            blocks.image(path, attr(e, "alt").unwrap_or_default());
        }
    };
    loop {
        match reader.read_event().map_err(|e| xml_error(path, e))? {
            Event::Eof => break,
            Event::Start(e) => {
                image(&e, blocks);
                let name = local(&e).to_ascii_lowercase();
                match name.as_str() {
                    "head" | "script" | "style" | "rt" => {
                        reader
                            .read_to_end(e.name())
                            .map_err(|e| xml_error(path, e))?;
                    }
                    "li" => {
                        blocks.end();
                        blocks.prefix = "- ".into();
                    }
                    "br" => blocks.line_break(),
                    _ => {
                        if let Some(level) = heading_tag(&name) {
                            blocks.end();
                            blocks.level = level;
                        } else if BLOCK_TAGS.contains(&name.as_str()) {
                            blocks.end();
                        }
                    }
                }
            }
            Event::Empty(e) => {
                image(&e, blocks);
                let name = local(&e).to_ascii_lowercase();
                if name == "br" {
                    blocks.line_break();
                } else if BLOCK_TAGS.contains(&name.as_str()) {
                    blocks.end();
                }
            }
            Event::End(e) => {
                let name = e.local_name().into_inner().to_ascii_lowercase();
                if heading_tag(&name).is_some() {
                    blocks.end();
                    blocks.level = 0;
                } else if name == "li" {
                    blocks.end();
                    blocks.prefix.clear();
                } else if BLOCK_TAGS.contains(&name.as_str()) {
                    blocks.end();
                }
            }
            Event::Text(t) => blocks.push(&t.xml10_content()),
            Event::CData(t) => blocks.push(&t.xml10_content()),
            Event::GeneralRef(r) => blocks.push(&entity(&r)),
            _ => {}
        }
    }
    blocks.end();
    blocks.level = 0;
    blocks.prefix.clear();
    Ok(())
}

fn heading_tag(name: &str) -> Option<usize> {
    let n = name.strip_prefix('h')?.parse().ok()?;
    (1..=6).contains(&n).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::MARK;
    use crate::zipdoc::Image;
    use crate::zipdoc::tests::*;

    #[test]
    fn marks_epub_pictures_with_their_package_paths() {
        let container = r#"<container><rootfiles><rootfile full-path="OEBPS/book.opf"/></rootfiles></container>"#;
        let opf = r#"<package><manifest><item id="c" href="Text/c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#;
        let chapter = r#"<html><body><p>Before</p><p><img src="../Images/Map%20of%20Europe.png" alt="Europe in 1648"/></p><p>After</p><svg><image xlink:href="../Images/cover.jpg"/></svg></body></html>"#;
        let converted = epub(&package(&[
            ("META-INF/container.xml", container),
            ("OEBPS/book.opf", opf),
            ("OEBPS/Text/c1.xhtml", chapter),
        ]))
        .unwrap();
        assert_eq!(
            converted.images,
            vec![
                Image {
                    path: "OEBPS/Images/Map of Europe.png".into(),
                    alt: "Europe in 1648".into()
                },
                Image {
                    path: "OEBPS/Images/cover.jpg".into(),
                    alt: String::new()
                },
            ]
        );
        assert_eq!(
            converted.markdown,
            format!("Before\n\n{MARK}0{MARK}\n\nAfter\n\n{MARK}1{MARK}")
        );
    }

    #[test]
    fn reads_an_epub_in_spine_order() {
        let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>The Thirty Years&#8217; War</dc:title></metadata>
            <manifest><item id="one" href="Text/Chapter%201.xhtml" media-type="application/xhtml+xml"/><item id="two" href="Text/two.xhtml#top" media-type="application/xhtml+xml"/><item id="css" href="style.css" media-type="text/css"/></manifest>
            <spine><itemref idref="two"/><itemref idref="css"/><itemref idref="missing"/><itemref idref="one"/></spine></package>"#;
        let one = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ignored</title><style>p{}</style></head><body>
            <h2>The   Bohemian revolt</h2><p>Prague,&nbsp;1618 &mdash; a <em>window</em>.</p><ul><li><p>first</p></li><li>second</li></ul><p>Verse one<br/>verse two</p><script>x()</script></body></html>"#;
        let two = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><section><h1>Prologue</h1><div>Before <b>the war</b></div></section></body></html>"#;
        let bytes = package(&[
            ("META-INF/container.xml", container),
            ("OEBPS/content.opf", opf),
            ("OEBPS/Text/Chapter 1.xhtml", one),
            ("OEBPS/Text/two.xhtml", two),
        ]);
        let converted = epub(&bytes).unwrap();
        assert_eq!(converted.title.as_deref(), Some("The Thirty Years’ War"));
        assert_eq!(
            converted.markdown,
            "# Prologue\n\nBefore the war\n\n## The Bohemian revolt\n\n\
             Prague,\u{a0}1618 — a window.\n\n- first\n\n- second\n\nVerse one\nverse two"
                .replace('\u{a0}', " ")
        );
    }
}
