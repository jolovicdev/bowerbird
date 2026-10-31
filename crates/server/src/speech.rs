//! Speech on this machine: audio overviews read aloud and recordings transcribed, one at a time.

use std::sync::PoisonError;

use axum::http::StatusCode;
use bowerbird_core::index::Index;
use bowerbird_core::settings;
use bowerbird_core::transcript;
use serde::Serialize;

use crate::notebooks::NotebookView;
use crate::studio::{read_study, update_study};
use crate::{ApiError, AppState, blocking, open_notebook, write_json};

/// How far a recording, a transcription, OCR of a scan, or a notebook's embedding got.
#[derive(Clone, Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub(crate) enum Progress {
    /// Behind another recording or transcription.
    Waiting,
    /// Fetching a speech model, which happens once.
    Downloading { done: u64, total: u64 },
    /// Lines read aloud so far.
    Speaking { done: usize, total: usize },
    /// Seconds transcribed so far, of the total when the file states it.
    Listening { done: f64, total: Option<f64> },
    /// Chunks embedded for search by meaning, of the notebook's total.
    Embedding { done: u64, total: u64 },
    /// Pages of a scanned PDF read by OCR so far.
    Reading { done: usize, total: usize },
    /// Stopped with this problem; not retried until the server restarts.
    Failed { problem: String },
}

impl AppState {
    /// Marks a job as queued; false when it is already under way.
    pub(crate) fn claim_job(&self, key: &str) -> bool {
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        if jobs.contains_key(key) {
            return false;
        }
        jobs.insert(key.to_string(), Progress::Waiting);
        true
    }

    pub(crate) fn job(&self, key: &str) -> Option<Progress> {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub(crate) fn report(&self, key: &str) -> impl Fn(Progress) + Send + Sync + 'static {
        let (jobs, key) = (self.jobs.clone(), key.to_string());
        move |stage| {
            jobs.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.clone(), stage);
        }
    }

    pub(crate) fn finish_job(&self, key: &str) {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

/// Transcribes the recordings that have no transcript yet, including any a
/// restart interrupted.
pub(crate) fn start_transcriptions(state: &AppState, view: &NotebookView) {
    for source in view.sources.iter().filter(|s| s.transcribing.is_some()) {
        tokio::spawn(transcribe_source(
            state.clone(),
            view.slug.clone(),
            source.id.clone(),
        ));
    }
}

/// Writes a recording's transcript beside it and indexes it, after any
/// recording or transcription already running. A failure is kept beside the
/// recording too, so it is shown instead of retried.
async fn transcribe_source(state: AppState, slug: String, id: String) {
    let key = format!("{slug}/sources/{id}");
    if !state.claim_job(&key) {
        return;
    }
    let turn = state.speech.clone().lock_owned().await;
    if let Err(problem) = transcription(&state, &slug, &id, &key).await {
        let _ = blocking({
            let (state, slug) = (state.clone(), slug.clone());
            move || {
                let lock = state.lock_for(&slug);
                let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let nb = open_notebook(&state, &slug)?;
                if let Some(source) = nb.source(&id) {
                    std::fs::write(nb.transcript_failure_path(source), problem)
                        .map_err(bowerbird_core::Error::from)?;
                }
                Ok(())
            }
        })
        .await;
    }
    drop(turn);
    state.finish_job(&key);
}

async fn transcription(state: &AppState, slug: &str, id: &str, key: &str) -> Result<(), String> {
    let progress = state.report(key);
    let dir = settings::models_dir().join("whisper");
    if !bowerbird_speech::transcriber_ready(&dir) {
        progress(Progress::Downloading { done: 0, total: 0 });
        bowerbird_speech::download_transcriber(&dir, |done, total| {
            progress(Progress::Downloading { done, total })
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let (bytes, title) = blocking({
        let (state, slug, id) = (state.clone(), slug.to_string(), id.to_string());
        move || {
            let nb = open_notebook(&state, &slug)?;
            let source = nb.source(&id).ok_or_else(|| {
                ApiError(StatusCode::NOT_FOUND, "The recording was removed.".into())
            })?;
            let bytes =
                std::fs::read(nb.source_path(source)).map_err(bowerbird_core::Error::from)?;
            Ok((bytes, source.title.clone()))
        }
    })
    .await
    .map_err(|e| e.1)?;
    progress(Progress::Listening {
        done: 0.0,
        total: None,
    });
    let said = tokio::task::spawn_blocking(move || {
        let listener = bowerbird_speech::listen::Listener::load(&dir).map_err(|e| e.to_string())?;
        listener
            .transcribe(&bytes, |done, total| {
                progress(Progress::Listening { done, total })
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if said.is_empty() {
        return Err("No speech was found in this recording.".into());
    }
    let said: Vec<(u64, String)> = said.into_iter().map(|s| (s.start as u64, s.text)).collect();
    let markdown = transcript::markdown(
        &title,
        "A recording, transcribed on this computer.",
        &said,
        &[],
    );
    blocking({
        let (state, slug, id) = (state.clone(), slug.to_string(), id.to_string());
        move || {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let nb = open_notebook(&state, &slug)?;
            let Some(source) = nb.source(&id) else {
                return Ok(());
            };
            let path = nb.transcript_path(source);
            let tmp = path.with_extension("md.tmp");
            std::fs::write(&tmp, markdown)
                .and_then(|()| std::fs::rename(&tmp, &path))
                .map_err(bowerbird_core::Error::from)?;
            Index::open_synced(&nb)?;
            Ok(())
        }
    })
    .await
    .map_err(|e| e.1)
}

/// Reads an audio overview's script aloud and saves the recording beside it,
/// after any recording already running. A failure is saved with the set.
pub(crate) async fn record_audio(state: AppState, slug: String, id: String) {
    let key = format!("{slug}/studio/{id}");
    if !state.claim_job(&key) {
        return;
    }
    let turn = state.speech.clone().lock_owned().await;
    if let Err(problem) = recording(&state, &slug, &id, &key).await {
        let _ = blocking({
            let (state, slug, id) = (state.clone(), slug.clone(), id.clone());
            move || {
                update_study(&state, &slug, id, |set| {
                    if let Some(audio) = &mut set.audio {
                        audio.problem = problem;
                    }
                    Ok(())
                })
            }
        })
        .await;
    }
    drop(turn);
    state.finish_job(&key);
}

async fn recording(state: &AppState, slug: &str, id: &str, key: &str) -> Result<(), String> {
    let progress = state.report(key);
    let dir = settings::models_dir().join("kokoro");
    if !bowerbird_speech::voices_ready(&dir) {
        progress(Progress::Downloading { done: 0, total: 0 });
        bowerbird_speech::download_voices(&dir, |done, total| {
            progress(Progress::Downloading { done, total })
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let lines: Vec<(u8, String)> = blocking({
        let (state, slug, id) = (state.clone(), slug.to_string(), id.to_string());
        move || {
            let set = read_study(&open_notebook(&state, &slug)?, &id)?;
            Ok(set
                .audio
                .map(|a| a.lines.into_iter().map(|l| (l.host, l.text)).collect())
                .unwrap_or_default())
        }
    })
    .await
    .map_err(|e| e.1)?;
    let total = lines.len();
    progress(Progress::Speaking { done: 0, total });
    let (ogg, starts, seconds) = tokio::task::spawn_blocking(move || {
        let speaker = bowerbird_speech::Speaker::load(&dir).map_err(|e| e.to_string())?;
        let rate = bowerbird_speech::SAMPLE_RATE as f32;
        let mut samples = Vec::new();
        let mut starts = Vec::with_capacity(total);
        let mut previous = None;
        for (i, (host, text)) in lines.iter().enumerate() {
            // A breath between one host's lines, a longer pause when the
            // other host answers.
            let pause = match previous {
                None => 0.0,
                Some(last) if last == *host => 0.25,
                Some(_) => 0.45,
            };
            samples.extend(std::iter::repeat_n(0.0, (pause * rate) as usize));
            starts.push(samples.len() as f32 / rate);
            samples.extend(
                speaker
                    .speak(usize::from(*host), text)
                    .map_err(|e| e.to_string())?,
            );
            previous = Some(*host);
            progress(Progress::Speaking { done: i + 1, total });
        }
        let ogg = bowerbird_speech::ogg_opus(&samples).map_err(|e| e.to_string())?;
        Ok::<_, String>((ogg, starts, samples.len() as f32 / rate))
    })
    .await
    .map_err(|e| e.to_string())??;
    blocking({
        let (state, slug, id) = (state.clone(), slug.to_string(), id.to_string());
        move || {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let nb = open_notebook(&state, &slug)?;
            let mut set = read_study(&nb, &id)?;
            let path = nb.item_path("studio", &id, "ogg")?;
            let tmp = path.with_extension("ogg.tmp");
            std::fs::write(&tmp, &ogg)
                .and_then(|()| std::fs::rename(&tmp, &path))
                .map_err(bowerbird_core::Error::from)?;
            if let Some(audio) = &mut set.audio {
                for (line, start) in audio.lines.iter_mut().zip(starts) {
                    line.start = start;
                }
                audio.seconds = seconds;
            }
            write_json(&nb.item_path("studio", &id, "json")?, &set)
        }
    })
    .await
    .map_err(|e| e.1)
}
