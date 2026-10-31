//! Sources: adding files, links and text, switching them on and off, and reading them back.

use std::sync::PoisonError;

use axum::Json;
use axum::extract::{Multipart, Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bowerbird_core::ask::Citation;
use bowerbird_core::index::Index;
use bowerbird_core::notebook::{self, Notebook, SourceKind};
use bowerbird_core::web;
use serde::{Deserialize, Serialize};

use crate::notebooks::{NotebookView, add_noting_duplicate, notebook_view};
use crate::speech::start_transcriptions;
use crate::{ApiError, ApiResult, AppState, Body, blocking, detached, notebook_dir, too_large};

#[derive(Deserialize)]
pub(crate) struct SourceUpdate {
    title: Option<String>,
    /// Leave the source out of answers and study material.
    excluded: Option<bool>,
}

pub(crate) async fn update_source(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
    Body(body): Body<SourceUpdate>,
) -> ApiResult<Json<NotebookView>> {
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut nb = Notebook::open(&dir)?;
        if let Some(title) = &body.title {
            nb.rename_source(&id, title)?;
        }
        if let Some(excluded) = body.excluded {
            nb.set_excluded(&id, excluded)?;
        }
        notebook_view(&state, slug).map(Json)
    })
    .await
}

pub(crate) async fn remove_source(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<NotebookView>> {
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        Notebook::open(&dir)?.remove_source(&id)?;
        notebook_view(&state, slug).map(Json)
    })
    .await
}

pub(crate) async fn add_files(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    mut form: Multipart,
) -> ApiResult<Json<NotebookView>> {
    let dir = notebook_dir(&state.library, &slug)?;
    let bad_upload = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return too_large();
        }
        ApiError(
            StatusCode::BAD_REQUEST,
            format!("The upload broke off: {e}"),
        )
    };
    let mut files = Vec::new();
    while let Some(field) = form.next_field().await.map_err(bad_upload)? {
        let name = field.file_name().unwrap_or("Untitled").to_string();
        files.push((name, field.bytes().await.map_err(bad_upload)?));
    }
    detached(async move {
        let view = blocking({
            let state = state.clone();
            move || {
                for (name, bytes) in &files {
                    notebook::check_file(name, bytes)?;
                }
                let lock = state.lock_for(&slug);
                let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let mut nb = Notebook::open(&dir)?;
                let mut already = Vec::new();
                for (name, bytes) in &files {
                    add_noting_duplicate(&mut nb, &mut already, |nb| nb.add_named(name, bytes))?;
                }
                Ok(NotebookView {
                    already,
                    ..notebook_view(&state, slug)?
                })
            }
        })
        .await?;
        start_transcriptions(&state, &view);
        Ok(Json(view))
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewUrl {
    url: String,
}

pub(crate) async fn add_url(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NewUrl>,
) -> ApiResult<Json<NotebookView>> {
    let dir = notebook_dir(&state.library, &slug)?;
    let url = body.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Links need to start with http:// or https://.".into(),
        ));
    }
    let known = Notebook::open(&dir)?.source_with_url(&url).is_some();
    detached(async move {
        // Fetched before taking the lock, so a slow site doesn't block the notebook.
        let page = if known {
            None
        } else {
            Some(web::fetch(&url).await?)
        };
        // The page's pictures are saved before its first index sync, which is
        // the only time its figures are read.
        let (page, images) = match page {
            Some(page) => {
                let url = url.clone();
                let (page, srcs) = tokio::task::spawn_blocking(move || {
                    let srcs = web::figure_sources(&page, &url);
                    (page, srcs)
                })
                .await
                .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let images = web::fetch_images(&srcs).await;
                (Some(page), images)
            }
            None => (None, Vec::new()),
        };
        blocking(move || {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let mut nb = Notebook::open(&dir)?;
            let mut already = Vec::new();
            match (&page, nb.source_with_url(&url)) {
                (_, Some(existing)) => already.push(existing.letter.clone()),
                (Some(page), None) => {
                    add_noting_duplicate(&mut nb, &mut already, |nb| nb.add_web_page(&url, page))?;
                    if let Some(source) = nb.source_with_url(&url) {
                        for (src, ext, bytes) in &images {
                            nb.save_image(source, src, ext, bytes)?;
                        }
                    }
                }
                (None, None) => {
                    unreachable!("the page is fetched unless the link was already a source")
                }
            }
            Ok(Json(NotebookView {
                already,
                ..notebook_view(&state, slug)?
            }))
        })
        .await
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewText {
    title: String,
    text: String,
}

pub(crate) async fn add_text(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NewText>,
) -> ApiResult<Json<NotebookView>> {
    let dir = notebook_dir(&state.library, &slug)?;
    if body.text.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Paste some text to add.".into(),
        ));
    }
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut nb = Notebook::open(&dir)?;
        let title = if body.title.trim().is_empty() {
            "Pasted text"
        } else {
            body.title.trim()
        };
        let mut already = Vec::new();
        add_noting_duplicate(&mut nb, &mut already, |nb| nb.add_text(title, &body.text))?;
        Ok(Json(NotebookView {
            already,
            ..notebook_view(&state, slug)?
        }))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct HeadingView {
    title: String,
    depth: u8,
    /// First passage of the section.
    ord: u32,
}

pub(crate) async fn outline(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<axum::Json<Vec<HeadingView>>> {
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let nb = Notebook::open(&dir)?;
        let index = Index::open(&nb.index_path())?;
        let headings = index
            .section_chunks(&id)?
            .into_iter()
            .map(|(s, ord)| HeadingView {
                title: s.title,
                depth: s.depth,
                ord,
            })
            .collect();
        Ok(axum::Json(headings))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct PassageView {
    section: String,
    page_start: Option<u32>,
    page_end: Option<u32>,
    text: String,
    /// Neighboring chunks in the same section, for reading context.
    before: Option<String>,
    after: Option<String>,
    figures: Vec<FigureView>,
}

/// A picture from a source, served by `source_image`.
#[derive(Serialize)]
pub(crate) struct FigureView {
    file: String,
    caption: String,
    width: u32,
    height: u32,
    /// Where it stands in a passage's text, in UTF-16 units as browsers
    /// count; set for pictures shown inside a passage.
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<usize>,
    /// Index of the answer's citation whose passage holds it; set for
    /// pictures shown under an answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    citation: Option<usize>,
}

/// Most pictures shown under one answer.
const ANSWER_FIGURES: usize = 4;

/// The pictures in an answer's verified passages, a few at most, each once.
pub(crate) fn cited_figures(index: &Index, citations: &[Citation]) -> ApiResult<Vec<FigureView>> {
    let mut out: Vec<FigureView> = Vec::new();
    for (i, c) in citations.iter().enumerate() {
        let Some(source_id) = c.source_id.as_deref().filter(|_| c.verified) else {
            continue;
        };
        let Some(ord) = c.label.get(c.letter.len()..).and_then(|o| o.parse().ok()) else {
            continue;
        };
        let Some(row) = index.chunk(source_id, ord)? else {
            continue;
        };
        for f in index.figures_between(source_id, row.start, row.start + row.text.len())? {
            if out.len() == ANSWER_FIGURES {
                return Ok(out);
            }
            // Files are named by the picture's address, so a name is one picture.
            if !out.iter().any(|o| o.file == f.file) {
                out.push(FigureView {
                    file: f.file,
                    caption: f.caption,
                    width: f.width,
                    height: f.height,
                    at: None,
                    citation: Some(i),
                });
            }
        }
    }
    Ok(out)
}

pub(crate) async fn passage(
    State(state): State<AppState>,
    UrlPath((slug, id, ord)): UrlPath<(String, String, String)>,
) -> ApiResult<axum::Json<PassageView>> {
    let ord: u32 = ord.parse().map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            format!("\"{ord}\" isn't a passage number."),
        )
    })?;
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let nb = Notebook::open(&dir)?;
        let index = Index::open(&nb.index_path())?;
        let Some(row) = index.chunk(&id, ord)? else {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "That passage isn't in the index.".into(),
            ));
        };
        let neighbor = |ord: Option<u32>| -> ApiResult<Option<String>> {
            Ok(match ord {
                Some(o) => index
                    .chunk(&id, o)?
                    .filter(|n| n.section == row.section)
                    .map(|n| n.text),
                None => None,
            })
        };
        let figures = index
            .figures_between(&id, row.start, row.start + row.text.len())?
            .into_iter()
            .filter_map(|f| {
                let at = row.text.get(..f.start - row.start)?.encode_utf16().count();
                Some(FigureView {
                    file: f.file,
                    caption: f.caption,
                    width: f.width,
                    height: f.height,
                    at: Some(at),
                    citation: None,
                })
            })
            .collect();
        Ok(axum::Json(PassageView {
            before: neighbor(ord.checked_sub(1))?,
            after: neighbor(ord.checked_add(1))?,
            figures,
            section: row.section,
            page_start: row.page_start,
            page_end: row.page_end,
            text: row.text,
        }))
    })
    .await
}

pub(crate) async fn source_file(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let nb = Notebook::open(&dir)?;
        let Some(source) = nb.source(&id) else {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "No source with that id.".into(),
            ));
        };
        let bytes = std::fs::read(nb.source_path(source)).map_err(bowerbird_core::Error::from)?;
        let content_type = match source.kind {
            SourceKind::Pdf => "application/pdf",
            // Saved web pages are untrusted HTML; never render them in this origin.
            SourceKind::Markdown | SourceKind::Text | SourceKind::Web | SourceKind::Youtube => {
                "text/plain; charset=utf-8"
            }
            SourceKind::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            SourceKind::Epub => "application/epub+zip",
            SourceKind::Audio => match source.file.rsplit('.').next().unwrap_or_default() {
                "mp3" => "audio/mpeg",
                "m4a" | "aac" => "audio/mp4",
                "wav" => "audio/wav",
                "flac" => "audio/flac",
                "webm" => "audio/webm",
                _ => "audio/ogg",
            },
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox"),
        );
        Ok((headers, bytes).into_response())
    })
    .await
}

/// A picture saved beside a web page. Names are hashes with a picture
/// extension, so a request can't reach other files.
pub(crate) async fn source_image(
    State(state): State<AppState>,
    UrlPath((slug, id, file)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let content_type = file
        .split_once('.')
        .filter(|(stem, _)| stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|(_, ext)| match ext {
            "jpg" => Some("image/jpeg"),
            "png" => Some("image/png"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        })
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such picture.".into()))?;
    blocking(move || {
        let dir = notebook_dir(&state.library, &slug)?;
        let nb = Notebook::open(&dir)?;
        let source = nb
            .source(&id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No source with that id.".into()))?;
        let bytes = std::fs::read(nb.images_dir(source).join(&file))
            .map_err(|_| ApiError(StatusCode::NOT_FOUND, "No such picture.".into()))?;
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox"),
        );
        Ok((headers, bytes).into_response())
    })
    .await
}
