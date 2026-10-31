//! Study sets: flashcards, quizzes, mind maps, tables, slides, infographics and audio overviews.

use std::sync::PoisonError;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bowerbird_core::ask::{self, Citation};
use bowerbird_core::notebook::Notebook;
use bowerbird_core::pptx;
use bowerbird_core::study::{self, Difficulty, Kind, StudySet};
use serde::{Deserialize, Serialize};

use crate::speech::{Progress, record_audio};
use crate::{
    ApiError, ApiResult, AppState, Body, blocking, detached, open_notebook, prepare, unix_now,
    write_json,
};

#[derive(Serialize)]
pub(crate) struct StudySummary {
    id: String,
    kind: Kind,
    title: String,
    /// Cards or questions.
    count: usize,
    /// Cards due for review now.
    due: usize,
    /// Best quiz score so far.
    best: Option<usize>,
    /// Length of an audio overview's recording; 0 while it is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<f32>,
    updated: u64,
}

#[derive(Serialize)]
pub(crate) struct StudyView {
    id: String,
    #[serde(flatten)]
    set: StudySet,
    due: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    recording: Option<Progress>,
}

fn study_view(id: String, set: StudySet) -> StudyView {
    StudyView {
        due: set.due(unix_now()),
        id,
        set,
        recording: None,
    }
}

pub(crate) fn read_study(nb: &Notebook, id: &str) -> ApiResult<StudySet> {
    let path = nb.item_path("studio", id, "json")?;
    let raw = std::fs::read_to_string(&path).map_err(bowerbird_core::Error::from)?;
    serde_json::from_str(&raw).map_err(|e| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("studio/{id}.json is damaged: {e}"),
        )
    })
}

pub(crate) async fn list_studio(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<Vec<StudySummary>>> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let now = unix_now();
        let mut out = Vec::new();
        for item in nb.items("studio", "json")? {
            let Ok(set) = read_study(&nb, &item.id) else {
                continue;
            };
            out.push(StudySummary {
                id: item.id,
                kind: set.kind,
                count: set.cards.len()
                    + set.questions.len()
                    + set.table.as_ref().map_or(0, |t| t.rows.len())
                    + set.slides.len()
                    + set.audio.as_ref().map_or(0, |a| a.lines.len()),
                due: set.due(now),
                best: set.attempts.iter().map(|a| a.score).max(),
                seconds: set.audio.as_ref().map(|a| a.seconds),
                title: set.title,
                updated: item.updated,
            });
        }
        Ok(Json(out))
    })
    .await
}

pub(crate) async fn study_set(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<StudyView>> {
    let (state, key) = (state.clone(), format!("{slug}/studio/{id}"));
    let (set, id) = blocking({
        let state = state.clone();
        move || Ok((read_study(&open_notebook(&state, &slug)?, &id)?, (slug, id)))
    })
    .await?;
    let mut view = study_view(id.1.clone(), set);
    view.recording = state.job(&key);
    // A recording stopped by a restart picks up again when its set is opened.
    let unrecorded = view
        .set
        .audio
        .as_ref()
        .is_some_and(|a| a.seconds == 0.0 && a.problem.is_empty());
    if unrecorded && view.recording.is_none() {
        tokio::spawn(record_audio(state, id.0, id.1));
        view.recording = Some(Progress::Waiting);
    }
    Ok(Json(view))
}

pub(crate) async fn slides_pptx(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let set = read_study(&nb, &id)?;
        if set.kind != Kind::Slides {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "This study set isn't a slide deck.".into(),
            ));
        }
        let place = |c: &Citation| {
            let title = c
                .source_id
                .as_deref()
                .and_then(|id| nb.source(id))
                .map_or("a removed source", |s| s.title.as_str());
            match c.page {
                Some(page) => format!("{title}, p. {page}"),
                None => title.to_string(),
            }
        };
        let bytes = pptx::deck(&set.title, &set.slides, place)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
        );
        Ok((headers, bytes).into_response())
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewStudy {
    kind: Kind,
    /// Cards or questions; a sensible number for the kind when left out.
    count: Option<usize>,
    #[serde(default)]
    difficulty: Difficulty,
    #[serde(default)]
    topic: String,
    /// What a table collects, in the user's words.
    #[serde(default)]
    describe: String,
    /// How an audio overview's hosts talk.
    #[serde(default)]
    format: study::AudioFormat,
}

pub(crate) async fn create_study(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<NewStudy>,
) -> ApiResult<Json<StudyView>> {
    // A mind map has no count; its size comes from the sources.
    let (usual, most) = match body.kind {
        Kind::Flashcards => (20, 40),
        Kind::Quiz => (10, 25),
        Kind::Slides => (10, 20),
        Kind::MindMap | Kind::Table | Kind::Infographic => (3, 3),
        Kind::Audio => (40, 90),
    };
    if body.kind == Kind::Table && body.describe.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Describe the table, such as \"battles: date, place, commanders, outcome\".".into(),
        ));
    }
    let request = study::Request {
        kind: body.kind,
        count: body.count.unwrap_or(usual).clamp(3, most),
        difficulty: body.difficulty,
        topic: body.topic,
        describe: body.describe,
        format: body.format,
    };
    detached(study_job(state, slug, request)).await
}

async fn study_job(
    state: AppState,
    slug: String,
    request: study::Request,
) -> ApiResult<Json<StudyView>> {
    let model = state.settings().chat_model().to_string();
    let client = ask::client(state.settings.clone())?;
    let (nb, ctx, prompt) =
        prepare(&state, &slug, &request.topic, study::instruction(&request)).await?;
    let set = study::write(&client, &model, &request, prompt, &ctx, unix_now()).await?;
    let (view, recorder) = blocking({
        let state = state.clone();
        let slug = slug.clone();
        move || {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            std::fs::create_dir_all(nb.dir.join("studio")).map_err(bowerbird_core::Error::from)?;
            let id = nb.new_item_id("studio", "json");
            write_json(&nb.item_path("studio", &id, "json")?, &set)?;
            let recorder = set.audio.is_some().then(|| id.clone());
            Ok((study_view(id, set), recorder))
        }
    })
    .await?;
    if let Some(id) = recorder {
        tokio::spawn(record_audio(state, slug, id));
    }
    Ok(Json(StudyView {
        recording: view.set.audio.as_ref().map(|_| Progress::Waiting),
        ..view
    }))
}

pub(crate) async fn study_audio(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let bytes = std::fs::read(nb.item_path("studio", &id, "ogg")?).map_err(|_| {
            ApiError(
                StatusCode::NOT_FOUND,
                "This audio overview isn't recorded yet.".into(),
            )
        })?;
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/ogg"));
        Ok((headers, bytes).into_response())
    })
    .await
}

pub(crate) async fn delete_study(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let nb = open_notebook(&state, &slug)?;
        nb.delete_item("studio", &id, "json")?;
        match std::fs::remove_file(nb.item_path("studio", &id, "ogg")?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(bowerbird_core::Error::from(e).into());
            }
            _ => {}
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Reads a study set, changes it, and saves it, under the notebook's lock.
pub(crate) fn update_study(
    state: &AppState,
    slug: &str,
    id: String,
    change: impl FnOnce(&mut StudySet) -> bowerbird_core::Result<()>,
) -> ApiResult<StudyView> {
    let lock = state.lock_for(slug);
    let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let nb = open_notebook(state, slug)?;
    let mut set = read_study(&nb, &id)?;
    change(&mut set)?;
    write_json(&nb.item_path("studio", &id, "json")?, &set)?;
    Ok(study_view(id, set))
}

#[derive(Deserialize)]
pub(crate) struct Review {
    card: usize,
    knew: bool,
}

pub(crate) async fn review_card(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
    Body(body): Body<Review>,
) -> ApiResult<Json<StudyView>> {
    blocking(move || {
        update_study(&state, &slug, id, |set| {
            set.review(body.card, body.knew, unix_now())
        })
        .map(Json)
    })
    .await
}

#[derive(Deserialize)]
pub(crate) struct NewAttempt {
    answers: Vec<Option<usize>>,
}

pub(crate) async fn record_attempt(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
    Body(body): Body<NewAttempt>,
) -> ApiResult<Json<StudyView>> {
    blocking(move || {
        update_study(&state, &slug, id, |set| {
            set.record(body.answers, unix_now()).map(|_| ())
        })
        .map(Json)
    })
    .await
}
