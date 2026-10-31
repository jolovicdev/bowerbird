//! PDF text extraction through PDFium. The heading tree comes from the PDF's
//! bookmarks; without bookmarks, headings are inferred from font sizes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use crate::doc::{Doc, Section};
use crate::{Error, Result};

/// Directory holding the PDFium shared library. Overrides the lookup next to
/// the executable.
pub const PDFIUM_DIR_ENV: &str = "BOWERBIRD_PDFIUM_DIR";

const MAX_DEPTH: u8 = 6;
/// Text this much larger than the body size counts as a heading candidate.
const HEADING_SCALE: f32 = 1.2;
const HEADING_LEVELS: usize = 3;
/// Small-caps lowercase letters are about 70 to 80% of the capitals' size.
const SMALL_CAPS_SCALE: f32 = 0.7;

static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();
static BUNDLED_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Folder where an app bundle ships PDFium, checked after the environment
/// variable. Takes effect only if set before the first PDF is read.
pub fn use_bundled_dir(dir: PathBuf) {
    let _ = BUNDLED_DIR.set(dir);
}

fn pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(bind)
        .as_ref()
        .map_err(|e| Error::Pdf(e.clone()))
}

fn bind() -> std::result::Result<Pdfium, String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os(PDFIUM_DIR_ENV) {
        dirs.push(dir.into());
    }
    dirs.extend(BUNDLED_DIR.get().cloned());
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(dir);
    }
    #[cfg(debug_assertions)]
    dirs.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/pdfium/lib"
    )));

    for dir in &dirs {
        let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
        if lib.exists() {
            return Pdfium::bind_to_library(&lib)
                .map(Pdfium::new)
                .map_err(|e| format!("PDFium at {} failed to load: {e}", lib.display()));
        }
    }
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .map_err(|_| {
            format!(
                "PDFium library not found. Run scripts/fetch-pdfium.sh or set {PDFIUM_DIR_ENV}."
            )
        })
}

/// The document's title: its Title metadata when that looks like a real title,
/// else the largest text on the first page.
pub fn title(bytes: &[u8]) -> Option<String> {
    let pdfium = pdfium().ok()?;
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).ok()?;
    if let Some(tag) = document.metadata().get(PdfDocumentMetadataTagType::Title) {
        let title = squash_spaces(tag.value());
        if plausible_title(&title) {
            return Some(title);
        }
    }
    let page = document.pages().first().ok()?;
    // (font size, bounds, has letters or digits) per text object, in content order.
    let objects: Vec<(f32, PdfRect, bool)> = page
        .objects()
        .iter()
        .filter_map(|object| {
            let text_object = object.as_text_object()?;
            let text = text_object.text();
            let rect = object.bounds().ok()?.to_rect();
            // Skip arXiv's sidebar stamp: rotated text taller than it is wide.
            let sideways =
                text.trim().chars().count() >= 3 && rect.height().value > rect.width().value;
            let stamp = text.trim_start().to_lowercase().starts_with("arxiv:");
            let has_words = text.chars().any(char::is_alphanumeric);
            (!sideways && !stamp).then(|| (text_object.scaled_font_size().value, rect, has_words))
        })
        .collect();
    let largest = objects
        .iter()
        .filter(|(_, _, words)| *words)
        .map(|(size, _, _)| *size)
        .fold(0.0, f32::max);
    let first = objects
        .iter()
        .position(|(size, _, words)| *words && *size >= largest - 0.1)?;
    // The title runs on while objects are title-sized, or smaller but on the
    // current title line or the line right under it: small caps set lowercase
    // letters smaller than the capitals. Author lines sit a clear gap lower.
    let mut bounds = objects[first].1;
    let mut line = (bounds.bottom().value, bounds.top().value);
    for (size, rect, has_words) in &objects[first + 1..] {
        let title_sized = *size >= largest - 0.1;
        let center = (rect.bottom().value + rect.top().value) / 2.0;
        let near_size = *size >= largest * SMALL_CAPS_SCALE;
        let same_line = (line.0..=line.1).contains(&center) && near_size;
        let gap = line.0 - rect.top().value;
        let next_line = center < line.0 && gap < line.1 - line.0 && near_size;
        if *has_words {
            if !title_sized && !same_line && !next_line {
                break;
            }
            if title_sized || next_line {
                line = (rect.bottom().value, rect.top().value);
            }
        } else if !same_line {
            // Symbols (hyphens, footnote marks) widen the box only on a title line;
            // their tiny boxes would distort the line band.
            continue;
        }
        bounds = PdfRect::new_from_values(
            bounds.bottom().value.min(rect.bottom().value),
            bounds.left().value.min(rect.left().value),
            bounds.top().value.max(rect.top().value),
            bounds.right().value.max(rect.right().value),
        );
    }
    let title = squash_spaces(&join_soft_hyphens(&page.text().ok()?.inside_rect(bounds)));
    plausible_title(&title).then_some(title)
}

/// Joins words PDFium split at a line-break hyphen, which it reports as U+0002.
fn join_soft_hyphens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{2}' {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
        } else {
            out.push(c);
        }
    }
    out
}

fn squash_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rejects empty and machine-made titles: file names, Word's "Microsoft Word - x.docx",
/// and arXiv's sidebar stamp, which is often the largest text on page one.
fn plausible_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    title.chars().filter(|c| c.is_alphabetic()).count() >= 3
        && title.chars().count() <= 200
        && lower != "untitled"
        && !lower.starts_with("arxiv:")
        && !lower.starts_with("microsoft word")
        && ![".pdf", ".doc", ".docx", ".tex", ".dvi"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// Fails with a readable reason when the bytes aren't a PDF Bowerbird can open.
pub fn check(bytes: &[u8]) -> Result<()> {
    pdfium()?
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(load_error)?;
    Ok(())
}

fn load_error(error: PdfiumError) -> Error {
    use pdfium_render::prelude::PdfiumInternalError as Internal;
    Error::Pdf(match error {
        PdfiumError::PdfiumLibraryInternalError(Internal::FormatError) => {
            "The file isn't a readable PDF. It may be damaged or another kind of file.".into()
        }
        PdfiumError::PdfiumLibraryInternalError(
            Internal::PasswordError | Internal::SecurityError,
        ) => "The PDF is password-protected. Save an unlocked copy and add that.".into(),
        other => format!("Couldn't open the PDF ({other:?})."),
    })
}

pub fn extract(path: &Path) -> Result<Doc> {
    let pdfium = pdfium()?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(load_error)?;
    let mut page_texts = Vec::new();
    for page in document.pages().iter() {
        let page_text = page
            .text()
            .map_err(|e| {
                Error::Pdf(format!(
                    "Couldn't read the text on page {} ({e:?}).",
                    page_texts.len() + 1
                ))
            })?
            .all()
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        page_texts.push(page_text);
    }
    if page_texts.iter().all(|t| t.trim().is_empty()) {
        return Err(Error::Scanned);
    }
    Ok(assemble(&document, page_texts))
}

/// A scanned PDF with its text recognized by OCR: one page per form feed,
/// in page order. Bookmarks still give the heading tree.
pub fn extract_scanned(path: &Path, ocr_text: &str) -> Result<Doc> {
    let pdfium = pdfium()?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(load_error)?;
    let mut page_texts: Vec<String> = ocr_text.split('\u{c}').map(str::to_string).collect();
    page_texts.resize(document.pages().len() as usize, String::new());
    Ok(assemble(&document, page_texts))
}

/// Width pages are rendered at for OCR, about 200 dpi on A4 or letter paper.
const OCR_WIDTH: i32 = 1700;

/// Renders each page for OCR, calling `each` with the page's index, the page
/// count, and the page as RGB bytes with its width and height.
pub fn render_pages(
    path: &Path,
    mut each: impl FnMut(usize, usize, Vec<u8>, u32, u32) -> Result<()>,
) -> Result<()> {
    let pdfium = pdfium()?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(load_error)?;
    let total = document.pages().len() as usize;
    let config = PdfRenderConfig::new().set_target_width(OCR_WIDTH);
    for (i, page) in document.pages().iter().enumerate() {
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| Error::Pdf(format!("Couldn't render page {} ({e:?}).", i + 1)))?;
        let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
        let rgb: Vec<u8> = bitmap
            .as_rgba_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, _]| [r, g, b])
            .collect();
        each(i, total, rgb, width, height)?;
    }
    Ok(())
}

fn assemble(document: &PdfDocument, page_texts: Vec<String>) -> Doc {
    let mut text = String::new();
    let mut page_starts = Vec::new();
    for page_text in &page_texts {
        page_starts.push(text.len());
        text.push_str(page_text);
        text.push_str("\n\n");
    }
    let mut sections = bookmark_sections(document, &page_starts, &text);
    if sections.is_empty() {
        sections = font_size_sections(document, &page_starts, &page_texts);
    }
    sections.sort_by_key(|s| s.start);
    Doc {
        text,
        page_starts,
        sections,
        figures: Vec::new(),
    }
}

fn bookmark_sections(document: &PdfDocument, page_starts: &[usize], text: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut next = document.bookmarks().root();
    while let Some(bookmark) = next {
        walk_bookmark(&bookmark, 1, page_starts, text, &mut out);
        next = bookmark.next_sibling();
    }
    out
}

fn walk_bookmark(
    bookmark: &PdfBookmark,
    depth: u8,
    page_starts: &[usize],
    text: &str,
    out: &mut Vec<Section>,
) {
    let title = bookmark.title().unwrap_or_default().trim().to_string();
    let page = bookmark
        .destination()
        .and_then(|d| d.page_index().ok())
        .or_else(|| {
            let action = bookmark.action()?;
            let local = action.as_local_destination_action()?;
            local.destination().ok()?.page_index().ok()
        });
    if let (false, Some(page)) = (title.is_empty(), page)
        && let Some(&page_start) = page_starts.get(page as usize)
    {
        let page_end = page_starts
            .get(page as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let page_text = &text[page_start..page_end];
        let at = find_line(page_text, 0, &title)
            .or_else(|| find_in(page_text, &title))
            .unwrap_or(0);
        out.push(Section {
            title,
            depth,
            start: page_start + at,
        });
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let mut child = bookmark.first_child();
    while let Some(c) = child {
        walk_bookmark(&c, depth + 1, page_starts, text, out);
        child = c.next_sibling();
    }
}

/// Offset of the first line at or after `from` that is exactly the heading,
/// optionally led by a section number such as "3.2" or "A.".
fn find_line(text: &str, from: usize, title: &str) -> Option<usize> {
    let want = squash(title);
    let mut offset = from;
    for line in text[from..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let line = squash(line);
        if line == want || strip_number(&line) == want {
            return Some(start);
        }
    }
    None
}

fn squash(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// "3.2 attention" -> "attention", "a. details" -> "details".
fn strip_number(line: &str) -> &str {
    let Some((head, rest)) = line.split_once(' ') else {
        return line;
    };
    let numeric = head.chars().all(|c| c.is_ascii_digit() || c == '.');
    let lettered = head.trim_end_matches('.').chars().count() == 1
        && head.chars().all(|c| c.is_alphabetic() || c == '.');
    if numeric || lettered { rest } else { line }
}

/// Offset of `needle` anywhere in `text`, ignoring case and runs of
/// whitespace. For bookmark titles that wrap onto several lines.
fn find_in(text: &str, needle: &str) -> Option<usize> {
    let words: Vec<String> = needle.split_whitespace().map(str::to_lowercase).collect();
    let first = words.first()?;
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; only trust offsets when it didn't.
    if lower.len() != text.len() {
        return None;
    }
    let mut search = 0;
    while let Some(pos) = lower[search..].find(first.as_str()) {
        let at = search + pos;
        let mut rest = lower[at..].split_whitespace();
        if words
            .iter()
            .all(|w| rest.next().is_some_and(|r| r.starts_with(w.as_str())))
        {
            return Some(at);
        }
        search = at + first.len();
    }
    None
}

struct Candidate {
    page: usize,
    size: f32,
    title: String,
}

fn font_size_sections(
    document: &PdfDocument,
    page_starts: &[usize],
    page_texts: &[String],
) -> Vec<Section> {
    let mut chars_by_size: HashMap<i32, usize> = HashMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for (page_idx, page) in document.pages().iter().enumerate() {
        let mut run: Option<Candidate> = None;
        for object in page.objects().iter() {
            let Some(text_object) = object.as_text_object() else {
                continue;
            };
            let content = text_object.text();
            let size = text_object.scaled_font_size().value;
            *chars_by_size.entry(size_key(size)).or_default() += content.chars().count();
            match &mut run {
                Some(c) if (c.size - size).abs() < 0.1 => {
                    c.title.push(' ');
                    c.title.push_str(content.trim());
                }
                _ => {
                    candidates.extend(run.take());
                    run = Some(Candidate {
                        page: page_idx,
                        size,
                        title: content.trim().to_string(),
                    });
                }
            }
        }
        candidates.extend(run);
    }

    let Some((&body_key, _)) = chars_by_size.iter().max_by_key(|(k, n)| (**n, -**k)) else {
        return Vec::new();
    };
    let body = body_key as f32 / 2.0;
    candidates.retain(|c| {
        let title = c.title.split_whitespace().collect::<Vec<_>>().join(" ");
        c.size >= body * HEADING_SCALE
            && (3..=100).contains(&title.chars().count())
            && title.chars().filter(|ch| ch.is_alphabetic()).count() >= 3
    });

    let mut levels: Vec<i32> = candidates.iter().map(|c| size_key(c.size)).collect();
    levels.sort_unstable_by(|a, b| b.cmp(a));
    levels.dedup();
    levels.truncate(HEADING_LEVELS);

    let mut sections = Vec::new();
    // Headings on a page are matched in content order, each after the previous one.
    let mut cursor = (usize::MAX, 0);
    for c in candidates {
        let Some(depth) = levels.iter().position(|&k| k == size_key(c.size)) else {
            continue;
        };
        let title = c.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let page_text = &page_texts[c.page];
        let from = if cursor.0 == c.page { cursor.1 } else { 0 };
        let Some(at) =
            find_line(page_text, from, &title).or_else(|| find_line(page_text, 0, &title))
        else {
            continue;
        };
        cursor = (c.page, at + 1);
        sections.push(Section {
            title,
            depth: depth as u8 + 1,
            start: page_starts[c.page] + at,
        });
    }
    sections
}

/// Font sizes bucketed to half points.
fn size_key(size: f32) -> i32 {
    (size * 2.0).round() as i32
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Writes minimal valid PDFs (Helvetica text in WinAnsi encoding, so
    //! `\374` is ü; optional outline) for tests.

    pub struct Page<'a> {
        /// (font size, y position, text)
        pub lines: Vec<(f32, f32, &'a str)>,
    }

    pub fn build(pages: &[Page], outline: &[(&str, usize)]) -> Vec<u8> {
        let mut objects: Vec<String> = Vec::new();
        let page_count = pages.len();
        // 1 catalog, 2 pages, 3 font, then per page: page + content, then outline objects.
        let first_page_obj = 4;
        let outline_root = first_page_obj + page_count * 2;
        let catalog = if outline.is_empty() {
            "<< /Type /Catalog /Pages 2 0 R >>".to_string()
        } else {
            format!("<< /Type /Catalog /Pages 2 0 R /Outlines {outline_root} 0 R >>")
        };
        objects.push(catalog);
        let kids: Vec<String> = (0..page_count)
            .map(|i| format!("{} 0 R", first_page_obj + i * 2))
            .collect();
        objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
            kids.join(" ")
        ));
        objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_string(),
        );
        for (i, page) in pages.iter().enumerate() {
            let content_obj = first_page_obj + i * 2 + 1;
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_obj} 0 R >>"
            ));
            let mut stream = String::new();
            for (size, y, text) in &page.lines {
                stream.push_str(&format!("BT /F1 {size} Tf 72 {y} Td ({text}) Tj ET\n"));
            }
            objects.push(format!(
                "<< /Length {} >>\nstream\n{stream}endstream",
                stream.len()
            ));
        }
        if !outline.is_empty() {
            let first_item = outline_root + 1;
            let last_item = outline_root + outline.len();
            objects.push(format!(
                "<< /Type /Outlines /First {first_item} 0 R /Last {last_item} 0 R /Count {} >>",
                outline.len()
            ));
            for (i, (title, page)) in outline.iter().enumerate() {
                let obj = first_item + i;
                let mut item = format!(
                    "<< /Title ({title}) /Parent {outline_root} 0 R /Dest [{} 0 R /XYZ 0 792 0]",
                    first_page_obj + page * 2
                );
                if i > 0 {
                    item.push_str(&format!(" /Prev {} 0 R", obj - 1));
                }
                if i + 1 < outline.len() {
                    item.push_str(&format!(" /Next {} 0 R", obj + 1));
                }
                item.push_str(" >>");
                objects.push(item);
            }
        }

        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_at = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A PDF of page images with no text layer, like a scanned book: each
    /// page is (RGB bytes, width, height), drawn over a letter-size page.
    pub fn scanned(pages: &[(Vec<u8>, u32, u32)]) -> Vec<u8> {
        // 1 catalog, 2 pages, then per page: page, content, image.
        let mut objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..pages.len())
                    .map(|i| format!("{} 0 R", 3 + i * 3))
                    .collect::<Vec<_>>()
                    .join(" "),
                pages.len()
            )
            .into_bytes(),
        ];
        for (i, (rgb, width, height)) in pages.iter().enumerate() {
            let (content, image) = (4 + i * 3, 5 + i * 3);
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im0 {image} 0 R >> >> /Contents {content} 0 R >>"
            ).into_bytes());
            let draw = "q 612 0 0 792 0 0 cm /Im0 Do Q";
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{draw}\nendstream",
                    draw.len() + 1
                )
                .into_bytes(),
            );
            let mut xobject = format!(
                "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
                rgb.len()
            )
            .into_bytes();
            xobject.extend_from_slice(rgb);
            xobject.extend_from_slice(b"\nendstream");
            objects.push(xobject);
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{Page, build, scanned};
    use super::*;

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bowerbird-pdf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn sections_come_from_bookmarks_with_pages() {
        let pdf = build(
            &[
                Page {
                    lines: vec![
                        (11.0, 700.0, "Introduction"),
                        (11.0, 680.0, "Bowerbirds collect blue objects."),
                    ],
                },
                Page {
                    lines: vec![
                        (11.0, 700.0, "Methods"),
                        (11.0, 680.0, "We counted bottle caps."),
                    ],
                },
            ],
            &[("Introduction", 0), ("Methods", 1)],
        );
        let doc = extract(&write_temp("outline.pdf", &pdf)).unwrap();
        let titles: Vec<_> = doc.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Introduction", "Methods"]);
        let methods = &doc.sections[1];
        assert_eq!(doc.page_at(methods.start), Some(2));
        assert!(doc.text[methods.start..].starts_with("Methods"));
    }

    #[test]
    fn sections_fall_back_to_large_fonts_without_bookmarks() {
        let body = "Bowerbirds sort their finds into piles by color on a mat of moss.";
        let pdf = build(
            &[
                Page {
                    lines: vec![
                        (20.0, 700.0, "Sorting behavior"),
                        (10.0, 670.0, body),
                        (10.0, 655.0, body),
                    ],
                },
                Page {
                    lines: vec![
                        (
                            10.0,
                            715.0,
                            "Keep field notes daily, with the date and weather.",
                        ),
                        (10.0, 700.0, body),
                        (20.0, 670.0, "Field notes"),
                        (10.0, 640.0, body),
                    ],
                },
            ],
            &[],
        );
        let doc = extract(&write_temp("fonts.pdf", &pdf)).unwrap();
        let titles: Vec<_> = doc
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s.depth))
            .collect();
        assert_eq!(titles, vec![("Sorting behavior", 1), ("Field notes", 1)]);
        let field_notes = &doc.sections[1];
        assert_eq!(doc.page_at(field_notes.start), Some(2));
        assert!(
            doc.text[field_notes.start..].starts_with("Field notes"),
            "heading must start its own line"
        );
    }

    #[test]
    fn title_is_the_largest_text_on_page_one_minus_the_arxiv_stamp() {
        let pdf = build(
            &[Page {
                lines: vec![
                    (24.0, 760.0, "arXiv:2101.00001v1 [cs.CL] 1 Jan 2021"),
                    (17.0, 700.0, "Bowerbird Courtship"),
                    (17.0, 680.0, "and Color Sorting"),
                    (14.0, 640.0, "Jane Doe and John Roe"),
                    (10.0, 600.0, "Abstract. Males sort their finds by color."),
                ],
            }],
            &[],
        );
        assert_eq!(
            title(&pdf).as_deref(),
            Some("Bowerbird Courtship and Color Sorting")
        );
    }

    #[test]
    fn heading_lines_may_carry_section_numbers() {
        assert_eq!(strip_number("3.2 attention"), "attention");
        assert_eq!(strip_number("a. details"), "details");
        assert_eq!(
            strip_number("attention is all you need"),
            "attention is all you need"
        );
        assert_eq!(
            find_line("body text\n3.2 Attention\nmore", 0, "Attention"),
            Some(10)
        );
    }

    #[test]
    fn a_pdf_without_text_is_scanned_and_takes_ocr_text_by_page() {
        let white = (vec![255u8; 30 * 40 * 3], 30, 40);
        let path = write_temp("scanned.pdf", &scanned(&[white.clone(), white]));
        assert!(matches!(extract(&path), Err(Error::Scanned)));

        let doc = extract_scanned(&path, "Treaty of Westphalia\u{c}Signed at Osnabrück").unwrap();
        assert_eq!(doc.page_starts.len(), 2);
        let second = &doc.text[doc.page_starts[1]..];
        assert!(second.starts_with("Signed at Osnabrück"), "{second:?}");

        let mut sizes = Vec::new();
        render_pages(&path, |i, total, rgb, width, height| {
            assert_eq!(rgb.len(), (width * height * 3) as usize);
            sizes.push((i, total, width));
            Ok(())
        })
        .unwrap();
        assert_eq!(sizes, [(0, 2, OCR_WIDTH as u32), (1, 2, OCR_WIDTH as u32)]);
    }
}
