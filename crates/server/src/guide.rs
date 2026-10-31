//! The notebook guide: an overview of the sources and questions to start from.

use std::time::Instant;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use bowerbird_core::ask::{self, Citation, Guide};
use bowerbird_core::notebook::{Notebook, SourceKind};
use serde::Serialize;

use crate::chats::{PieceView, pieces};
use crate::{ApiResult, AppState, blocking, detached, open_notebook, prepare, write_json};

const GUIDE_FILE: &str = "guide.json";

#[derive(Serialize)]
struct GuideView {
    model: String,
    pieces: Vec<PieceView>,
    citations: Vec<Citation>,
    questions: Vec<String>,
}

impl From<Guide> for GuideView {
    fn from(g: Guide) -> Self {
        GuideView {
            model: g.model,
            pieces: pieces(&g.text),
            citations: g.citations,
            questions: g.questions,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct GuideState {
    guide: Option<GuideView>,
    /// Written from the sources switched on now. A stale guide is still
    /// worth showing while a new one is written.
    fresh: bool,
}

/// The switched-on sources a guide is written from, in a stable order. A
/// recording counts once its transcript is written.
fn guide_sources(nb: &Notebook) -> Vec<String> {
    let mut ids: Vec<String> = nb
        .included()
        .filter(|s| s.kind != SourceKind::Audio || nb.transcript_path(s).exists())
        .map(|s| s.id.clone())
        .collect();
    ids.sort();
    ids
}

pub(crate) async fn guide(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<GuideState>> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let saved: Option<Guide> = std::fs::read_to_string(nb.dir.join(GUIDE_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let fresh = saved
            .as_ref()
            .is_some_and(|g| g.sources == guide_sources(&nb));
        Ok(Json(GuideState {
            guide: saved.map(GuideView::from),
            fresh,
        }))
    })
    .await
}

pub(crate) async fn write_guide(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<GuideState>> {
    detached(guide_job(state, slug)).await
}

async fn guide_job(state: AppState, slug: String) -> ApiResult<Json<GuideState>> {
    let started = Instant::now();
    let model = state.settings().chat_model().to_string();
    let client = ask::client(state.settings.clone())?;
    let (nb, ctx, prompt) = prepare(&state, &slug, "", ask::GUIDE_REQUEST.into()).await?;
    let answer = ask::answer(
        &client,
        &model,
        &ask::guide_system(),
        started,
        "",
        prompt,
        &ctx,
        &[],
        |_| {},
    )
    .await?;
    let guide = ask::guide_from(answer, guide_sources(&nb), &ctx);
    blocking(move || {
        write_json(&nb.dir.join(GUIDE_FILE), &guide)?;
        Ok(Json(GuideState {
            guide: Some(GuideView::from(guide)),
            fresh: true,
        }))
    })
    .await
}
