//! Notebooks: listing, creating, renaming and deleting them, and the view of one with its sources.

use std::io::Seek;
use std::path::PathBuf;
use std::sync::PoisonError;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bowerbird_core::ask;
use bowerbird_core::export;
use bowerbird_core::index::Index;
use bowerbird_core::notebook::{self, ChatStyle, Notebook, SourceKind};
use bowerbird_core::retrieve::{self, PassageHit};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::chats::read_chat;
use crate::embedding::start_embedding;
use crate::scans::start_reading;
use crate::speech::{Progress, start_transcriptions};
use crate::{ApiError, ApiResult, AppState, Body, blocking, notebook_dir, open_notebook};

/// Lock key for creating notebooks; no slug can contain a slash.
const LIBRARY_LOCK: &str = "/";
/// Deleted notebooks move here, so a mistaken delete can be undone by hand.
/// No slug starts with a dot, so it never lists as a notebook.
const TRASH: &str = ".trash";

#[derive(Serialize)]
struct Spine {
    letter: String,
    color: u8,
}

#[derive(Serialize)]
pub(crate) struct NotebookSummary {
    slug: String,
    title: String,
    sources: Vec<Spine>,
    /// Unix seconds of the last change to sources or chats.
    updated: u64,
}

pub(crate) async fn list_notebooks(
    State(state): State<AppState>,
) -> ApiResult<axum::Json<Vec<NotebookSummary>>> {
    blocking(move || {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(state.library.as_ref()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(axum::Json(out)),
            Err(e) => return Err(bowerbird_core::Error::from(e).into()),
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let Ok(nb) = Notebook::open(&dir) else {
                continue;
            };
            let toml = std::fs::metadata(dir.join("notebook.toml"))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let updated = [("chats", "jsonl"), ("notes", "md"), ("studio", "json")]
                .iter()
                .filter_map(|(folder, ext)| nb.items(folder, ext).ok()?.first().map(|i| i.updated))
                .fold(toml, u64::max);
            out.push(NotebookSummary {
                slug: entry.file_name().to_string_lossy().into_owned(),
                title: nb.title,
                sources: nb
                    .sources
                    .into_iter()
                    .map(|s| Spine {
                        letter: s.letter,
                        color: s.color,
                    })
                    .collect(),
                updated,
            });
        }
        out.sort_by_key(|n| std::cmp::Reverse(n.updated));
        Ok(axum::Json(out))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct SourceView {
    pub(crate) id: String,
    title: String,
    kind: SourceKind,
    letter: String,
    color: u8,
    url: Option<String>,
    pages: Option<u32>,
    chunks: u32,
    /// Left out of answers and study material.
    excluded: bool,
    /// Why the source couldn't be indexed, if it couldn't.
    error: Option<String>,
    /// A recording's transcription, until its transcript is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transcribing: Option<Progress>,
    /// A scanned PDF's OCR, until its text is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reading: Option<Progress>,
    /// True when this PDF was read (or tried to be read) with OCR.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) scanned: bool,
}

#[derive(Serialize)]
pub(crate) struct NotebookView {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) chat: ChatStyle,
    pub(crate) sources: Vec<SourceView>,
    /// Letters of sources an add request found already in the notebook.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) already: Vec<String>,
    /// Files or playlist entries skipped during a batch import.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) skipped: Vec<String>,
    /// Embedding of chunks for search by meaning, while some have no vector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) embedding: Option<Progress>,
}

pub(crate) async fn notebook(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<NotebookView>> {
    let view = blocking({
        let state = state.clone();
        move || {
            notebook_dir(&state.library, &slug)?;
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            notebook_view(&state, slug)
        }
    })
    .await?;
    start_transcriptions(&state, &view);
    Ok(Json(view))
}

/// The notebook with its sources, after bringing the index up to date, with
/// OCR of new scans and embedding of new chunks started. Callers hold the
/// notebook's lock, since syncing writes to the index.
pub(crate) fn notebook_view(state: &AppState, slug: String) -> ApiResult<NotebookView> {
    let dir = notebook_dir(&state.library, &slug)?;
    let nb = Notebook::open(&dir)?;
    let (index, report) = Index::open_synced(&nb)?;
    let counts = index.chunk_counts()?;
    let mut sources = Vec::new();
    for s in &nb.sources {
        let untranscribed = s.kind == SourceKind::Audio
            && !nb.transcript_path(s).exists()
            && !nb.transcript_failure_path(s).exists();
        let job = || {
            state
                .job(&format!("{slug}/sources/{}", s.id))
                .unwrap_or(Progress::Waiting)
        };
        let transcribing = untranscribed.then(job);
        let reading = report.scanned.contains(&s.id).then(job);
        let scanned = s.kind == SourceKind::Pdf
            && (nb.ocr_path(s).exists() || nb.ocr_failure_path(s).exists() || reading.is_some());
        sources.push(SourceView {
            id: s.id.clone(),
            title: s.title.clone(),
            kind: s.kind,
            letter: s.letter.clone(),
            color: s.color,
            url: s.url.clone(),
            pages: index.pages(&s.id)?,
            chunks: counts.get(&s.id).copied().unwrap_or(0),
            excluded: s.excluded,
            error: report
                .failed
                .iter()
                .find(|(id, _)| *id == s.id && transcribing.is_none() && reading.is_none())
                .map(|(_, e)| e.clone()),
            transcribing,
            reading,
            scanned,
        });
    }
    let unembedded = index.vector_count()? < index.chunk_count()?;
    let embedding = unembedded.then(|| {
        state
            .job(&format!("{slug}/embedding"))
            .unwrap_or(Progress::Waiting)
    });
    let view = NotebookView {
        slug,
        title: nb.title,
        chat: nb.chat,
        sources,
        already: Vec::new(),
        skipped: Vec::new(),
        embedding,
    };
    start_embedding(state, &view);
    start_reading(state, &view);
    Ok(view)
}

/// Adds a source and records its letter when it was already there.
pub(crate) fn add_noting_duplicate(
    nb: &mut Notebook,
    already: &mut Vec<String>,
    add: impl FnOnce(&mut Notebook) -> bowerbird_core::Result<notebook::Source>,
) -> ApiResult<()> {
    let before = nb.sources.len();
    let source = add(nb)?;
    if nb.sources.len() == before {
        already.push(source.letter);
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct NotebookUpdate {
    title: Option<String>,
    /// How answers are written.
    chat: Option<ChatStyle>,
}

pub(crate) async fn rename_notebook(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NotebookUpdate>,
) -> ApiResult<Json<NotebookView>> {
    let title = body.title.map(|t| t.trim().to_string());
    if title.as_deref() == Some("") {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Give the notebook a name.".into(),
        ));
    }
    if let Some(chat) = &body.chat
        && chat.custom.chars().count() > notebook::CUSTOM_STYLE_MAX
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!(
                "Keep the answer style under {} characters.",
                notebook::CUSTOM_STYLE_MAX
            ),
        ));
    }
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut nb = Notebook::open(&dir)?;
        if let Some(title) = title {
            nb.title = title;
        }
        if let Some(chat) = body.chat {
            nb.chat = chat;
        }
        nb.save()?;
        notebook_view(&state, slug).map(Json)
    })
    .await
}

pub(crate) async fn delete_notebook(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let trash = state.library.join(TRASH);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        std::fs::create_dir_all(&trash)
            .and_then(|()| std::fs::rename(&dir, trash.join(format!("{slug}-{now}"))))
            .map_err(bowerbird_core::Error::from)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewNotebook {
    title: String,
}

pub(crate) async fn create_notebook(
    State(state): State<AppState>,
    Body(body): Body<NewNotebook>,
) -> ApiResult<Json<NotebookView>> {
    blocking(move || {
        let title = body.title.trim();
        let base = notebook::slug(title);
        if base.is_empty() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Give the notebook a name with at least one letter or digit.".into(),
            ));
        }
        let lock = state.lock_for(LIBRARY_LOCK);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let slug = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|s| !state.library.join(s).exists())
            .expect("an unused slug exists");
        Notebook::open_or_create(&state.library.join(&slug), title)?;
        notebook_view(&state, slug).map(Json)
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    include_excluded: bool,
}

#[derive(Serialize)]
pub(crate) struct NoteHit {
    id: String,
    title: String,
    snippet: String,
}

#[derive(Serialize)]
pub(crate) struct ChatHit {
    chat_id: String,
    chat_title: String,
    turn: usize,
    question: String,
    snippet: String,
}

#[derive(Serialize)]
pub(crate) struct NotebookSearch {
    query: String,
    passages: Vec<PassageHit>,
    notes: Vec<NoteHit>,
    chats: Vec<ChatHit>,
}

fn search_notes_and_chats(
    nb: &Notebook,
    query: &str,
    limit: usize,
) -> (Vec<NoteHit>, Vec<ChatHit>) {
    let needle = query.trim().to_lowercase();
    let terms = bowerbird_core::index::query_terms(query);
    if needle.is_empty() && terms.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let matches = |haystack: &str| {
        let lower = haystack.to_lowercase();
        (!needle.is_empty() && lower.contains(&needle))
            || (!terms.is_empty() && terms.iter().all(|t| lower.contains(t)))
    };

    let mut note_hits = Vec::new();
    if let Ok(items) = nb.items("notes", "md") {
        for item in items {
            let Ok((title, body)) = nb.read_note(&item.id) else {
                continue;
            };
            let plain = ask::plain_text(&body);
            if matches(&title) || matches(&plain) {
                note_hits.push(NoteHit {
                    id: item.id,
                    title,
                    snippet: retrieve::snippet_around(&plain, query, 200),
                });
                if note_hits.len() >= limit {
                    break;
                }
            }
        }
    }

    let mut chat_hits = Vec::new();
    if let Ok(items) = nb.items("chats", "jsonl") {
        for item in items {
            let Ok(turns) = read_chat(nb, &item.id) else {
                continue;
            };
            let Some(first) = turns.first() else {
                continue;
            };
            let chat_title = first.question.clone();
            for (idx, turn) in turns.iter().enumerate() {
                let plain = ask::plain_text(&turn.text);
                if matches(&turn.question) || matches(&plain) {
                    chat_hits.push(ChatHit {
                        chat_id: item.id.clone(),
                        chat_title: chat_title.clone(),
                        turn: idx + 1,
                        question: turn.question.clone(),
                        snippet: retrieve::snippet_around(&plain, query, 200),
                    });
                    if chat_hits.len() >= limit {
                        return (note_hits, chat_hits);
                    }
                }
            }
        }
    }
    (note_hits, chat_hits)
}

/// Interactive hybrid search inside one notebook: passages, notes, and conversations.
pub(crate) async fn search_notebook(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<NotebookSearch>> {
    let query = params.q.trim().to_string();
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let include_excluded = params.include_excluded;
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        if query.is_empty() {
            return Ok(Json(NotebookSearch {
                query,
                passages: Vec::new(),
                notes: Vec::new(),
                chats: Vec::new(),
            }));
        }
        let index = Index::open(&nb.index_path())?;
        let vector = retrieve::query_vector(&index, &query)?;
        let passages = retrieve::search_passages(
            &index,
            &nb,
            &query,
            vector.as_deref(),
            limit,
            include_excluded,
        )?;
        let (notes, chats) = search_notes_and_chats(&nb, &query, limit);
        Ok(Json(NotebookSearch {
            query,
            passages,
            notes,
            chats,
        }))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct LibraryNotebookHit {
    slug: String,
    title: String,
    sources: usize,
}

#[derive(Serialize)]
pub(crate) struct LibraryPassageHit {
    notebook_slug: String,
    notebook_title: String,
    #[serde(flatten)]
    hit: PassageHit,
}

#[derive(Serialize)]
pub(crate) struct LibraryNoteHit {
    notebook_slug: String,
    notebook_title: String,
    #[serde(flatten)]
    hit: NoteHit,
}

#[derive(Serialize)]
pub(crate) struct LibraryChatHit {
    notebook_slug: String,
    notebook_title: String,
    #[serde(flatten)]
    hit: ChatHit,
}

#[derive(Serialize)]
pub(crate) struct LibrarySearch {
    query: String,
    notebooks: Vec<LibraryNotebookHit>,
    passages: Vec<LibraryPassageHit>,
    notes: Vec<LibraryNoteHit>,
    chats: Vec<LibraryChatHit>,
}

/// Interactive search across every notebook in the library.
pub(crate) async fn search_library(
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<LibrarySearch>> {
    let query = params.q.trim().to_string();
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    blocking(move || {
        if query.is_empty() {
            return Ok(Json(LibrarySearch {
                query,
                notebooks: Vec::new(),
                passages: Vec::new(),
                notes: Vec::new(),
                chats: Vec::new(),
            }));
        }
        let entries = match std::fs::read_dir(state.library.as_ref()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Json(LibrarySearch {
                    query,
                    notebooks: Vec::new(),
                    passages: Vec::new(),
                    notes: Vec::new(),
                    chats: Vec::new(),
                }));
            }
            Err(e) => return Err(bowerbird_core::Error::from(e).into()),
        };
        let needle = query.to_lowercase();
        let mut notebooks = Vec::new();
        let mut passages = Vec::new();
        let mut notes = Vec::new();
        let mut chats = Vec::new();
        // Embedded once, by the first notebook that has vectors.
        let mut vector: Option<Vec<f32>> = None;

        for entry in entries.flatten() {
            let dir = entry.path();
            let Ok(nb) = Notebook::open(&dir) else {
                continue;
            };
            let slug = entry.file_name().to_string_lossy().into_owned();
            if nb.title.to_lowercase().contains(&needle)
                || nb
                    .sources
                    .iter()
                    .any(|s| s.title.to_lowercase().contains(&needle))
            {
                notebooks.push(LibraryNotebookHit {
                    slug: slug.clone(),
                    title: nb.title.clone(),
                    sources: nb.sources.len(),
                });
            }
            if nb.index_path().is_file()
                && let Ok(index) = Index::open(&nb.index_path())
            {
                if vector.is_none() {
                    vector = retrieve::query_vector(&index, &query).ok().flatten();
                }
                let hits =
                    retrieve::search_passages(&index, &nb, &query, vector.as_deref(), 5, true)
                        .unwrap_or_default();
                for hit in hits {
                    passages.push(LibraryPassageHit {
                        notebook_slug: slug.clone(),
                        notebook_title: nb.title.clone(),
                        hit,
                    });
                }
            }
            let (nb_notes, nb_chats) = search_notes_and_chats(&nb, &query, 5);
            for hit in nb_notes {
                notes.push(LibraryNoteHit {
                    notebook_slug: slug.clone(),
                    notebook_title: nb.title.clone(),
                    hit,
                });
            }
            for hit in nb_chats {
                chats.push(LibraryChatHit {
                    notebook_slug: slug.clone(),
                    notebook_title: nb.title.clone(),
                    hit,
                });
            }
        }
        passages.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
        passages.truncate(limit);
        notes.truncate(limit);
        chats.truncate(limit);
        notebooks.truncate(limit);
        Ok(Json(LibrarySearch {
            query,
            notebooks,
            passages,
            notes,
            chats,
        }))
    })
    .await
}

/// BibTeX bibliography of all sources in the notebook.
pub(crate) async fn export_bibtex(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Response> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let bib = export::bibtex(&nb);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-bibtex; charset=utf-8"),
        );
        if let Ok(disposition) = HeaderValue::from_str(&format!(
            "attachment; filename=\"{}.bib\"",
            slug.replace('"', "")
        )) {
            headers.insert(header::CONTENT_DISPOSITION, disposition);
        }
        Ok((headers, bib).into_response())
    })
    .await
}

/// Removes an export's temporary file when the response that reads it ends,
/// whether it was sent in full or the reader left early.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The next piece of an export for the response body, or None at its end.
async fn next_piece(
    (mut file, guard): (tokio::fs::File, RemoveOnDrop),
) -> std::io::Result<Option<(Bytes, (tokio::fs::File, RemoveOnDrop))>> {
    let mut piece = vec![0; 64 * 1024];
    let read = file.read(&mut piece).await?;
    piece.truncate(read);
    Ok((read > 0).then(|| (Bytes::from(piece), (file, guard))))
}

/// Portable ZIP archive of the entire notebook (plain files + BibTeX + Markdown
/// exports). It is packed into a temporary file in the notebook's cache folder
/// and sent from there piece by piece, so its size never sits in memory.
pub(crate) async fn export_notebook(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Response> {
    let name = slug.replace('"', "");
    let (file, size, guard) = blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = nb
            .index_path()
            .with_file_name(format!("export-{nanos}.tmp"));
        // A notebook unpacked from an export has no cache folder until its index is first opened.
        if let Some(cache) = path.parent() {
            std::fs::create_dir_all(cache).map_err(bowerbird_core::Error::from)?;
        }
        let mut file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(bowerbird_core::Error::from)?;
        let guard = RemoveOnDrop(path);
        export::notebook_zip(&nb, &mut file)?;
        let size = file
            .stream_position()
            .map_err(bowerbird_core::Error::from)?;
        file.rewind().map_err(bowerbird_core::Error::from)?;
        Ok((file, size, guard))
    })
    .await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}.zip\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    let pieces = futures::stream::try_unfold((tokio::fs::File::from_std(file), guard), next_piece);
    Ok((headers, axum::body::Body::from_stream(pieces)).into_response())
}
