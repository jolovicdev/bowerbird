//! Text from documents stored as zip packages: Word (.docx) and EPUB. Both
//! become Markdown with headings, so the heading tree, chunking, and
//! citations work as they do for Markdown sources.

use std::borrow::Cow;
use std::io::{Cursor, Read};

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::doc::MARK;
use crate::{Error, Result};

mod docx;
mod epub;

pub use docx::docx;
pub use epub::epub;

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

    pub(crate) fn package(parts: &[(&str, &str)]) -> Vec<u8> {
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
