//! Exporting conversations and notebooks as portable plain files:
//! Markdown transcripts with verified citation footnotes, BibTeX source
//! bibliographies, and whole-notebook ZIP archives.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::Path;

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::ask::Answer;
use crate::cite::{self, Piece};
use crate::notebook::{self, Notebook, Source, SourceKind};
use crate::{Error, Result};

/// A conversation exported as GitHub-flavored Markdown with numbered citation
/// footnotes and a source bibliography at the bottom.
pub fn chat_markdown(nb: &Notebook, turns: &[Answer]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", nb.title));
    if !turns.is_empty() {
        out.push_str(&format!(
            "\n*Exported conversation ({} {})*\n",
            turns.len(),
            if turns.len() == 1 { "turn" } else { "turns" }
        ));
    }

    // Footnotes are numbered globally across the document, deduplicating
    // identical (label, quote) citations so repeat cites share one footnote.
    let mut footnotes: Vec<(String, String)> = Vec::new();
    let mut seen: HashMap<(String, String), usize> = HashMap::new();

    for (i, turn) in turns.iter().enumerate() {
        out.push_str(&format!("\n## {}. {}\n\n", i + 1, one_line(&turn.question)));

        let mut parser = cite::Parser::default();
        let mut pieces = parser.feed(&turn.text);
        pieces.extend(parser.finish());
        let mut cites = turn.citations.iter();
        let mut body = String::new();

        for piece in pieces {
            match piece {
                Piece::Text(t) => body.push_str(&t),
                Piece::Cite { label, quote } => {
                    let resolved = cites.next();
                    let effective_label = resolved
                        .map(|c| c.label.clone())
                        .unwrap_or_else(|| label.clone());
                    let key = (effective_label.clone(), quote.clone());
                    let number = *seen.entry(key).or_insert_with(|| {
                        let n = footnotes.len() + 1;
                        let source_desc = resolved
                            .and_then(|c| c.source_id.as_deref())
                            .and_then(|id| nb.source(id))
                            .or_else(|| {
                                cite::split_label(&effective_label)
                                    .and_then(|(letter, _)| nb.source_by_letter(letter))
                            });
                        let title = source_desc.map_or("Unknown source", |s| s.title.as_str());
                        let letter = resolved
                            .map(|c| c.letter.as_str())
                            .filter(|l| !l.is_empty())
                            .or_else(|| source_desc.map(|s| s.letter.as_str()))
                            .unwrap_or("?");
                        let page = resolved
                            .and_then(|c| c.page)
                            .map(|p| format!(", p. {p}"))
                            .unwrap_or_default();
                        let url = source_desc
                            .and_then(|s| s.url.as_deref())
                            .map(|u| format!(" (<{u}>)"))
                            .unwrap_or_default();
                        let unverified = if resolved.is_some_and(|c| !c.verified) {
                            " *(quote not verified in source)*"
                        } else {
                            ""
                        };
                        let clean_quote = one_line(&quote);
                        footnotes.push((
                            format!("{n}"),
                            format!(
                                "**[{letter}] {title}**{page}{url} — \"{clean_quote}\"{unverified}"
                            ),
                        ));
                        n
                    });
                    body.push_str(&format!("[^{number}]"));
                }
            }
        }
        out.push_str(body.trim());
        out.push('\n');

        let mut meta = vec![turn.model.clone()];
        if turn.deep {
            meta.push("Deep search".into());
        }
        if !turn.citations.is_empty() {
            let verified = turn.citations.iter().filter(|c| c.verified).count();
            meta.push(format!(
                "{verified}/{} citations verified",
                turn.citations.len()
            ));
        }
        out.push_str(&format!("\n*{}*\n", meta.join(" · ")));
    }

    if !footnotes.is_empty() {
        out.push_str("\n---\n\n## References\n\n");
        for (n, text) in footnotes {
            out.push_str(&format!("[^{n}]: {text}\n"));
        }
    }

    out
}

/// Text on a single line, as a heading or a footnote needs it: a line break
/// inside either would end it early.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// BibTeX bibliography of every source in the notebook.
pub fn bibtex(nb: &Notebook) -> String {
    let mut out = String::new();
    for source in &nb.sources {
        let entry_type = match source.kind {
            SourceKind::Epub => "book",
            _ => "misc",
        };
        let key = bibtex_key(source);
        let kind_label = match source.kind {
            SourceKind::Pdf => "PDF",
            SourceKind::Markdown => "Markdown",
            SourceKind::Text => "Text",
            SourceKind::Web => "Web page",
            SourceKind::Docx => "Word document",
            SourceKind::Epub => "EPUB",
            SourceKind::Youtube => "YouTube video",
            SourceKind::Audio => "Audio recording",
        };
        out.push_str(&format!("@{entry_type}{{{key},\n"));
        out.push_str(&format!(
            "  title = {{{}}},\n",
            escape_bibtex(&source.title)
        ));
        if let Some(url) = &source.url {
            let url = bibtex_url(url);
            out.push_str(&format!("  url = {{{url}}},\n"));
            out.push_str(&format!("  howpublished = {{\\url{{{url}}}}},\n"));
        }
        out.push_str(&format!(
            "  note = {{Bowerbird source {} ({kind_label})}}\n",
            source.letter
        ));
        out.push_str("}\n\n");
    }
    out
}

fn bibtex_key(source: &Source) -> String {
    let title_part: String = notebook::slug(&source.title)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(28)
        .collect();
    let title_part = title_part.trim_matches('-');
    let letter = source.letter.to_ascii_lowercase();
    if title_part.is_empty() {
        format!("source_{letter}_{}", &source.id[..6.min(source.id.len())])
    } else {
        format!("{letter}_{}", title_part.replace('-', "_"))
    }
}

/// A URL for a `url` field or `\url{}`, which reference managers and LaTeX
/// read as written: a backslash escape would become part of the link. Only
/// braces change, since they would end the field early.
fn bibtex_url(url: &str) -> String {
    url.replace('{', "%7B").replace('}', "%7D")
}

fn escape_bibtex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Packs the notebook's plain files (`notebook.toml`, `sources/`, `notes/`,
/// `chats/`, `studio/`, `guide.json`) into `out`, along with a generated
/// `references.bib` and Markdown conversation exports under `exports/chats/`.
/// Skips derived caches (`.bowerbird/`), temporary files, and symlinks. Files
/// are copied through one at a time, so a notebook of long recordings needs
/// no more memory than a small one.
pub fn notebook_zip(nb: &Notebook, out: impl Write + Seek) -> Result<()> {
    let mut zip = ZipWriter::new(out);

    for name in ["notebook.toml", "guide.json"] {
        let path = nb.dir.join(name);
        if path.is_file() && !is_symlink(&path)? {
            add_file_to_zip(&mut zip, name, &path)?;
        }
    }

    for folder in ["sources", "notes", "chats", "studio"] {
        let dir = nb.dir.join(folder);
        if dir.is_dir() && !is_symlink(&dir)? {
            add_dir_to_zip(&mut zip, &nb.dir, &dir)?;
        }
    }

    let bib = bibtex(nb);
    if !bib.is_empty() {
        zip.start_file("references.bib", deflated())
            .map_err(zip_err)?;
        zip.write_all(bib.as_bytes())?;
    }

    for item in nb.items("chats", "jsonl")? {
        let path = nb.item_path("chats", &item.id, "jsonl")?;
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let turns: Vec<Answer> = raw
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if turns.is_empty() {
            continue;
        }
        let md = chat_markdown(nb, &turns);
        zip.start_file(format!("exports/chats/{}.md", item.id), deflated())
            .map_err(zip_err)?;
        zip.write_all(md.as_bytes())?;
    }

    zip.finish().map_err(zip_err)?;
    Ok(())
}

fn add_dir_to_zip<W: Write + Seek>(zip: &mut ZipWriter<W>, root: &Path, dir: &Path) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".tmp") || name_str.starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            add_dir_to_zip(zip, root, &path)?;
        } else if kind.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let zip_name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            add_file_to_zip(zip, &zip_name, &path)?;
        }
    }
    Ok(())
}

fn add_file_to_zip<W: Write + Seek>(zip: &mut ZipWriter<W>, name: &str, path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let options = if already_compressed(path) {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        deflated()
    };
    // An entry of 4 GiB or more needs the larger header.
    let large = file.metadata()?.len() > u64::from(u32::MAX);
    zip.start_file(name, options.large_file(large))
        .map_err(zip_err)?;
    std::io::copy(&mut file, zip)?;
    Ok(())
}

fn deflated() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

/// Recordings, pictures and packaged documents are compressed as they are;
/// deflating them again takes time and saves nothing.
fn already_compressed(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| {
            notebook::AUDIO_EXTENSIONS.contains(&ext.as_str())
                || matches!(
                    ext.as_str(),
                    "pdf" | "docx" | "epub" | "png" | "jpg" | "jpeg" | "gif" | "webp"
                )
        })
}

fn is_symlink(path: &Path) -> Result<bool> {
    Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
}

fn zip_err(e: zip::result::ZipError) -> Error {
    Error::Invalid(format!("Couldn't build the zip archive: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::{Citation, Timings};
    use crate::index::Index;
    use std::io::Cursor;
    use zip::ZipArchive;

    fn sample_notebook(name: &str) -> Notebook {
        let dir =
            std::env::temp_dir().join(format!("bowerbird-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Thirty Years' War").unwrap();
        nb.add_text(
            "Peace of Westphalia",
            "Signed in 1648 at Münster and Osnabrück.",
        )
        .unwrap();
        nb.write_note("n1", "Key dates", "Ended in 1648.").unwrap();
        Index::open_synced(&nb).unwrap();
        nb
    }

    fn sample_turn(source_id: &str) -> Answer {
        Answer {
            ts: 100,
            question: "When was the peace signed?".into(),
            model: "deepseek-flash".into(),
            text: "The treaty was signed in 1648 [[A0|Signed in 1648]] and ended the war [[A0|ended the war]].".into(),
            citations: vec![
                Citation {
                    label: "A0".into(),
                    letter: "A".into(),
                    source_id: Some(source_id.into()),
                    page: Some(3),
                    quote: "Signed in 1648".into(),
                    verified: true,
                },
                Citation {
                    label: "A0".into(),
                    letter: "A".into(),
                    source_id: Some(source_id.into()),
                    page: None,
                    quote: "ended the war".into(),
                    verified: false,
                },
            ],
            timings: Timings::default(),
            deep: false,
        }
    }

    #[test]
    fn chat_markdown_formats_footnotes_and_marks_unverified_quotes() {
        let nb = sample_notebook("md");
        let turn = sample_turn(&nb.sources[0].id);
        let md = chat_markdown(&nb, &[turn]);
        assert!(md.contains("# Thirty Years' War"), "{md}");
        assert!(md.contains("The treaty was signed in 1648 [^1]"), "{md}");
        assert!(
            md.contains("[^1]: **[A] Peace of Westphalia**, p. 3 — \"Signed in 1648\""),
            "{md}"
        );
        assert!(md.contains("[^2]: **[A] Peace of Westphalia** — \"ended the war\" *(quote not verified in source)*"), "{md}");

        // A question typed over two lines stays one heading.
        let mut asked_in_two_lines = sample_turn(&nb.sources[0].id);
        asked_in_two_lines.question = "When was the peace signed?\nAnd where?".into();
        let md = chat_markdown(&nb, &[asked_in_two_lines]);
        assert!(
            md.contains("\n## 1. When was the peace signed? And where?\n"),
            "{md}"
        );
    }

    #[test]
    fn bibtex_escapes_special_characters_and_includes_urls() {
        let mut nb = sample_notebook("bib");
        nb.sources[0].title = "100% Peace & Treaties #1".into();
        nb.sources[0].url = Some("https://example.org/peace_1648?a=1&b=50%25".into());
        let bib = bibtex(&nb);
        assert!(bib.contains("@misc{a_100_peace_treaties_1,"), "{bib}");
        assert!(
            bib.contains(r"title = {100\% Peace \& Treaties \#1}"),
            "{bib}"
        );
        assert!(
            bib.contains("url = {https://example.org/peace_1648?a=1&b=50%25}"),
            "{bib}"
        );
        assert!(
            bib.contains(r"howpublished = {\url{https://example.org/peace_1648?a=1&b=50%25}}"),
            "{bib}"
        );
    }

    #[test]
    fn notebook_zip_packs_plain_files_and_exports_without_cache() {
        let mut nb = sample_notebook("zip");
        nb.append_chat("c1", &sample_turn(&nb.sources[0].id))
            .unwrap();
        let recording = nb.add_named("talk.mp3", b"not really sound").unwrap();
        let mut packed = Cursor::new(Vec::new());
        notebook_zip(&nb, &mut packed).unwrap();
        let mut archive = ZipArchive::new(packed).unwrap();
        let stored = |archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str| {
            archive.by_name(name).unwrap().compression() == CompressionMethod::Stored
        };
        assert!(stored(
            &mut archive,
            &format!("sources/{}.mp3", recording.id)
        ));
        assert!(!stored(&mut archive, "notes/n1.md"));
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(names.contains(&"notebook.toml".to_string()), "{names:?}");
        assert!(names.contains(&"notes/n1.md".to_string()), "{names:?}");
        assert!(names.contains(&"chats/c1.jsonl".to_string()), "{names:?}");
        assert!(names.contains(&"references.bib".to_string()), "{names:?}");
        assert!(
            names.contains(&"exports/chats/c1.md".to_string()),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with(".bowerbird")),
            "derived cache must stay out: {names:?}"
        );
    }
}
