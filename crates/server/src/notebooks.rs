//! Notebooks: listing, creating, renaming and deleting them, and the view of one with its sources.

use std::sync::PoisonError;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use bowerbird_core::index::Index;
use bowerbird_core::notebook::{self, ChatStyle, Notebook, SourceKind};
use serde::{Deserialize, Serialize};

use crate::embedding::start_embedding;
use crate::scans::start_reading;
use crate::speech::{Progress, start_transcriptions};
use crate::{ApiError, ApiResult, AppState, Body, blocking, notebook_dir};

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
