//! HTTP API over the core, plus the built UI from the same origin. Every
//! `/api` request needs the bearer token printed at startup.

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
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, FromRequest, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use bowerbird_core::ask::{self, SharedSettings};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::Notebook;
use bowerbird_core::retrieve::{self, Context};
use bowerbird_core::settings::Settings;
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
}

/// Largest request body any API route takes: one uploaded file with its form
/// headers, or a JSON body such as pasted text. Bodies are read into memory
/// whole, so this bounds what one request holds.
const UPLOAD_LIMIT: usize = 200 * 1024 * 1024;
#[derive(Clone)]
struct AppState {
    library: Arc<PathBuf>,
    token: Arc<str>,
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
        token: config.token.into(),
        locks: Arc::default(),
        settings: Arc::new(RwLock::new(config.settings)),
        settings_path: Arc::new(config.settings_path),
        jobs: Arc::default(),
        speech: Arc::default(),
        embedding: Arc::default(),
    };
    let api = Router::new()
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
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
        .route(
            "/notebooks/{slug}/sources/{id}",
            patch(sources::update_source).delete(sources::remove_source),
        )
        .route("/notebooks/{slug}/chats", get(chats::list_chats))
        .route(
            "/notebooks/{slug}/chats/{id}",
            get(chats::chat).delete(chats::delete_chat),
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

async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if constant_time_eq(presented.as_bytes(), state.token.as_bytes()) {
        next.run(request).await
    } else {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Missing or wrong access token.".into(),
        )
        .into_response()
    }
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

/// Slugs come from URLs, so only accept what `notebook::slug` produces.
fn notebook_dir(library: &Path, slug: &str) -> ApiResult<PathBuf> {
    let valid = !slug.is_empty() && slug.chars().all(|c| c.is_alphanumeric() || c == '-');
    let dir = library.join(slug);
    if valid && dir.join("notebook.toml").exists() {
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
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_distinct() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    /// A server on a free port with an empty library and settings file.
    async fn serve(name: &str) -> std::net::SocketAddr {
        let dir = std::env::temp_dir().join(format!("bowerbird-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = router(Config {
            library: dir.join("library"),
            token: "right-token".into(),
            ui_dir: None,
            settings_path: dir.join("settings.toml"),
            settings: Settings::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    /// The raw HTTP response to one request.
    async fn call(
        addr: std::net::SocketAddr,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let auth = token
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    async fn status(addr: std::net::SocketAddr, token: Option<&str>) -> String {
        let response = call(addr, "GET", "/api/notebooks", token, "").await;
        response.lines().next().unwrap_or_default().to_string()
    }

    #[tokio::test]
    async fn api_requires_the_token() {
        let addr = serve("auth").await;
        assert!(status(addr, None).await.contains("401"));
        assert!(status(addr, Some("wrong-token")).await.contains("401"));
        assert!(status(addr, Some("right-token")).await.contains("200"));
    }

    #[tokio::test]
    async fn settings_take_keys_but_never_send_them_back() {
        let addr = serve("settings").await;
        let body =
            r#"{"chat_model":"claude-sonnet-5-5","keys":{"ANTHROPIC_API_KEY":"sk-test-secret"}}"#;
        let saved = call(addr, "PUT", "/api/settings", Some("right-token"), body).await;
        assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
        let read = call(addr, "GET", "/api/settings", Some("right-token"), "").await;
        assert!(read.contains(r#""chat_model":"claude-sonnet-5-5""#));
        assert!(
            read.contains(r#""key_env":"ANTHROPIC_API_KEY","has_key":true,"saved":true"#),
            "{read}"
        );
        assert!(!saved.contains("sk-test-secret") && !read.contains("sk-test-secret"));
        let unknown = call(
            addr,
            "PUT",
            "/api/settings",
            Some("right-token"),
            r#"{"keys":{"PATH":"x"}}"#,
        )
        .await;
        assert!(unknown.starts_with("HTTP/1.1 400"));
    }

    #[tokio::test]
    async fn api_errors_are_json() {
        let addr = serve("errors").await;
        let unknown = call(addr, "GET", "/api/nope", Some("right-token"), "").await;
        assert!(
            unknown.starts_with("HTTP/1.1 404") && unknown.contains(r#"{"error":"#),
            "{unknown}"
        );
        let malformed = call(addr, "POST", "/api/notebooks", Some("right-token"), "{").await;
        assert!(
            malformed.starts_with("HTTP/1.1 400") && malformed.contains(r#"{"error":"#),
            "{malformed}"
        );
    }

    #[test]
    fn slugs_with_path_parts_are_rejected() {
        let library = std::env::temp_dir().join(format!("bowerbird-slug-{}", std::process::id()));
        let inside = library.join("birds");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("notebook.toml"), "title = \"Birds\"\n").unwrap();
        assert!(notebook_dir(&library, "birds").is_ok());
        assert!(notebook_dir(&library, "../birds").is_err());
        assert!(notebook_dir(&library, "birds/../birds").is_err());
    }

    fn json_body(response: &str) -> serde_json::Value {
        let body = response.split_once("\r\n\r\n").map_or("", |(_, b)| b);
        serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {response}"))
    }

    /// A notebook "history" with one indexed text source.
    async fn history_notebook(name: &str) -> (std::net::SocketAddr, String) {
        let addr = serve(name).await;
        let t = Some("right-token");
        call(addr, "POST", "/api/notebooks", t, r#"{"title":"History"}"#).await;
        let added = call(
            addr,
            "POST",
            "/api/notebooks/history/sources/text",
            t,
            r#"{"title":"Westphalia","text":"The Peace of Westphalia was signed in 1648 and ended the Thirty Years' War."}"#,
        )
        .await;
        let id = json_body(&added)["sources"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        (addr, id)
    }

    #[tokio::test]
    async fn notes_check_their_citations_against_the_sources() {
        let (addr, _) = history_notebook("notes").await;
        let t = Some("right-token");
        let body = r#"{"title":"Peace","body":"It ended in 1648 [[A0|was signed in 1648 and ended]], not in 1650 [[A0|the war went on until 1650]]."}"#;
        let created = json_body(&call(addr, "POST", "/api/notebooks/history/notes", t, body).await);
        let verified: Vec<bool> = created["citations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["verified"].as_bool().unwrap())
            .collect();
        assert_eq!(verified, [true, false]);
        let list = json_body(&call(addr, "GET", "/api/notebooks/history/notes", t, "").await);
        assert_eq!(list[0]["title"], "Peace");
        let bad = call(
            addr,
            "GET",
            "/api/notebooks/history/notes/..%2Fnotebook",
            t,
            "",
        )
        .await;
        assert!(bad.starts_with("HTTP/1.1 404"), "{bad}");
    }

    #[tokio::test]
    async fn asking_with_every_source_switched_off_is_refused() {
        let (addr, id) = history_notebook("excluded").await;
        let t = Some("right-token");
        let path = format!("/api/notebooks/history/sources/{id}");
        let off = json_body(&call(addr, "PATCH", &path, t, r#"{"excluded":true}"#).await);
        assert_eq!(off["sources"][0]["excluded"], true);
        let asked = call(
            addr,
            "POST",
            "/api/notebooks/history/ask",
            t,
            r#"{"question":"When was the peace signed?"}"#,
        )
        .await;
        assert!(
            asked.starts_with("HTTP/1.1 400") && asked.contains("switched off"),
            "{asked}"
        );
    }

    #[tokio::test]
    async fn custom_reports_need_a_description_before_any_model_call() {
        let (addr, _) = history_notebook("report").await;
        let response = call(
            addr,
            "POST",
            "/api/notebooks/history/reports",
            Some("right-token"),
            r#"{"kind":"custom","instructions":"  "}"#,
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 400") && response.contains("Describe the report"),
            "{response}"
        );
    }

    #[tokio::test]
    async fn a_link_is_added_even_if_the_reader_leaves_first() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let site = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/westphalia.html", site.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut conn, _) = site.accept().await.unwrap();
            let mut request = [0; 2048];
            let _ = conn.read(&mut request).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let html = "<!doctype html><title>Peace of Westphalia</title><h1>Peace of Westphalia</h1>\
                <p>The Peace of Westphalia is the collective name for two peace treaties signed in \
                October 1648 in the Westphalian cities of Osnabrück and Münster. They ended the \
                Thirty Years' War and brought peace to the Holy Roman Empire.</p>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            let _ = conn.write_all(response.as_bytes()).await;
        });
        let addr = serve("leave").await;
        let t = Some("right-token");
        call(addr, "POST", "/api/notebooks", t, r#"{"title":"History"}"#).await;
        let body = format!(r#"{{"url":"{url}"}}"#);
        let add = call(addr, "POST", "/api/notebooks/history/sources/url", t, &body);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), add)
                .await
                .is_err(),
            "the site answers only after the reader leaves"
        );
        for _ in 0..50 {
            let notebook = json_body(&call(addr, "GET", "/api/notebooks/history", t, "").await);
            if notebook["sources"].as_array().is_some_and(|s| s.len() == 1) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the link was never added");
    }

    #[tokio::test]
    async fn settings_take_a_search_backend_and_search_keys() {
        let addr = serve("search-settings").await;
        let body = r#"{"search":{"backend":"searxng","searxng_url":"http://localhost:8888"},"keys":{"TAVILY_API_KEY":"tvly-secret"}}"#;
        let saved = call(addr, "PUT", "/api/settings", Some("right-token"), body).await;
        let view = json_body(&saved);
        assert_eq!(view["search"]["backend"], "searxng");
        assert_eq!(view["search"]["searxng_url"], "http://localhost:8888");
        let tavily = view["search_keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["key_env"] == "TAVILY_API_KEY")
            .unwrap();
        assert_eq!(tavily["saved"], true);
        assert!(!saved.contains("tvly-secret"));
    }
}
