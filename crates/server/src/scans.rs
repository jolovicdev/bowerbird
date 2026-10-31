//! Reads scanned PDFs with OCR in the background and indexes the text, one
//! at a time with the other jobs that use every core.

use std::sync::PoisonError;

use axum::http::StatusCode;
use bowerbird_core::index::Index;
use bowerbird_core::ocr::{self, Reader};

use crate::notebooks::NotebookView;
use crate::speech::Progress;
use crate::{ApiError, AppState, blocking, open_notebook};

pub(crate) fn start_reading(state: &AppState, view: &NotebookView) {
    for source in view.sources.iter().filter(|s| s.reading.is_some()) {
        tokio::spawn(read_source(
            state.clone(),
            view.slug.clone(),
            source.id.clone(),
        ));
    }
}

/// Writes the text beside the PDF and indexes it. A failure is kept beside
/// the PDF too, so it is shown instead of retried.
async fn read_source(state: AppState, slug: String, id: String) {
    let key = format!("{slug}/sources/{id}");
    if !state.claim_job(&key) {
        return;
    }
    let turn = state.speech.clone().lock_owned().await;
    let outcome = reading(&state, &slug, &id, &key).await;
    let _ = blocking({
        let (state, slug) = (state.clone(), slug.clone());
        move || {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let nb = open_notebook(&state, &slug)?;
            let Some(source) = nb.source(&id) else {
                return Ok(());
            };
            let (path, text) = match outcome {
                Ok(text) => (nb.ocr_path(source), text),
                Err(problem) => (nb.ocr_failure_path(source), problem),
            };
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, text)
                .and_then(|()| std::fs::rename(&tmp, &path))
                .map_err(bowerbird_core::Error::from)?;
            Index::open_synced(&nb)?;
            Ok(())
        }
    })
    .await;
    drop(turn);
    state.finish_job(&key);
}

async fn reading(state: &AppState, slug: &str, id: &str, key: &str) -> Result<String, String> {
    let progress = state.report(key);
    let dir = ocr::dir();
    if !ocr::ready(&dir) {
        progress(Progress::Downloading { done: 0, total: 0 });
        ocr::download(&dir, |done, total| {
            progress(Progress::Downloading { done, total })
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let path = blocking({
        let (state, slug, id) = (state.clone(), slug.to_string(), id.to_string());
        move || {
            let nb = open_notebook(&state, &slug)?;
            let source = nb
                .source(&id)
                .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "The PDF was removed.".into()))?;
            Ok(nb.source_path(source))
        }
    })
    .await
    .map_err(|e| e.1)?;
    let text = tokio::task::spawn_blocking(move || {
        let reader = Reader::load(&dir).map_err(|e| e.to_string())?;
        reader
            .read_pdf(&path, |done, total| {
                progress(Progress::Reading { done, total })
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if text.trim().is_empty() {
        return Err("No text was found on the scanned pages.".into());
    }
    Ok(text)
}
