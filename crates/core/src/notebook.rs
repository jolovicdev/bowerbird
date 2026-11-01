//! A notebook is a folder of plain files:
//!
//! ```text
//! <notebook>/
//!   notebook.toml            title, answer style, sources (order, letters)
//!   sources/                 originals as added, plus OCR text and transcripts
//!   notes/<id>.md            Markdown notes, titled by their first heading
//!   chats/<id>.jsonl         one conversation: questions, answers, citations
//!   studio/<id>.json         flashcards, quizzes and other study material
//!   guide.json               overview and suggested questions, per source set
//!   .bowerbird/index.sqlite  derived cache, rebuilt when missing
//! ```

use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::doc::{self, Doc};
use crate::{Error, Result, pdf, web, zipdoc};

/// Size of the finds palette that source colors index into.
pub const FIND_COLORS: u8 = 8;
pub const HOME_ENV: &str = "BOWERBIRD_HOME";
/// Longest notebook folder name in bytes, well under the usual 255-byte
/// file-name limit even after a "-2" suffix.
const SLUG_MAX: usize = 80;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Pdf,
    Markdown,
    Text,
    Web,
    Docx,
    Epub,
    Youtube,
    /// A sound recording, read through the transcript made of it.
    Audio,
}

/// Endings of the sound files Bowerbird transcribes.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "ogg", "oga", "opus", "flac", "webm",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    /// First 12 hex digits of the SHA-256 of the stored file.
    pub id: String,
    pub title: String,
    pub kind: SourceKind,
    /// File name inside `sources/`.
    pub file: String,
    pub letter: String,
    pub color: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Left out of answers and study material until switched back on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub excluded: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Goal {
    #[default]
    Default,
    /// Answers as a tutor would: step by step, checking understanding.
    Tutor,
    /// The user's own instructions in `ChatStyle::custom`.
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Length {
    Shorter,
    #[default]
    Default,
    Longer,
}

/// How answers in this notebook are written.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatStyle {
    #[serde(default)]
    pub goal: Goal,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom: String,
    #[serde(default)]
    pub length: Length,
}

/// Longest custom answer style, in characters.
pub const CUSTOM_STYLE_MAX: usize = 10_000;

#[derive(Serialize, Deserialize)]
struct NotebookFile {
    title: String,
    #[serde(default)]
    chat: ChatStyle,
    #[serde(default, rename = "source")]
    sources: Vec<Source>,
}

pub struct Notebook {
    pub dir: PathBuf,
    pub title: String,
    pub chat: ChatStyle,
    pub sources: Vec<Source>,
}

/// A file in one of the notebook's folders, by id, with its last change.
pub struct Item {
    pub id: String,
    /// Unix seconds.
    pub updated: u64,
}

/// Conversation the command line writes to, and the one conversation of
/// notebooks made before they could hold several.
pub const FIRST_CHAT: &str = "log";

/// `$BOWERBIRD_HOME`, else `~/Bowerbird`.
pub fn library_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV) {
        return dir.into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join("Bowerbird")
}

/// Folder name for a notebook title: lowercase letters and digits joined by
/// hyphens. Lowercasing can add combining marks ("İ" becomes "i" plus a dot),
/// so only the letters and digits of each lowercased character are kept.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase().filter(|l| l.is_alphanumeric()));
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.truncate(out.floor_char_boundary(SLUG_MAX));
    out.trim_end_matches('-').to_string()
}

/// 0 -> "A", 25 -> "Z", 26 -> "AA".
pub fn letter_for(mut n: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

/// "A" -> 0, "AA" -> 26: the inverse of `letter_for`.
fn letter_index(letter: &str) -> Option<usize> {
    if letter.is_empty() || !letter.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    Some(
        letter
            .bytes()
            .fold(0, |n, b| n * 26 + usize::from(b - b'A') + 1)
            - 1,
    )
}

fn no_source(id: &str) -> Error {
    not_found(format!("No source {id} in this notebook."))
}

fn not_found(message: String) -> Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, message).into()
}

/// Ids of notes, conversations, and study sets come from URLs, so they must
/// not be able to name a path outside their folder.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A single file name, so joining it to `sources/` can't leave that folder.
/// Backslashes and colons are refused too: on Windows they separate paths.
fn plain_file_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) && !name.contains(['/', '\\', ':'])
}

/// Fails on a symbolic link anywhere under `dir`, naming it relative to
/// `root`. Entries are listed without following links, so a link to a folder
/// is caught rather than entered.
fn refuse_links(root: &Path, dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            let path = entry.path();
            return Err(Error::Invalid(format!(
                "“{}” in this notebook is a link to somewhere else, so the notebook won't open.",
                path.strip_prefix(root).unwrap_or(&path).display()
            )));
        }
        if kind.is_dir() {
            refuse_links(root, &entry.path())?;
        }
    }
    Ok(())
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Splits a note file into its title (the first line's `# ` heading) and body.
fn split_note(raw: &str) -> (String, String) {
    match raw.strip_prefix("# ") {
        Some(rest) => {
            let (title, body) = rest.split_once('\n').unwrap_or((rest, ""));
            (
                title.trim().to_string(),
                body.trim_start_matches(['\n', '\r']).to_string(),
            )
        }
        None => (String::new(), raw.to_string()),
    }
}

impl Notebook {
    /// A notebook folder may come from someone else, so every path its
    /// notebook.toml names must stay inside the folder: source files are read
    /// and deleted through them. A symbolic link in the folder could point
    /// anywhere, and files are read and written through it, so a notebook
    /// holding one isn't opened, checked before notebook.toml is even read.
    pub fn open(dir: &Path) -> Result<Notebook> {
        refuse_links(dir, dir)?;
        let raw = std::fs::read_to_string(dir.join("notebook.toml"))?;
        let file: NotebookFile = toml::from_str(&raw).map_err(|e| Error::Toml(e.to_string()))?;
        if let Some(bad) = file
            .sources
            .iter()
            .find(|s| !valid_id(&s.id) || !plain_file_name(&s.file))
        {
            return Err(Error::Toml(format!(
                "source “{}” names a file outside the notebook",
                bad.title
            )));
        }
        Ok(Notebook {
            dir: dir.to_path_buf(),
            title: file.title,
            chat: file.chat,
            sources: file.sources,
        })
    }

    pub fn open_or_create(dir: &Path, title: &str) -> Result<Notebook> {
        if dir.join("notebook.toml").exists() {
            return Notebook::open(dir);
        }
        if dir.is_dir() {
            refuse_links(dir, dir)?;
        }
        for sub in ["sources", "notes", "chats", ".bowerbird"] {
            std::fs::create_dir_all(dir.join(sub))?;
        }
        let nb = Notebook {
            dir: dir.to_path_buf(),
            title: title.to_string(),
            chat: ChatStyle::default(),
            sources: Vec::new(),
        };
        nb.save()?;
        Ok(nb)
    }

    pub fn save(&self) -> Result<()> {
        let file = NotebookFile {
            title: self.title.clone(),
            chat: self.chat.clone(),
            sources: self.sources.clone(),
        };
        let raw = toml::to_string_pretty(&file).map_err(|e| Error::Toml(e.to_string()))?;
        let tmp = self.dir.join("notebook.toml.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(tmp, self.dir.join("notebook.toml"))?;
        Ok(())
    }

    pub fn index_path(&self) -> PathBuf {
        self.dir.join(".bowerbird").join("index.sqlite")
    }

    pub fn source_path(&self, source: &Source) -> PathBuf {
        self.dir.join("sources").join(&source.file)
    }

    /// Where a recording's transcript is kept, beside the recording.
    pub fn transcript_path(&self, source: &Source) -> PathBuf {
        self.dir
            .join("sources")
            .join(format!("{}.transcript.md", source.id))
    }

    /// Where pictures from a web page are kept, beside the page, under names
    /// from `image_file`.
    pub fn images_dir(&self, source: &Source) -> PathBuf {
        self.dir
            .join("sources")
            .join(format!("{}.images", source.id))
    }

    /// Saves a picture of a web page, fetched from `src`.
    pub fn save_image(&self, source: &Source, src: &str, ext: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.images_dir(source);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(image_file(src, ext)), bytes)?;
        Ok(())
    }

    /// Saves the pictures a Word document or EPUB carries beside it and keeps
    /// the ones big enough to show as figures, where their marks stood. The
    /// package stays the original; pictures are unpacked again if removed.
    fn unpack_figures(
        &self,
        source: &Source,
        package: &[u8],
        images: &[zipdoc::Image],
        doc: &mut Doc,
    ) -> Result<()> {
        let dir = self.images_dir(source);
        for (number, start) in doc.take_marks() {
            let Some(image) = images.get(number) else {
                continue;
            };
            let src = format!("package:{}", image.path);
            if doc.figures.iter().any(|f| f.src == src) {
                continue;
            }
            let Some(bytes) = zipdoc::read_image(package, &image.path) else {
                continue;
            };
            let Some((ext, width, height)) = zipdoc::picture_kind(&bytes) else {
                continue;
            };
            if width < doc::FIGURE_MIN_WIDTH || height < doc::FIGURE_MIN_HEIGHT {
                continue;
            }
            let file = image_file(&src, ext);
            if !dir.join(&file).exists() {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(&file), &bytes)?;
            }
            let caption = doc::caption(&image.alt, &doc.text[start..]);
            doc.figures.push(doc::Figure {
                start,
                src,
                file,
                caption,
                width,
                height,
            });
        }
        Ok(())
    }

    /// Keeps the figures whose picture was saved, naming their files. The
    /// index can then be rebuilt without fetching anything.
    fn keep_saved_figures(&self, source: &Source, doc: &mut Doc) {
        let dir = self.images_dir(source);
        doc.figures.retain_mut(|figure| {
            let saved = IMAGE_EXTENSIONS
                .iter()
                .map(|ext| image_file(&figure.src, ext))
                .find(|name| dir.join(name).exists());
            if let Some(name) = saved {
                figure.file = name;
            }
            !figure.file.is_empty()
        });
    }

    /// The text OCR read from a scanned PDF, pages separated by form feeds.
    pub fn ocr_path(&self, source: &Source) -> PathBuf {
        self.dir
            .join("sources")
            .join(format!("{}.ocr.txt", source.id))
    }

    /// Why a scanned PDF couldn't be read, kept so it isn't retried every
    /// time the notebook opens.
    pub fn ocr_failure_path(&self, source: &Source) -> PathBuf {
        self.dir
            .join("sources")
            .join(format!("{}.ocr-failed.txt", source.id))
    }

    /// Why a recording couldn't be transcribed, kept so it isn't retried
    /// every time the notebook opens.
    pub fn transcript_failure_path(&self, source: &Source) -> PathBuf {
        self.dir
            .join("sources")
            .join(format!("{}.transcript-failed.txt", source.id))
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn source_by_letter(&self, letter: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.letter == letter)
    }

    /// Sources that answers and study material draw on.
    pub fn included(&self) -> impl Iterator<Item = &Source> {
        self.sources.iter().filter(|s| !s.excluded)
    }

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

    pub fn source_with_url(&self, url: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.url.as_deref() == Some(url))
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

    pub fn rename_source(&mut self, id: &str, title: &str) -> Result<Source> {
        let title = title.trim();
        if title.is_empty() {
            return Err(Error::Invalid("Give the source a name.".into()));
        }
        let source = self
            .sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| no_source(id))?;
        source.title = title.to_string();
        let source = source.clone();
        self.save()?;
        Ok(source)
    }

    pub fn set_excluded(&mut self, id: &str, excluded: bool) -> Result<Source> {
        let source = self
            .sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| no_source(id))?;
        source.excluded = excluded;
        let source = source.clone();
        self.save()?;
        Ok(source)
    }

    /// Removes a source and its stored file. The index drops it on the next sync.
    pub fn remove_source(&mut self, id: &str) -> Result<()> {
        let at = self
            .sources
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| no_source(id))?;
        let source = self.sources.remove(at);
        self.save()?;
        match std::fs::remove_dir_all(self.images_dir(&source)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        for path in [
            self.source_path(&source),
            self.transcript_path(&source),
            self.transcript_failure_path(&source),
            self.ocr_path(&source),
            self.ocr_failure_path(&source),
        ] {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads the stored file and extracts its text and heading tree.
    pub fn extract(&self, source: &Source) -> Result<Doc> {
        let path = self.source_path(source);
        match source.kind {
            SourceKind::Pdf => match std::fs::read_to_string(self.ocr_path(source)) {
                Ok(text) => pdf::extract_scanned(&path, &text),
                Err(_) => pdf::extract(&path).map_err(|e| match e {
                    Error::Scanned => std::fs::read_to_string(self.ocr_failure_path(source))
                        .map_or(Error::Scanned, Error::Invalid),
                    other => other,
                }),
            },
            SourceKind::Markdown | SourceKind::Youtube => {
                Ok(doc::markdown(std::fs::read_to_string(path)?))
            }
            SourceKind::Text => Ok(Doc::plain(std::fs::read_to_string(path)?)),
            SourceKind::Web => {
                let (_, mut doc) =
                    web::extract(&std::fs::read_to_string(path)?, source.url.as_deref())?;
                self.keep_saved_figures(source, &mut doc);
                Ok(doc)
            }
            SourceKind::Docx | SourceKind::Epub => {
                let package = std::fs::read(path)?;
                let converted = if source.kind == SourceKind::Docx {
                    zipdoc::docx(&package)?
                } else {
                    zipdoc::epub(&package)?
                };
                let mut doc = doc::markdown(converted.markdown);
                self.unpack_figures(source, &package, &converted.images, &mut doc)?;
                Ok(doc)
            }
            SourceKind::Audio => match std::fs::read_to_string(self.transcript_path(source)) {
                Ok(transcript) => Ok(doc::markdown(transcript)),
                Err(_) => Err(Error::Invalid(
                    std::fs::read_to_string(self.transcript_failure_path(source))
                        .unwrap_or_else(|_| "Bowerbird is transcribing this recording.".into()),
                )),
            },
        }
    }

    /// The file of an item in one of the notebook's folders.
    pub fn item_path(&self, folder: &str, id: &str, ext: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(not_found(format!("Nothing in {folder} is called “{id}”.")));
        }
        Ok(self.dir.join(folder).join(format!("{id}.{ext}")))
    }

    /// Items in a folder with the given extension, most recently changed first.
    pub fn items(&self, folder: &str, ext: &str) -> Result<Vec<Item>> {
        let entries = match std::fs::read_dir(self.dir.join(folder)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut items = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some(ext) {
                continue;
            }
            let updated = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_or(0, unix_secs);
            items.push(Item {
                id: id.to_string(),
                updated,
            });
        }
        items.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        Ok(items)
    }

    /// An unused id for a new item: the time in milliseconds, so ids sort by age.
    pub fn new_item_id(&self, folder: &str, ext: &str) -> String {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        (ms..)
            .map(|n| n.to_string())
            .find(|id| !self.dir.join(folder).join(format!("{id}.{ext}")).exists())
            .expect("an unused id exists")
    }

    pub fn delete_item(&self, folder: &str, id: &str, ext: &str) -> Result<()> {
        std::fs::remove_file(self.item_path(folder, id, ext)?).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                not_found(format!("Nothing in {folder} is called “{id}”."))
            }
            _ => e.into(),
        })
    }

    pub fn append_chat(&self, chat: &str, record: &impl Serialize) -> Result<()> {
        let line = serde_json::to_string(record).map_err(|e| Error::Invalid(e.to_string()))?;
        let path = self.item_path("chats", chat, "jsonl")?;
        std::fs::create_dir_all(self.dir.join("chats"))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// A note's title and Markdown body.
    pub fn read_note(&self, id: &str) -> Result<(String, String)> {
        let raw =
            std::fs::read_to_string(self.item_path("notes", id, "md")?).map_err(|e| {
                match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        not_found(format!("No note “{id}” in this notebook."))
                    }
                    _ => e.into(),
                }
            })?;
        Ok(split_note(&raw))
    }

    pub fn write_note(&self, id: &str, title: &str, body: &str) -> Result<()> {
        let path = self.item_path("notes", id, "md")?;
        std::fs::create_dir_all(self.dir.join("notes"))?;
        let title = title.trim().replace(['\n', '\r'], " ");
        let title = if title.is_empty() {
            "Untitled note"
        } else {
            &title
        };
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, format!("# {title}\n\n{}\n", body.trim_end()))?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Picture formats a browser shows that are saved from web pages.
pub const IMAGE_EXTENSIONS: [&str; 4] = ["jpg", "png", "gif", "webp"];

/// File name of the picture fetched from `src`: the same address always
/// gives the same name.
pub fn image_file(src: &str, ext: &str) -> String {
    let digest = Sha256::digest(src.as_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.{ext}")
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
                "Can't add {file_name}: Bowerbird reads PDF, Word (.docx), EPUB, Markdown, text, and audio files (MP3, M4A, WAV, Ogg, Opus, FLAC, WebM)."
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
    fn letters_continue_past_z() {
        assert_eq!(letter_for(0), "A");
        assert_eq!(letter_for(25), "Z");
        assert_eq!(letter_for(26), "AA");
        assert_eq!(letter_for(27), "AB");
        assert_eq!(letter_for(52), "BA");
        for n in [0, 25, 26, 27, 52, 701, 702] {
            assert_eq!(letter_index(&letter_for(n)), Some(n));
        }
    }

    #[test]
    fn removed_sources_keep_their_letters_retired() {
        let dir = std::env::temp_dir().join(format!("bowerbird-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text("One", "Satin bowers.").unwrap();
        let b = nb.add_text("Two", "Vogelkop huts.").unwrap();
        nb.add_text("Three", "Great bowers.").unwrap();
        nb.remove_source(&b.id).unwrap();
        assert!(!nb.source_path(&b).exists());
        let letters: Vec<String> = Notebook::open(&dir)
            .unwrap()
            .sources
            .into_iter()
            .map(|s| s.letter)
            .collect();
        assert_eq!(letters, ["A", "C"]);
        assert_eq!(nb.add_text("Four", "Regent bowers.").unwrap().letter, "D");
    }

    #[test]
    fn notebooks_naming_files_outside_their_folder_do_not_open() {
        let dir = std::env::temp_dir().join(format!("bowerbird-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        let source = |id: &str, file: &str| {
            format!(
                "title = \"Birds\"\n\n[[source]]\nid = \"{id}\"\ntitle = \"T\"\nkind = \"text\"\nfile = '{file}'\nletter = \"A\"\ncolor = 0\n"
            )
        };
        for (id, file) in [
            ("a1b2c3d4e5f6", "../../escape.txt"),
            ("a1b2c3d4e5f6", "/etc/hostname"),
            ("a1b2c3d4e5f6", r"..\escape.txt"),
            ("../../escape", "a1b2c3d4e5f6.txt"),
        ] {
            std::fs::write(dir.join("notebook.toml"), source(id, file)).unwrap();
            assert!(Notebook::open(&nb.dir).is_err(), "{id} {file}");
        }
        std::fs::write(
            dir.join("notebook.toml"),
            source("a1b2c3d4e5f6", "a1b2c3d4e5f6.txt"),
        )
        .unwrap();
        assert_eq!(Notebook::open(&nb.dir).unwrap().sources.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn notebooks_holding_links_do_not_open() {
        use std::os::unix::fs::symlink;
        let tmp = std::env::temp_dir().join(format!("bowerbird-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let elsewhere = tmp.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let secret = elsewhere.join("secret.txt");
        std::fs::write(&secret, "Not part of any notebook.").unwrap();
        let fresh = |name: &str| {
            let mut nb = Notebook::open_or_create(&tmp.join(name), "Birds").unwrap();
            let source = nb
                .add_text("Bowers", "The male gathers blue finds.")
                .unwrap();
            (nb, source)
        };
        let refusal = |nb: &Notebook| Notebook::open(&nb.dir).err().map(|e| e.to_string());

        let (nb, source) = fresh("source");
        std::fs::remove_file(nb.source_path(&source)).unwrap();
        symlink(&secret, nb.source_path(&source)).unwrap();
        assert!(refusal(&nb).is_some_and(|e| e.contains(&source.file)));

        let (nb, _) = fresh("note");
        symlink(&secret, nb.dir.join("notes").join("n1.md.tmp")).unwrap();
        assert!(refusal(&nb).is_some_and(|e| e.contains("n1.md.tmp")));

        let (nb, _) = fresh("folder");
        std::fs::remove_dir_all(nb.dir.join("chats")).unwrap();
        symlink(&elsewhere, nb.dir.join("chats")).unwrap();
        assert!(refusal(&nb).is_some_and(|e| e.contains("chats")));

        let unfinished = tmp.join("unfinished");
        std::fs::create_dir_all(&unfinished).unwrap();
        symlink(&secret, unfinished.join("notebook.toml.tmp")).unwrap();
        assert!(Notebook::open_or_create(&unfinished, "Birds").is_err());
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "Not part of any notebook."
        );
    }

    #[test]
    fn notes_keep_title_and_body_and_ids_stay_in_their_folder() {
        let dir = std::env::temp_dir().join(format!("bowerbird-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nb = Notebook::open_or_create(&dir, "History").unwrap();
        let id = nb.new_item_id("notes", "md");
        let body = "The war ended in 1648 [[A0|signed in 1648]].\n\n# Not a title";
        nb.write_note(&id, "Causes\nof the war", body).unwrap();
        assert_eq!(
            nb.read_note(&id).unwrap(),
            ("Causes of the war".to_string(), format!("{body}\n"))
        );
        let listed: Vec<String> = nb
            .items("notes", "md")
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(listed, std::slice::from_ref(&id));
        assert!(nb.item_path("notes", "../notebook", "md").is_err());
        assert!(nb.read_note("missing").is_err());
        nb.delete_item("notes", &id, "md").unwrap();
        assert!(nb.items("notes", "md").unwrap().is_empty());
    }

    #[test]
    fn switched_off_sources_stay_off_after_reopening() {
        let dir = std::env::temp_dir().join(format!("bowerbird-excluded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        let a = nb.add_text("One", "Westphalia.").unwrap();
        nb.add_text("Two", "Utrecht.").unwrap();
        nb.set_excluded(&a.id, true).unwrap();
        let reopened = Notebook::open(&dir).unwrap();
        let included: Vec<&str> = reopened.included().map(|s| s.title.as_str()).collect();
        assert_eq!(included, ["Two"]);
    }

    #[test]
    fn slug_keeps_words_and_drops_symbols() {
        assert_eq!(slug("  Bird Notes: 2026! "), "bird-notes-2026");
    }

    #[test]
    fn slug_stays_a_valid_short_folder_name() {
        assert_eq!(slug("QA İstanbul"), "qa-istanbul");
        let long = slug(&"日本語 ".repeat(40));
        assert!(long.len() <= SLUG_MAX && !long.ends_with('-'));
        assert!(long.chars().all(|c| c.is_alphanumeric() || c == '-'));
    }

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
    fn unpacks_book_pictures_big_enough_to_show() {
        use crate::zipdoc::tests::{package_bytes, png};
        let chapter = r#"<html><body><h1>The war</h1><p><img src="map.png" alt=""/></p><p>The empire in 1648</p><p><img src="icon.png"/>Prague, 1618.</p></body></html>"#;
        let (map, icon) = (png(640, 480), png(20, 20));
        let book = package_bytes(&[
            ("META-INF/container.xml", br#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#),
            ("book.opf", br#"<package><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#),
            ("c.xhtml", chapter.as_bytes()),
            ("map.png", &map),
            ("icon.png", &icon),
        ]);
        let dir = std::env::temp_dir().join(format!("bowerbird-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        let source = nb.add_named("war.epub", &book).unwrap();
        let doc = nb.extract(&source).unwrap();
        assert_eq!(doc.text, "# The war\n\nThe empire in 1648\n\nPrague, 1618.");
        let [figure] = doc.figures.as_slice() else {
            panic!("only the map is big enough: {:?}", doc.figures);
        };
        assert_eq!(
            (figure.width, figure.height, figure.caption.as_str()),
            (640, 480, "The empire in 1648")
        );
        assert_eq!(&doc.text[figure.start..][..10], "The empire");
        assert_eq!(
            std::fs::read(nb.images_dir(&source).join(&figure.file)).unwrap(),
            map
        );
        nb.remove_source(&source.id).unwrap();
        assert!(!nb.images_dir(&source).exists());
    }
}
