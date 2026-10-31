//! Notes, their citations, and reports written into new notes.

use std::convert::Infallible;
use std::sync::PoisonError;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use bowerbird_core::ask::{self, Citation};
use bowerbird_core::cite::{self, Piece};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::Notebook;
use bowerbird_core::report;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::chats::send_event;
use crate::notebooks::{NotebookView, add_noting_duplicate, notebook_view};
use crate::{
    ApiError, ApiResult, AppState, Body, blocking, modified, notebook_dir, open_notebook, prepare,
};

#[derive(Serialize)]
pub(crate) struct NoteSummary {
    id: String,
    title: String,
    updated: u64,
}

#[derive(Serialize)]
pub(crate) struct NoteView {
    id: String,
    title: String,
    /// Markdown, with citations as `[[A3|quote]]` markers.
    body: String,
    /// The body's citations in order, each checked against its passage again.
    citations: Vec<Citation>,
    updated: u64,
}

#[derive(Deserialize)]
pub(crate) struct NoteBody {
    title: String,
    body: String,
}

/// Checks each citation marker in a note against the passage its label names.
fn note_citations(nb: &Notebook, body: &str) -> ApiResult<Vec<Citation>> {
    let mut parser = cite::Parser::default();
    let mut found = parser.feed(body);
    found.extend(parser.finish());
    let index = Index::open(&nb.index_path())?;
    let mut out = Vec::new();
    for piece in found {
        let Piece::Cite { label, quote } = piece else {
            continue;
        };
        let (letter, ord) = cite::split_label(&label).unwrap_or(("", 0));
        let source = nb.source_by_letter(letter);
        let row = match source {
            Some(s) => index.chunk(&s.id, ord)?,
            None => None,
        };
        out.push(Citation {
            verified: row.as_ref().is_some_and(|r| cite::verify(&quote, &r.text)),
            page: row.as_ref().and_then(|r| r.page_start),
            source_id: source.map(|s| s.id.clone()),
            letter: letter.to_string(),
            label,
            quote,
        });
    }
    Ok(out)
}

fn note_view(nb: &Notebook, id: String) -> ApiResult<NoteView> {
    let (title, body) = nb.read_note(&id)?;
    Ok(NoteView {
        citations: note_citations(nb, &body)?,
        updated: modified(&nb.item_path("notes", &id, "md")?),
        id,
        title,
        body,
    })
}

pub(crate) async fn list_notes(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<Vec<NoteSummary>>> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let mut out = Vec::new();
        for item in nb.items("notes", "md")? {
            let (title, _) = nb.read_note(&item.id)?;
            out.push(NoteSummary {
                id: item.id,
                title,
                updated: item.updated,
            });
        }
        Ok(Json(out))
    })
    .await
}

pub(crate) async fn note(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<NoteView>> {
    blocking(move || note_view(&open_notebook(&state, &slug)?, id).map(Json)).await
}

pub(crate) async fn create_note(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NoteBody>,
) -> ApiResult<Json<NoteView>> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let nb = open_notebook(&state, &slug)?;
        let id = nb.new_item_id("notes", "md");
        nb.write_note(&id, &body.title, &body.body)?;
        note_view(&nb, id).map(Json)
    })
    .await
}

pub(crate) async fn update_note(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
    Body(body): Body<NoteBody>,
) -> ApiResult<Json<NoteView>> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let nb = open_notebook(&state, &slug)?;
        nb.read_note(&id)?;
        nb.write_note(&id, &body.title, &body.body)?;
        note_view(&nb, id).map(Json)
    })
    .await
}

pub(crate) async fn delete_note(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        open_notebook(&state, &slug)?.delete_item("notes", &id, "md")?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Adds a note's text as a source, without its citation markers.
pub(crate) async fn note_to_source(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<NotebookView>> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut nb = open_notebook(&state, &slug)?;
        let (title, body) = nb.read_note(&id)?;
        let text = ask::plain_text(&body);
        if text.trim().is_empty() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "This note is empty, so there's nothing to add.".into(),
            ));
        }
        let mut already = Vec::new();
        add_noting_duplicate(&mut nb, &mut already, |nb| {
            nb.add_text(&title, &format!("# {title}\n\n{text}"))
        })?;
        Ok(Json(NotebookView {
            already,
            ..notebook_view(&state, slug)?
        }))
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewReport {
    kind: report::Kind,
    #[serde(default)]
    topic: String,
    /// What to write, for a custom report.
    #[serde(default)]
    instructions: String,
}

/// Writes a report from the sources into a new note, streamed as server-sent
/// events: `note` (its title), then `text` and `cite` as the model writes,
/// then `done` (the saved note) or `error`. The writing goes on if the reader
/// leaves, so the note is still saved.
pub(crate) async fn write_report(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NewReport>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let started = Instant::now();
    if body.instructions.chars().count() > report::INSTRUCTIONS_MAX {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!(
                "Keep the description under {} characters.",
                report::INSTRUCTIONS_MAX
            ),
        ));
    }
    let system = report::system(body.kind, &body.instructions)?;
    notebook_dir(&state.library, &slug)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SseEvent>();
    tokio::spawn(async move {
        if let Err(ApiError(_, message)) =
            report_job(state, slug, body, system, started, tx.clone()).await
        {
            send_event(&tx, "error", &serde_json::json!({ "error": message }));
        }
    });
    let events =
        futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|e| (Ok(e), rx)) },
        );
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

async fn report_job(
    state: AppState,
    slug: String,
    body: NewReport,
    system: String,
    started: Instant,
    tx: tokio::sync::mpsc::UnboundedSender<SseEvent>,
) -> ApiResult<()> {
    let model = state.settings().chat_model().to_string();
    let client = ask::client(state.settings.clone())?;
    let (nb, ctx, prompt) = prepare(&state, &slug, &body.topic, report::REQUEST.into()).await?;
    let title = report::title(body.kind, &body.topic, &nb.title);
    send_event(&tx, "note", &serde_json::json!({ "title": title }));
    let written = ask::answer(
        &client,
        &model,
        &system,
        started,
        "",
        prompt,
        &ctx,
        &[],
        |event| match event {
            ask::Event::Text(text) => send_event(&tx, "text", text),
            ask::Event::Citation(c) => send_event(&tx, "cite", c),
        },
    )
    .await?;
    // Written only when the report is complete, so no page ever opens it empty.
    let note = blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let id = nb.new_item_id("notes", "md");
        nb.write_note(&id, &title, &written.text)?;
        note_view(&nb, id)
    })
    .await?;
    send_event(&tx, "done", &note);
    Ok(())
}
