//! Adding sources to a notebook: files, archives of files, pasted text and
//! web pages. A source is stored under the hash of its bytes, so adding the
//! same one twice gives back the one already there.

use std::io::{Cursor, Read as _};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{
    AUDIO_EXTENSIONS, FIND_COLORS, Notebook, Source, SourceKind, letter_for, letter_index,
};
use crate::doc;
use crate::{Error, Result, pdf, web, zipdoc};

/// Bounds for unpacking a batch `.zip` source archive safely.
const MAX_ZIP_ENTRIES: usize = 200;
const MAX_ZIP_FILE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ZIP_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Summary of importing a `.zip` archive of source files.
#[derive(Debug, Default)]
pub struct ZipImport {
    pub added: Vec<Source>,
    /// Letters of sources already in the notebook.
    pub already: Vec<String>,
    /// Entries skipped because their format isn't supported or couldn't be read.
    pub skipped: Vec<String>,
}

impl Notebook {
    pub fn add_file(&mut self, path: &Path) -> Result<Source> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled");
        let bytes = std::fs::read(path)?;
        self.add_named(name, &bytes)
    }

    /// Adds a file's contents under its original name, which decides the kind
    /// and the fallback title.
    pub fn add_named(&mut self, file_name: &str, bytes: &[u8]) -> Result<Source> {
        let kind = check_file(file_name, bytes)?;
        let path = Path::new(file_name);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let packaged = |c: zipdoc::Converted| {
            c.title
                .or_else(|| first_h1(&c.markdown).map(|t| doc::strip_marks(&t)))
        };
        let title = match kind {
            SourceKind::Markdown => first_h1(&String::from_utf8_lossy(bytes)),
            SourceKind::Pdf => pdf::title(bytes),
            SourceKind::Docx => zipdoc::docx(bytes).ok().and_then(packaged),
            SourceKind::Epub => zipdoc::epub(bytes).ok().and_then(packaged),
            SourceKind::Text | SourceKind::Web | SourceKind::Youtube | SourceKind::Audio => None,
        }
        .unwrap_or_else(|| stem.to_string());
        let ext = match kind {
            SourceKind::Pdf => "pdf",
            SourceKind::Markdown => "md",
            SourceKind::Docx => "docx",
            SourceKind::Epub => "epub",
            SourceKind::Text | SourceKind::Web | SourceKind::Youtube => "txt",
            SourceKind::Audio => path.extension().and_then(|e| e.to_str()).unwrap_or("audio"),
        };
        let ext = ext.to_lowercase();
        let ext = ext.as_str();
        self.add_bytes(bytes, ext, kind, title, None)
    }

    /// Adds a file that came inside `folder`, as in a dropped folder or an
    /// archive. A title that fell back to the file's name gets the folder in
    /// front, so files of one name in different folders stay apart. `folder`
    /// is only ever text for the title, never a path that is opened.
    pub fn add_named_in(&mut self, folder: &str, file_name: &str, bytes: &[u8]) -> Result<Source> {
        let before = self.sources.len();
        let source = self.add_named(file_name, bytes)?;
        let folder = folder.replace('\\', "/");
        let folder = folder.trim_matches('/');
        let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str());
        if self.sources.len() == before || folder.is_empty() || stem != Some(&source.title) {
            return Ok(source);
        }
        self.rename_source(&source.id, &format!("{folder} / {}", source.title))
    }

    /// Unpacks a `.zip` archive and adds every supported file inside it,
    /// preserving subfolder prefixes in fallback titles and reporting duplicates
    /// or unsupported entries without failing the whole batch.
    pub fn add_zip(&mut self, bytes: &[u8]) -> Result<ZipImport> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| Error::Invalid("The file isn't a readable zip archive.".into()))?;
        if archive.len() > MAX_ZIP_ENTRIES {
            return Err(Error::Invalid(format!(
                "This zip archive holds more than {MAX_ZIP_ENTRIES} entries; split it into smaller archives."
            )));
        }
        let mut report = ZipImport::default();
        let mut total_bytes: u64 = 0;

        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| Error::Invalid(format!("Can't read zip entry: {e}")))?;
            if entry.is_dir() {
                continue;
            }
            let Some(enclosed) = entry.enclosed_name() else {
                continue;
            };
            // Skip macOS metadata and hidden files/folders.
            if enclosed.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == "__MACOSX" || s.starts_with('.')
            }) {
                continue;
            }
            let rel_path = enclosed.to_string_lossy().replace('\\', "/");
            let Some(file_name) = enclosed.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let remaining = MAX_ZIP_TOTAL_BYTES.saturating_sub(total_bytes);
            let limit = MAX_ZIP_FILE_BYTES.min(remaining);
            let mut data = Vec::new();
            entry.take(limit + 1).read_to_end(&mut data)?;
            if data.len() as u64 > limit {
                if total_bytes + data.len() as u64 > MAX_ZIP_TOTAL_BYTES {
                    return Err(Error::Invalid(format!(
                        "This zip archive unpacks to more than {} MB.",
                        MAX_ZIP_TOTAL_BYTES >> 20
                    )));
                }
                report.skipped.push(format!(
                    "{rel_path} (larger than {} MB)",
                    MAX_ZIP_FILE_BYTES >> 20
                ));
                continue;
            }
            total_bytes += data.len() as u64;

            if check_file(file_name, &data).is_err() {
                report.skipped.push(rel_path);
                continue;
            }
            let folder = enclosed
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let before = self.sources.len();
            let source = self.add_named_in(&folder, file_name, &data)?;
            if self.sources.len() == before {
                report.already.push(source.letter);
            } else {
                report.added.push(source);
            }
        }
        if report.added.is_empty() && report.already.is_empty() {
            return Err(Error::Invalid(
                "This zip archive has no files Bowerbird can read.".into(),
            ));
        }
        Ok(report)
    }

    pub fn add_text(&mut self, title: &str, text: &str) -> Result<Source> {
        self.add_bytes(
            text.as_bytes(),
            "md",
            SourceKind::Markdown,
            title.to_string(),
            None,
        )
    }

    /// Fetches and adds a link. For the command line, which only this machine
    /// runs, so the link may point anywhere.
    pub async fn add_url(&mut self, url: &str) -> Result<Source> {
        if let Some(existing) = self.source_with_url(url) {
            return Ok(existing.clone());
        }
        let page = web::fetch(url, web::Reach::Any).await?;
        self.add_web_page(url, &page)
    }

    /// Stores a page fetched with `web::fetch`. Split from `add_url` so callers
    /// can fetch without holding a lock on the notebook. A link to a PDF adds
    /// the PDF.
    pub fn add_web_page(&mut self, url: &str, page: &web::Page) -> Result<Source> {
        if let Some(existing) = self.source_with_url(url) {
            return Ok(existing.clone());
        }
        if let Some(video) = &page.transcript {
            return self.add_bytes(
                video.markdown.as_bytes(),
                "md",
                SourceKind::Youtube,
                video.title.clone(),
                Some(url.to_string()),
            );
        }
        if page.is_pdf() {
            pdf::check(&page.bytes)?;
            let title = pdf::title(&page.bytes).unwrap_or_else(|| web::file_name(url));
            return self.add_bytes(
                &page.bytes,
                "pdf",
                SourceKind::Pdf,
                title,
                Some(url.to_string()),
            );
        }
        let html = page.html()?;
        let (title, _) = web::extract(&html, Some(url))?;
        let title = if title.trim().is_empty() {
            url.to_string()
        } else {
            title
        };
        self.add_bytes(
            html.as_bytes(),
            "html",
            SourceKind::Web,
            title,
            Some(url.to_string()),
        )
    }

    fn add_bytes(
        &mut self,
        bytes: &[u8],
        ext: &str,
        kind: SourceKind,
        title: String,
        url: Option<String>,
    ) -> Result<Source> {
        let digest = Sha256::digest(bytes);
        let id: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        if let Some(existing) = self.source(&id) {
            return Ok(existing.clone());
        }
        let file = format!("{id}.{ext}");
        std::fs::write(self.dir.join("sources").join(&file), bytes)?;
        // Letters are never reused, so a citation in a saved answer can't
        // come to mean a different source after one is removed.
        let n = self
            .sources
            .iter()
            .filter_map(|s| letter_index(&s.letter))
            .max()
            .map_or(0, |last| last + 1);
        let source = Source {
            id,
            title,
            kind,
            file,
            letter: letter_for(n),
            color: (n % FIND_COLORS as usize) as u8,
            url,
            excluded: false,
        };
        self.sources.push(source.clone());
        self.save()?;
        Ok(source)
    }
}

/// The kind of source a file becomes, or why it can't be added. Checked
/// before anything is saved, so one bad file in an upload adds nothing.
pub fn check_file(file_name: &str, bytes: &[u8]) -> Result<SourceKind> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "pdf" => SourceKind::Pdf,
        "md" | "markdown" => SourceKind::Markdown,
        "txt" => SourceKind::Text,
        "docx" => SourceKind::Docx,
        "epub" => SourceKind::Epub,
        ext if AUDIO_EXTENSIONS.contains(&ext) => SourceKind::Audio,
        _ => {
            return Err(Error::Invalid(format!(
                "Can't add {file_name}: Bowerbird reads PDF, Word (.docx), EPUB, Markdown, text, audio/video recordings (MP3, M4A, WAV, Ogg, Opus, FLAC, WebM, MP4, MOV, MKV), and .zip archives."
            )));
        }
    };
    let invalid = |why: &str| Error::Invalid(format!("Can't add {file_name}. {why}"));
    match kind {
        SourceKind::Pdf => pdf::check(bytes).map_err(|e| invalid(&e.to_string()))?,
        // Whether a recording can be read shows when it is transcribed.
        SourceKind::Audio if bytes.is_empty() => return Err(invalid("The file is empty.")),
        SourceKind::Audio => {}
        SourceKind::Docx | SourceKind::Epub => {
            let converted = if kind == SourceKind::Docx {
                zipdoc::docx(bytes)
            } else {
                zipdoc::epub(bytes)
            }
            .map_err(|e| invalid(&e.to_string()))?;
            if converted.markdown.trim().is_empty() {
                return Err(invalid("The document has no text to read."));
            }
        }
        _ => match std::str::from_utf8(bytes) {
            Ok(text) if text.trim().is_empty() => return Err(invalid("The file is empty.")),
            Ok(_) => {}
            Err(_) => {
                return Err(invalid(
                    "The file isn't UTF-8 text. Save it as UTF-8 and add it again.",
                ));
            }
        },
    }
    Ok(kind)
}

fn first_h1(text: &str) -> Option<String> {
    doc::markdown(text.to_string())
        .sections
        .into_iter()
        .find(|s| s.depth == 1)
        .map(|s| s.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_non_utf8_text_files_are_refused() {
        assert!(check_file("notes.txt", b"  \n").is_err());
        assert!(check_file("notes.txt", &[0xff, 0xfe, 0x00]).is_err());
        assert_eq!(
            check_file("notes.MD", b"# Hi").unwrap(),
            SourceKind::Markdown
        );
    }

    #[test]
    fn word_files_keep_their_original_and_index_by_heading() {
        use std::io::Write;
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
        let document = format!(
            r#"<w:document {w}><w:body><w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>Causes</w:t></w:r></w:p><w:p><w:r><w:t>Prague, 1618.</w:t></w:r></w:p></w:body></w:document>"#
        );
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(document.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let dir = std::env::temp_dir().join(format!("bowerbird-docx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        let source = nb.add_named("war.DOCX", &bytes).unwrap();
        assert_eq!(
            (source.kind, source.title.as_str()),
            (SourceKind::Docx, "Causes")
        );
        assert!(source.file.ends_with(".docx"));
        assert_eq!(std::fs::read(nb.source_path(&source)).unwrap(), bytes);
        let doc = nb.extract(&source).unwrap();
        assert_eq!(doc.text, "# Causes\n\nPrague, 1618.");
        assert_eq!(doc.sections[0].title, "Causes");

        let empty = document.replace("Causes", "").replace("Prague, 1618.", "");
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(empty.as_bytes()).unwrap();
        let err = check_file("blank.docx", &zip.finish().unwrap().into_inner()).unwrap_err();
        assert!(err.to_string().contains("no text"), "{err}");
        assert!(check_file("book.epub", b"not a zip").is_err());
    }

    #[test]
    fn zip_archives_unpack_supported_sources_and_skip_junk() {
        use crate::zipdoc::tests::package_bytes;
        let archive = package_bytes(&[
            ("papers/notes.txt", b"Westphalia treaty notes from 1648."),
            ("chapter.md", b"# Causes\n\nPrague 1618."),
            ("__MACOSX/._chapter.md", b"junk"),
            (".hidden.txt", b"hidden"),
            ("unsupported.rtf", br"{\rtf1 ignored}"),
        ]);
        let dir = std::env::temp_dir().join(format!("bowerbird-zip-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        let report = nb.add_zip(&archive).unwrap();
        assert_eq!(report.added.len(), 2);
        assert_eq!(report.skipped, ["unsupported.rtf"]);
        assert!(report.already.is_empty());
        assert!(
            nb.sources.iter().any(|s| s.title == "papers / notes"),
            "{:?}",
            nb.sources
        );
        assert!(nb.sources.iter().any(|s| s.title == "Causes"));
        // Adding the same zip again marks both as already present.
        let second = nb.add_zip(&archive).unwrap();
        assert!(second.added.is_empty());
        assert_eq!(second.already.len(), 2);
        // A file from a dropped folder is titled the same way; one with a
        // title of its own keeps it.
        let dropped = nb
            .add_named_in("Papers/2024/", "draft.txt", b"A draft on Prague.")
            .unwrap();
        assert_eq!(dropped.title, "Papers/2024 / draft");
        let titled = nb
            .add_named_in("Papers", "siege.md", b"# The Siege\n\nMagdeburg 1631.")
            .unwrap();
        assert_eq!(titled.title, "The Siege");
        // Video containers are accepted as audio sources for local transcription.
        assert_eq!(
            check_file("lecture.mp4", b"ftyp").unwrap(),
            SourceKind::Audio
        );
        assert_eq!(check_file("talk.mkv", b"ebml").unwrap(), SourceKind::Audio);
    }
}
