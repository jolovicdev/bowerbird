//! HTTP API over the core, plus the built UI from the same origin. Every
//! `/api` request needs the token printed at startup, as a bearer header or
//! as the cookie a sign-in sets.

mod chats;
mod embedding;
mod guide;
mod notebooks;
mod notes;
mod research;
mod scans;
mod settings;
mod sources;
mod speech;
mod studio;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, FromRequest, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use bowerbird_core::ask::{self, SharedSettings};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::Notebook;
use bowerbird_core::retrieve::{self, Context};
use bowerbird_core::settings::Settings;
use bowerbird_core::web;
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};

use chats::usable_sources;
use speech::Progress;

/// File the UI build writes as its single-page-app entry point.
pub const UI_SHELL: &str = "200.html";

pub struct Config {
    pub library: PathBuf,
    pub token: String,
    /// Built UI (`ui/build`). Without it the server is API-only.
    pub ui_dir: Option<PathBuf>,
    /// Where Settings saves, and what it holds at startup.
    pub settings_path: PathBuf,
    pub settings: Settings,
    /// Whether a link added through the API may point at this machine or its
    /// local network. The listening address can't decide this: behind a
    /// reverse proxy every request arrives from loopback.
    pub private_links: bool,
}

/// Largest request body any API route takes: one uploaded file with its form
/// headers, or a JSON body such as pasted text. Bodies are read into memory
/// whole, so this bounds what one request holds.
const UPLOAD_LIMIT: usize = 200 * 1024 * 1024;
#[derive(Clone)]
struct AppState {
    library: Arc<PathBuf>,
    token: Arc<str>,
    /// What a signed-in browser's cookie holds: a hash of the token, so the
    /// cookie never carries the password, and a new token signs everyone out.
    session: Arc<str>,
    /// Held by each sign-in, so wrong passwords are tried one at a time.
    sign_ins: Arc<tokio::sync::Mutex<()>>,
    /// One lock per notebook slug, held by anything that changes the notebook
    /// or syncs its index, so two requests never rewrite notebook.toml or index
    /// one source at once. Per notebook, so a slow ingest blocks only its own.
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    settings: SharedSettings,
    settings_path: Arc<PathBuf>,
    /// Recordings and transcriptions under way, by "slug/studio/id" or
    /// "slug/sources/id", with how far each got.
    jobs: Arc<Mutex<HashMap<String, Progress>>>,
    /// Held by the recording, transcription, or OCR that is running: each
    /// one uses every core.
    speech: Arc<tokio::sync::Mutex<()>>,
    /// Held by the notebook whose chunks are being embedded.
    embedding: Arc<tokio::sync::Mutex<()>>,
    /// Where links added through the API may be fetched from.
    reach: web::Reach,
}

impl AppState {
    fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        locks.entry(key.to_string()).or_default().clone()
    }

    fn settings(&self) -> RwLockReadGuard<'_, Settings> {
        self.settings.read().unwrap_or_else(PoisonError::into_inner)
    }
}

pub fn router(config: Config) -> Router {
    let state = AppState {
        library: Arc::new(config.library),
        session: session_for(&config.token).into(),
        sign_ins: Arc::default(),
        token: config.token.into(),
        locks: Arc::default(),
        settings: Arc::new(RwLock::new(config.settings)),
        settings_path: Arc::new(config.settings_path),
        jobs: Arc::default(),
        speech: Arc::default(),
        embedding: Arc::default(),
        reach: if config.private_links {
            web::Reach::Any
        } else {
            web::Reach::Public
        },
    };
    let api = Router::new()
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/settings/test", post(settings::test_model))
        .route("/settings/models", get(settings::list_models))
        .route("/search", get(notebooks::search_library))
        .route(
            "/notebooks",
            get(notebooks::list_notebooks).post(notebooks::create_notebook),
        )
        .route(
            "/notebooks/{slug}",
            get(notebooks::notebook)
                .patch(notebooks::rename_notebook)
                .delete(notebooks::delete_notebook),
        )
        .route("/notebooks/{slug}/search", get(notebooks::search_notebook))
        .route("/notebooks/{slug}/bibtex", get(notebooks::export_bibtex))
        .route("/notebooks/{slug}/export", get(notebooks::export_notebook))
        .route(
            "/notebooks/{slug}/sources/{id}",
            patch(sources::update_source).delete(sources::remove_source),
        )
        .route(
            "/notebooks/{slug}/sources/{id}/reread",
            post(sources::reread_source),
        )
        .route("/notebooks/{slug}/chats", get(chats::list_chats))
        .route(
            "/notebooks/{slug}/chats/{id}",
            get(chats::chat).delete(chats::delete_chat),
        )
        .route(
            "/notebooks/{slug}/chats/{id}/export",
            get(chats::export_chat),
        )
        .route(
            "/notebooks/{slug}/guide",
            get(guide::guide).post(guide::write_guide),
        )
        .route(
            "/notebooks/{slug}/notes",
            get(notes::list_notes).post(notes::create_note),
        )
        .route(
            "/notebooks/{slug}/notes/{id}",
            get(notes::note)
                .put(notes::update_note)
                .delete(notes::delete_note),
        )
        .route(
            "/notebooks/{slug}/notes/{id}/source",
            post(notes::note_to_source),
        )
        .route("/notebooks/{slug}/reports", post(notes::write_report))
        .route("/notebooks/{slug}/research", post(research::research))
        .route(
            "/notebooks/{slug}/studio",
            get(studio::list_studio).post(studio::create_study),
        )
        .route(
            "/notebooks/{slug}/studio/stream",
            post(studio::create_study_stream),
        )
        .route(
            "/notebooks/{slug}/studio/{id}",
            get(studio::study_set).delete(studio::delete_study),
        )
        .route(
            "/notebooks/{slug}/studio/{id}/review",
            put(studio::review_card),
        )
        .route(
            "/notebooks/{slug}/studio/{id}/pptx",
            get(studio::slides_pptx),
        )
        .route(
            "/notebooks/{slug}/studio/{id}/audio",
            get(studio::study_audio),
        )
        .route(
            "/notebooks/{slug}/studio/{id}/attempts",
            post(studio::record_attempt),
        )
        .route("/notebooks/{slug}/ask", post(chats::ask_question))
        .route("/notebooks/{slug}/sources/file", post(sources::add_files))
        .route("/notebooks/{slug}/sources/url", post(sources::add_url))
        .route("/notebooks/{slug}/sources/text", post(sources::add_text))
        .route(
            "/notebooks/{slug}/sources/{id}/file",
            get(sources::source_file),
        )
        .route(
            "/notebooks/{slug}/sources/{id}/images/{file}",
            get(sources::source_image),
        )
        .route(
            "/notebooks/{slug}/sources/{id}/passages/{ord}",
            get(sources::passage),
        )
        .route(
            "/notebooks/{slug}/sources/{id}/outline",
            get(sources::outline),
        )
        .layer(DefaultBodyLimit::max(UPLOAD_LIMIT))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route("/login", post(sign_in))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "No such API route.".into()) })
        .with_state(state);
    let app = Router::new().nest("/api", api);
    match config.ui_dir {
        Some(dir) => {
            // The single-page app's shell; it routes every unknown path itself.
            let shell = dir.join(UI_SHELL);
            app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(shell)))
        }
        None => app,
    }
}

/// 256 bits for the bearer token. `RandomState` keys come from the OS random
/// source, so keyed hashes of a counter are unpredictable from outside the
/// process; good enough for a local token, not for long-lived secrets.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    (0..4u64)
        .map(|i| {
            let mut h = state.build_hasher();
            h.write_u64(i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

const SESSION_COOKIE: &str = "bowerbird_session";
const SESSION_SECONDS: u32 = 30 * 24 * 60 * 60;

fn session_for(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("bowerbird session {token}"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
}

async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let signed_in = bearer.is_some_and(|t| constant_time_eq(t.as_bytes(), state.token.as_bytes()))
        || session_cookie(headers)
            .is_some_and(|s| constant_time_eq(s.as_bytes(), state.session.as_bytes()));
    if signed_in {
        next.run(request).await
    } else {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Missing or wrong access token.".into(),
        )
        .into_response()
    }
}

#[derive(serde::Deserialize)]
struct SignIn {
    password: String,
}

/// Trades the password, which is the token, for a cookie that keeps this
/// browser signed in.
async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Body(body): Body<SignIn>,
) -> Response {
    // A password someone chose can be guessable: wrong ones wait their turn
    // and a second each, so guesses come one a second however many are sent.
    let _turn = state.sign_ins.lock().await;
    if !constant_time_eq(body.password.as_bytes(), state.token.as_bytes()) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return ApiError(StatusCode::UNAUTHORIZED, "Wrong password.".into()).into_response();
    }
    // Over plain HTTP a browser drops a Secure cookie; a reverse proxy serving
    // HTTPS says so in this header.
    let secure = headers
        .get("x-forwarded-proto")
        .is_some_and(|p| p == "https");
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/api; Max-Age={SESSION_SECONDS}; HttpOnly; SameSite=Strict{}",
        state.session,
        if secure { "; Secure" } else { "" }
    );
    ([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<bowerbird_core::Error> for ApiError {
    fn from(e: bowerbird_core::Error) -> Self {
        let status = match &e {
            bowerbird_core::Error::Invalid(_) | bowerbird_core::Error::Pdf(_) => {
                StatusCode::BAD_REQUEST
            }
            bowerbird_core::Error::Http(_) | bowerbird_core::Error::Model(_) => {
                StatusCode::BAD_GATEWAY
            }
            bowerbird_core::Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(status, e.to_string())
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// A JSON request body whose rejections answer in the API's error shape.
struct Body<T>(T);

impl<S: Send + Sync, T: serde::de::DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Body(value)),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                Err(too_large())
            }
            Err(rejection) => Err(ApiError(rejection.status(), rejection.body_text())),
        }
    }
}

fn too_large() -> ApiError {
    ApiError(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "That's more than Bowerbird takes in one request ({} MB).",
            UPLOAD_LIMIT >> 20
        ),
    )
}

/// Runs work that should finish even if the reader leaves, such as writing a
/// quiz. A dropped request would otherwise cancel it halfway, losing what the
/// model already wrote.
async fn detached<T: Send + 'static>(
    work: impl std::future::Future<Output = ApiResult<T>> + Send + 'static,
) -> ApiResult<T> {
    tokio::spawn(work)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// Runs blocking core work (SQLite, PDF extraction, file reads) off the async threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// Slugs come from URLs, so only a single name inside the library is accepted.
/// Any such name will do: a folder someone copied needn't look like a new slug.
fn notebook_dir(library: &Path, slug: &str) -> ApiResult<PathBuf> {
    let dir = library.join(slug);
    if bowerbird_core::notebook::plain_file_name(slug) && dir.join("notebook.toml").exists() {
        Ok(dir)
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("No notebook named \"{slug}\"."),
        ))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Writes JSON through a temporary file, so a crash never leaves half a file.
fn write_json(path: &Path, value: &impl Serialize) -> ApiResult<()> {
    let raw = serde_json::to_string_pretty(value)
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| bowerbird_core::Error::from(e).into())
}

fn open_notebook(state: &AppState, slug: &str) -> ApiResult<Notebook> {
    Ok(Notebook::open(&notebook_dir(&state.library, slug)?)?)
}

/// Passages and the prompt for work over the switched-on sources: the
/// passages matching `topic`, or an overview of every source when there is
/// no topic or nothing matches it.
async fn prepare(
    state: &AppState,
    slug: &str,
    topic: &str,
    request: String,
) -> ApiResult<(Notebook, Context, String)> {
    let (state, slug, topic) = (state.clone(), slug.to_string(), topic.trim().to_string());
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let nb = open_notebook(&state, &slug)?;
        usable_sources(&nb)?;
        let (index, _) = Index::open_synced(&nb)?;
        let mut ctx = if topic.is_empty() {
            None
        } else {
            Some(retrieve::gather(&index, &nb, &topic)?)
        };
        if ctx.as_ref().is_none_or(|c| c.passages.is_empty()) {
            ctx = Some(retrieve::gather_overview(&index, &nb)?);
        }
        let ctx = ctx.expect("set above");
        if ctx.passages.is_empty() {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "This notebook has no readable text yet. Add a PDF, a link, or pasted text first."
                    .into(),
            ));
        }
        let prompt = ask::prompt(&index, &nb, &ctx, &request)?;
        Ok((nb, ctx, prompt))
    })
    .await
}

#[cfg(test)]
mod tests;
