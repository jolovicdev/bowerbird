//! Text from documents stored as zip packages: Word (.docx) and EPUB. Both
//! become Markdown with headings, so the heading tree, chunking, and
//! citations work as they do for Markdown sources.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::doc::MARK;
use crate::{Error, Result};

/// Most bytes inflated from one package. Real books and reports are far
/// smaller; the cap stops a small zip that inflates to gigabytes.
const INFLATE_MAX: u64 = 256 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Converted {
    /// From the package's own metadata, when it names one.
    pub title: Option<String>,
    /// Pictures stand in it as marks (see `doc::MARK`) numbering `images`.
    pub markdown: String,
    pub images: Vec<Image>,
}

/// A picture inside the package.
#[derive(Debug, PartialEq)]
pub struct Image {
    /// Path of the picture inside the package.
    pub path: String,
    pub alt: String,
}

/// Largest picture read from a package.
const IMAGE_BYTES_MAX: u64 = 8 * 1024 * 1024;

pub fn docx(bytes: &[u8]) -> Result<Converted> {
    let mut package = Package::open(bytes, "Word document", INFLATE_MAX)?;
    let body = package
        .part("word/document.xml")?
        .ok_or_else(|| invalid("Word document", "it has no word/document.xml"))?;
    let levels = match package.part("word/styles.xml")? {
        Some(styles) => heading_styles(&styles)?,
        None => HashMap::new(),
    };
    let title = package
        .part("docProps/core.xml")?
        .and_then(|core| first_text(&core, "title"));
    let rels = match package.part("word/_rels/document.xml.rels")? {
        Some(rels) => relationships(&rels)?,
        None => HashMap::new(),
    };
    let blocks = docx_body(&body, &levels, &rels)?;
    Ok(Converted {
        title,
        markdown: blocks.out,
        images: blocks.images,
    })
}

/// Pictures a Word document links by relationship id, as package paths.
fn relationships(xml: &str) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/_rels/document.xml.rels", e))?
        {
            Event::Eof => return Ok(out),
            Event::Start(e) | Event::Empty(e) if local(&e) == "Relationship" => {
                let external = attr(&e, "TargetMode").is_some_and(|m| m == "External");
                if let (Some(id), Some(target), false) =
                    (attr(&e, "Id"), attr(&e, "Target"), external)
                {
                    let path = match target.strip_prefix('/') {
                        Some(absolute) => absolute.to_string(),
                        None => join("word", &target),
                    };
                    out.insert(id, path);
                }
            }
            _ => {}
        }
    }
}

/// The bytes of a picture inside a package, unless it is missing or too large.
pub fn read_image(package: &[u8], path: &str) -> Option<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(package)).ok()?;
    let file = zip.by_name(path).ok()?;
    let mut bytes = Vec::new();
    file.take(IMAGE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= IMAGE_BYTES_MAX).then_some(bytes)
}

/// The format of a PNG, JPEG, GIF or WebP picture, as a file extension, and
/// its size in pixels, read from its header. Other formats give `None`.
pub fn picture_kind(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let at = |i: usize, n: usize| bytes.get(i..i + n);
    let be16 = |i| at(i, 2).map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])));
    let le16 = |i| at(i, 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    let le24 = |i| at(i, 3).map(|b| u32::from_le_bytes([b[0], b[1], b[2], 0]));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let be32 = |i| at(i, 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        return Some(("png", be32(16)?, be32(20)?));
    }
    if bytes.starts_with(b"GIF8") {
        return Some(("gif", le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"RIFF") && at(8, 4)? == b"WEBP" {
        return match at(12, 4)? {
            b"VP8 " => Some(("webp", le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(at(21, 4)?.try_into().ok()?);
                Some(("webp", (bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some(("webp", le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        // Walk the segments to a start-of-frame, which holds the size.
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff {
                return None;
            }
            let marker = bytes[i + 1];
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                return Some(("jpg", be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

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

struct Package<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    /// Bytes still allowed to be inflated.
    budget: u64,
}

impl<'a> Package<'a> {
    fn open(bytes: &'a [u8], what: &str, budget: u64) -> Result<Self> {
        let zip = ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| invalid(what, "it isn't a zip package"))?;
        Ok(Package { zip, budget })
    }

    /// A part as text, or `None` when the package doesn't have it.
    fn part(&mut self, name: &str) -> Result<Option<String>> {
        let file = match self.zip.by_name(name) {
            Ok(file) => file,
            Err(ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(Error::Invalid(format!("Can't read {name}: {e}"))),
        };
        let mut raw = Vec::new();
        file.take(self.budget + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 > self.budget {
            return Err(Error::Invalid(format!(
                "The document unpacks to more than {} MB of text; Bowerbird reads smaller files.",
                INFLATE_MAX >> 20
            )));
        }
        self.budget -= raw.len() as u64;
        let text = String::from_utf8_lossy(&raw);
        Ok(Some(text.trim_start_matches('\u{feff}').to_string()))
    }
}

/// Markdown written block by block, with whitespace collapsed as HTML does.
#[derive(Default)]
struct Blocks {
    out: String,
    line: String,
    space: bool,
    /// Heading level of the block being written, 0 for body text.
    level: usize,
    /// Written before a body block, such as a list marker.
    prefix: String,
    images: Vec<Image>,
}

impl Blocks {
    /// Marks where a picture stands in the text.
    fn image(&mut self, path: String, alt: String) {
        self.push(&format!("{MARK}{}{MARK}", self.images.len()));
        self.images.push(Image { path, alt });
    }

    fn push(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_whitespace() {
                self.space = !self.line.is_empty();
            } else {
                if self.space {
                    self.line.push(' ');
                    self.space = false;
                }
                self.line.push(c);
            }
        }
    }

    fn line_break(&mut self) {
        if !self.line.is_empty() {
            self.line.push(if self.level > 0 { ' ' } else { '\n' });
            self.space = false;
        }
    }

    /// Ends the current block. Level and prefix stay until the caller clears
    /// them, so a list item made of several paragraphs keeps its marker.
    fn end(&mut self) {
        let text = std::mem::take(&mut self.line);
        self.space = false;
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.level > 0 {
            self.raw(&format!("{} {text}", "#".repeat(self.level.min(6))));
        } else {
            // A body paragraph that starts like a heading or a code fence
            // would change the heading tree of everything after it.
            let escape = if text.starts_with(['#', '`']) {
                "\\"
            } else {
                ""
            };
            self.raw(&format!("{}{escape}{text}", self.prefix));
        }
    }

    fn raw(&mut self, block: &str) {
        if !self.out.is_empty() {
            self.out.push_str("\n\n");
        }
        self.out.push_str(block);
    }
}

/// Heading level of each paragraph style, from its name ("heading 2"; Word
/// writes built-in names in English whatever the interface language) or its
/// outline level, following `basedOn` for styles that inherit one.
fn heading_styles(xml: &str) -> Result<HashMap<String, usize>> {
    #[derive(Default)]
    struct Style {
        level: Option<usize>,
        based_on: Option<String>,
    }
    let mut styles: HashMap<String, Style> = HashMap::new();
    let mut current: Option<String> = None;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/styles.xml", e))?
        {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                if local(&e) == "style" {
                    current = (attr(&e, "type").as_deref() == Some("paragraph"))
                        .then(|| attr(&e, "styleId"))
                        .flatten();
                    if let Some(id) = &current {
                        styles.entry(id.clone()).or_default();
                    }
                    continue;
                }
                let Some(style) = current.as_ref().and_then(|id| styles.get_mut(id)) else {
                    continue;
                };
                match local(&e) {
                    "name" => {
                        let name = attr(&e, "val").unwrap_or_default().to_lowercase();
                        if name == "title" {
                            style.level = Some(1);
                        } else if let Some(n) = name.strip_prefix("heading ") {
                            style.level = n.parse().ok().filter(|n| (1..=6).contains(n));
                        }
                    }
                    "outlineLvl" => {
                        if let Some(n) = attr(&e, "val").and_then(|v| v.parse::<usize>().ok()) {
                            style.level = (n < 6).then_some(n + 1);
                        }
                    }
                    "basedOn" => style.based_on = attr(&e, "val"),
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().into_inner() == "style" => current = None,
            _ => {}
        }
    }
    let mut levels = HashMap::new();
    for id in styles.keys() {
        let mut at = id;
        // A style chain longer than this is a loop.
        for _ in 0..10 {
            let Some(style) = styles.get(at) else { break };
            if let Some(level) = style.level {
                levels.insert(id.clone(), level);
                break;
            }
            let Some(parent) = &style.based_on else { break };
            at = parent;
        }
    }
    Ok(levels)
}

fn docx_body(
    xml: &str,
    levels: &HashMap<String, usize>,
    rels: &HashMap<String, String>,
) -> Result<Blocks> {
    let mut blocks = Blocks::default();
    // A drawing's alt text comes before the picture it describes.
    let mut alt = String::new();
    let mut reader = Reader::from_str(xml);
    let (mut paragraphs, mut runs, mut tables) = (0, 0, 0);
    let mut in_text = false;
    let mut list_level: Option<usize> = None;
    let mut table: Vec<Vec<String>> = Vec::new();
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/document.xml", e))?
        {
            Event::Eof => break,
            Event::Start(e) => match local(&e) {
                "p" => {
                    paragraphs += 1;
                    if paragraphs == 1 {
                        list_level = None;
                    }
                }
                "r" => runs += 1,
                "t" if runs > 0 => in_text = true,
                "tbl" => {
                    tables += 1;
                    if tables == 1 {
                        blocks.end();
                        table.clear();
                    }
                }
                "tr" if tables == 1 => table.push(Vec::new()),
                "tc" if tables == 1 => {
                    if let Some(row) = table.last_mut() {
                        row.push(String::new());
                    }
                }
                // Word stores a text box twice, as a drawing and as a VML
                // fallback, and tracked moves keep the text at its old place
                // too; reading only one copy keeps the text once.
                "Fallback" | "moveFrom" => {
                    reader
                        .read_to_end(e.name())
                        .map_err(|e| xml_error("word/document.xml", e))?;
                }
                _ => {}
            },
            Event::Empty(e) => match local(&e) {
                "pStyle" if paragraphs == 1 => {
                    blocks.level = attr(&e, "val")
                        .and_then(|id| levels.get(&id).copied())
                        .unwrap_or(0);
                }
                "outlineLvl" if paragraphs == 1 => {
                    if let Some(n) = attr(&e, "val").and_then(|v| v.parse::<usize>().ok()) {
                        blocks.level = if n < 6 { n + 1 } else { 0 };
                    }
                }
                "ilvl" if paragraphs == 1 => {
                    list_level = attr(&e, "val").and_then(|v| v.parse().ok());
                }
                "docPr" => alt = attr(&e, "descr").unwrap_or_default(),
                "blip" => {
                    if let Some(path) = attr(&e, "embed").and_then(|id| rels.get(&id)) {
                        blocks.image(path.clone(), std::mem::take(&mut alt));
                    }
                }
                "tab" if runs > 0 => blocks.push(" "),
                "br" | "cr" if runs > 0 => blocks.line_break(),
                "noBreakHyphen" if runs > 0 => blocks.push("-"),
                _ => {}
            },
            Event::Text(t) if in_text => blocks.push(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => blocks.push(&entity(&r)),
            Event::End(e) => match e.local_name().into_inner() {
                "t" => in_text = false,
                "r" => runs = (runs - 1).max(0),
                "p" => {
                    paragraphs -= 1;
                    if paragraphs > 0 {
                        blocks.push(" ");
                    } else if tables > 0 {
                        let text = std::mem::take(&mut blocks.line);
                        blocks.space = false;
                        if let Some(cell) = table.last_mut().and_then(|row| row.last_mut()) {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(text.trim());
                        }
                    } else {
                        if blocks.level == 0
                            && let Some(depth) = list_level
                        {
                            blocks.prefix = format!("{}- ", "  ".repeat(depth));
                        }
                        blocks.end();
                        blocks.level = 0;
                        blocks.prefix.clear();
                    }
                }
                "tbl" => {
                    tables -= 1;
                    if tables == 0 {
                        blocks.level = 0;
                        if let Some(markdown) = markdown_table(&table) {
                            blocks.raw(&markdown);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(blocks)
}

fn markdown_table(rows: &[Vec<String>]) -> Option<String> {
    let width = rows.iter().map(Vec::len).max().filter(|&w| w > 0)?;
    if rows.iter().flatten().all(|cell| cell.is_empty()) {
        return None;
    }
    let line = |row: &[String]| {
        let cells: Vec<String> = (0..width)
            .map(|i| row.get(i).map_or(String::new(), |c| c.replace('|', "\\|")))
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    out.extend(rows[1..].iter().map(|row| line(row)));
    Some(out.join("\n"))
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

fn local<'e>(e: &'e BytesStart) -> &'e str {
    e.local_name().into_inner()
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().into_inner() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Explicit1_0)
                .ok()
                .map(Cow::into_owned)
        })
}

fn entity(r: &BytesRef) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return c.to_string();
    }
    let name = r.xml10_content();
    resolve_predefined_entity(&name).unwrap_or("").to_string()
}

/// Text of the first element with this local name, such as a `dc:title`.
fn first_text(xml: &str, name: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_event().ok()? {
            Event::Eof => return None,
            Event::Start(e) if local(&e) == name => inside = true,
            Event::Text(t) if inside => text.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if inside => text.push_str(&entity(&r)),
            Event::End(e) if inside && e.local_name().into_inner() == name => {
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                return (!text.is_empty()).then_some(text);
            }
            _ => {}
        }
    }
}

fn first_attr(xml: &str, element: &str, name: &str) -> Result<Option<String>> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("META-INF/container.xml", e))?
        {
            Event::Eof => return Ok(None),
            Event::Start(e) | Event::Empty(e) if local(&e) == element => {
                return Ok(attr(&e, name));
            }
            _ => {}
        }
    }
}

/// Resolves an href against the folder of the file that contains it.
fn join(base: &str, href: &str) -> String {
    let mut path: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for piece in href.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            piece => path.push(piece),
        }
    }
    path.join("/")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn invalid(what: &str, why: &str) -> Error {
    Error::Invalid(format!("The file isn't a readable {what}: {why}."))
}

fn xml_error(part: &str, e: quick_xml::Error) -> Error {
    Error::Invalid(format!("Can't read {part}: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use zip::write::{SimpleFileOptions, ZipWriter};

    use super::*;

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let parts: Vec<(&str, &[u8])> = parts.iter().map(|(n, b)| (*n, b.as_bytes())).collect();
        package_bytes(&parts)
    }

    pub(crate) fn package_bytes(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// The first bytes of a PNG of this size, which is all `picture_kind` reads.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn reads_picture_sizes_from_their_headers() {
        assert_eq!(picture_kind(&png(400, 300)), Some(("png", 400, 300)));
        assert_eq!(
            picture_kind(b"GIF89a\x90\x01\x2c\x01"),
            Some(("gif", 400, 300))
        );
        let jpeg = [
            &[0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0][..],
            &[0xff, 0xc0, 0, 17, 8, 0x01, 0x2c, 0x01, 0x90, 3],
        ]
        .concat();
        assert_eq!(picture_kind(&jpeg), Some(("jpg", 400, 300)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend([0x8f, 0x01, 0, 0x2b, 0x01, 0]);
        assert_eq!(picture_kind(&webp), Some(("webp", 400, 300)));
        assert_eq!(picture_kind(b"<svg/>"), None);
    }

    #[test]
    fn marks_word_pictures_through_their_relationships() {
        let drawing = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Map of the Empire"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let document = format!(
            r#"<w:document {W} xmlns:wp="wp" xmlns:a="a" xmlns:pic="pic" xmlns:r="r"><w:body>{}{drawing}{}</w:body></w:document>"#,
            paragraph("", "Before"),
            paragraph("", "After")
        );
        let rels = r#"<Relationships><Relationship Id="rId5" Target="media/image1.png"/><Relationship Id="rId9" Target="https://example.org" TargetMode="External"/></Relationships>"#;
        let converted = docx(&package(&[
            ("word/document.xml", &document),
            ("word/_rels/document.xml.rels", rels),
        ]))
        .unwrap();
        assert_eq!(
            converted.images,
            vec![Image {
                path: "word/media/image1.png".into(),
                alt: "Map of the Empire".into()
            }]
        );
        assert_eq!(
            converted.markdown,
            format!("Before\n\n{MARK}0{MARK}\n\nAfter")
        );
    }

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

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

    fn paragraph(style: &str, text: &str) -> String {
        let style = if style.is_empty() {
            String::new()
        } else {
            format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#)
        };
        format!(r#"<w:p>{style}<w:r><w:t>{text}</w:t></w:r></w:p>"#)
    }

    #[test]
    fn reads_a_word_document() {
        let styles = format!(
            r#"<w:styles {W}>
            <w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style>
            <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
            <w:style w:type="paragraph" w:styleId="berschrift2"><w:name w:val="heading 2"/></w:style>
            <w:style w:type="paragraph" w:styleId="Chapter"><w:name w:val="Chapter"/><w:basedOn w:val="Heading1"/></w:style>
            <w:style w:type="paragraph" w:styleId="Loop"><w:name w:val="Loop"/><w:basedOn w:val="Loop"/></w:style>
            <w:style w:type="character" w:styleId="Heading1Char"><w:name w:val="heading 1 Char"/></w:style>
            </w:styles>"#
        );
        let body = [
            paragraph("Title", "The Peace of Westphalia"),
            paragraph("Heading1", "Background"),
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t xml:space="preserve">Signed in </w:t></w:r><w:r><w:t>Münster</w:t></w:r><w:r><w:tab/><w:t>&amp; Osnabrück</w:t></w:r><w:del><w:r><w:delText>in 1650</w:delText></w:r></w:del><w:moveFrom><w:r><w:t xml:space="preserve"> in 1648.</w:t></w:r></w:moveFrom><w:moveTo><w:r><w:t xml:space="preserve"> in 1648.</w:t></w:r></w:moveTo></w:p>"#.into(),
            paragraph("berschrift2", "Terms"),
            r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Sovereignty</w:t></w:r></w:p>"#.into(),
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>for princes</w:t></w:r></w:p>"#.into(),
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Treaty</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Place</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Osnabrück</w:t></w:r></w:p><w:p><w:r><w:t>(Sweden)</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>a|b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#.into(),
            r#"<w:p><w:r><mc:AlternateContent><mc:Choice><w:drawing><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>note</w:t></w:r></w:p>"#.into(),
            paragraph("Chapter", "Aftermath"),
            paragraph("Loop", "# not a heading"),
            r#"<w:p><w:r><w:t>Line one</w:t><w:br/><w:t>line two</w:t></w:r></w:p>"#.into(),
        ]
        .concat();
        let document = format!(
            r#"<?xml version="1.0"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
        );
        let core = r#"<cp:coreProperties xmlns:cp="c" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Westphalia  notes</dc:title></cp:coreProperties>"#;
        let bytes = package(&[
            ("word/document.xml", &document),
            ("word/styles.xml", &styles),
            ("docProps/core.xml", core),
        ]);
        let converted = docx(&bytes).unwrap();
        assert_eq!(converted.title.as_deref(), Some("Westphalia notes"));
        assert_eq!(
            converted.markdown,
            "# The Peace of Westphalia\n\n# Background\n\n\
             Signed in Münster & Osnabrück in 1648.\n\n## Terms\n\n- Sovereignty\n\n  - for princes\n\n\
             | Treaty | Place |\n| --- | --- |\n| Osnabrück (Sweden) | a\\|b |\n\n\
             Boxed note\n\n# Aftermath\n\n\\# not a heading\n\nLine one\nline two"
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

    #[test]
    fn refuses_what_is_not_a_package_and_stops_a_zip_bomb() {
        let err = docx(b"plain text").unwrap_err().to_string();
        assert!(err.contains("isn't a readable Word document"), "{err}");
        let err = epub(&package(&[("mimetype", "application/epub+zip")]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no META-INF/container.xml"), "{err}");

        let big = "x".repeat(4096);
        let bytes = package(&[("word/document.xml", &big)]);
        let mut small = Package::open(&bytes, "Word document", 4095).unwrap();
        assert!(small.part("word/document.xml").is_err());
        let mut enough = Package::open(&bytes, "Word document", 4096).unwrap();
        assert_eq!(
            enough.part("word/document.xml").unwrap().unwrap().len(),
            4096
        );
    }
}
