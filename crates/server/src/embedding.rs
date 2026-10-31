//! Embeds each notebook's new chunks in the background, so search matches by
//! meaning as well as by keyword. One notebook at a time; questions asked
//! meanwhile are answered from keyword search and the chunks done so far.

use std::sync::PoisonError;

use bowerbird_core::embed::{self, Embedder};
use bowerbird_core::index::Index;

use crate::notebooks::NotebookView;
use crate::speech::Progress;
use crate::{ApiResult, AppState, blocking, open_notebook};

pub(crate) fn start_embedding(state: &AppState, view: &NotebookView) {
    if view.embedding.is_some() {
        tokio::spawn(embed_notebook(state.clone(), view.slug.clone()));
    }
}

async fn embed_notebook(state: AppState, slug: String) {
    let key = format!("{slug}/embedding");
    if !state.claim_job(&key) {
        return;
    }
    let turn = state.embedding.clone().lock_owned().await;
    match fill(&state, &slug, &key).await {
        Ok(()) => state.finish_job(&key),
        // Left in place of the job, so an offline machine isn't asked to
        // download the model again on every look at the notebook.
        Err(e) => state.report(&key)(Progress::Failed { problem: e.1 }),
    }
    drop(turn);
}

async fn fill(state: &AppState, slug: &str, key: &str) -> ApiResult<()> {
    let progress = state.report(key);
    let dir = embed::dir();
    if !embed::ready(&dir) {
        progress(Progress::Downloading { done: 0, total: 0 });
        embed::download(&dir, |done, total| {
            progress(Progress::Downloading { done, total })
        })
        .await?;
    }
    let embedder = blocking(|| {
        Embedder::shared().ok_or_else(|| {
            bowerbird_core::Error::Model("The search model failed to load.".into()).into()
        })
    })
    .await?;
    loop {
        // The notebook's lock is held only to read and write the index, not
        // while embedding, so adding sources and asking aren't held up.
        let (rows, done, total) = blocking({
            let (state, slug) = (state.clone(), slug.to_string());
            move || {
                let lock = state.lock_for(&slug);
                let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let index = Index::open(&open_notebook(&state, &slug)?.index_path())?;
                Ok((
                    index.unembedded(embed::BATCH)?,
                    index.vector_count()?,
                    index.chunk_count()?,
                ))
            }
        })
        .await?;
        if rows.is_empty() {
            return Ok(());
        }
        progress(Progress::Embedding { done, total });
        let embedder = embedder.clone();
        let (state, slug) = (state.clone(), slug.to_string());
        blocking(move || {
            let vectors = embedder.documents(&rows)?;
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let mut index = Index::open(&open_notebook(&state, &slug)?.index_path())?;
            index.store_vectors(&rows, &vectors)?;
            Ok(())
        })
        .await?;
    }
}
