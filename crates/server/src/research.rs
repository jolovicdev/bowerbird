//! Finding new sources on the web for a question.

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use bowerbird_core::ask;
use bowerbird_core::search;
use serde::{Deserialize, Serialize};

use crate::{ApiError, ApiResult, AppState, Body, blocking, open_notebook};

#[derive(Deserialize)]
pub(crate) struct ResearchQuestion {
    question: String,
}

#[derive(Serialize)]
struct Found {
    #[serde(flatten)]
    hit: search::Hit,
    /// The letter of the source this page already is.
    already: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Research {
    queries: Vec<String>,
    results: Vec<Found>,
}

/// Finds web pages for a research question: the model writes a few search
/// queries, the search backend in Settings runs them, and the results come
/// back merged, with pages already in the notebook marked.
pub(crate) async fn research(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<ResearchQuestion>,
) -> ApiResult<Json<Research>> {
    let question = body.question.trim().to_string();
    if question.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Type what you want to find sources about.".into(),
        ));
    }
    let nb = {
        let (state, slug) = (state.clone(), slug.clone());
        blocking(move || open_notebook(&state, &slug)).await?
    };
    let settings = state.settings().clone();
    let client = ask::client(state.settings.clone())?;
    let queries = search::queries(&client, settings.chat_model(), &question).await?;
    let outcomes =
        futures::future::join_all(queries.iter().map(|q| search::search(&settings, q))).await;
    let mut lists = Vec::new();
    let mut first_error = None;
    for outcome in outcomes {
        match outcome {
            Ok(hits) => lists.push(hits),
            Err(e) => first_error = first_error.or(Some(e)),
        }
    }
    if lists.is_empty()
        && let Some(e) = first_error
    {
        return Err(e.into());
    }
    let results = search::merge(lists)
        .into_iter()
        .map(|hit| Found {
            already: nb.source_with_url(&hit.url).map(|s| s.letter.clone()),
            hit,
        })
        .collect();
    Ok(Json(Research { queries, results }))
}
