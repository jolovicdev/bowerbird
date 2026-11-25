//! Tests that drive the server over HTTP, as the UI does, and the state
//! other modules' tests build on.

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
    serve_with(name, true).await
}

async fn serve_with(name: &str, private_links: bool) -> std::net::SocketAddr {
    let dir = std::env::temp_dir().join(format!("bowerbird-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = router(Config {
        library: dir.join("library"),
        token: "right-token".into(),
        ui_dir: None,
        settings_path: dir.join("settings.toml"),
        settings: Settings::default(),
        private_links,
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
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    send(addr, method, path, &auth, body).await
}

/// `headers` are whole lines, each ending in CRLF.
async fn send(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
async fn a_sign_in_cookie_opens_the_api_without_the_token() {
    let addr = serve("sign-in").await;
    let wrong = call(addr, "POST", "/api/login", None, r#"{"password":"guess"}"#).await;
    assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
    let right = call(
        addr,
        "POST",
        "/api/login",
        None,
        r#"{"password":"right-token"}"#,
    )
    .await;
    let cookie = right
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: "))
        .unwrap();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(!cookie.contains("right-token"), "{cookie}");
    let session = cookie.split(';').next().unwrap();
    let read = send(
        addr,
        "GET",
        "/api/notebooks",
        &format!("Cookie: {session}\r\n"),
        "",
    )
    .await;
    assert!(read.starts_with("HTTP/1.1 200"), "{read}");
    let forged = "Cookie: bowerbird_session=right-token\r\n";
    let refused = send(addr, "GET", "/api/notebooks", forged, "").await;
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
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
    let body = r#"{"chat_model":"claude-sonnet-5-5","thinking":"low","keys":{"ANTHROPIC_API_KEY":"sk-test-secret"}}"#;
    let saved = call(addr, "PUT", "/api/settings", Some("right-token"), body).await;
    assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
    let read = call(addr, "GET", "/api/settings", Some("right-token"), "").await;
    assert!(read.contains(r#""chat_model":"claude-sonnet-5-5""#));
    assert!(read.contains(r#""thinking":"low""#), "{read}");
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
async fn the_model_test_says_what_is_missing() {
    let addr = serve("model-test").await;
    let t = Some("right-token");
    assert!(
        call(addr, "POST", "/api/settings/test", None, "")
            .await
            .contains("401")
    );
    let body = r#"{"chat_model":"Heabsy:some-model"}"#;
    call(addr, "PUT", "/api/settings", t, body).await;
    let tested = call(addr, "POST", "/api/settings/test", t, "").await;
    assert!(tested.starts_with("HTTP/1.1 502"), "{tested}");
    assert_eq!(
        json_body(&tested)["error"],
        "No API key for Heabsy. Add one in Settings."
    );
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

/// A server's state with an empty library, for handlers called directly,
/// and the library's path.
pub(crate) fn test_state(name: &str) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("bowerbird-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    let state = AppState {
        library: Arc::new(library.clone()),
        token: "right-token".into(),
        session: session_for("right-token").into(),
        sign_ins: Arc::default(),
        locks: Arc::default(),
        settings: Arc::new(RwLock::new(Settings::default())),
        settings_path: Arc::new(dir.join("settings.toml")),
        jobs: Arc::default(),
        speech: Arc::default(),
        embedding: Arc::default(),
        reach: web::Reach::Any,
    };
    (state, library)
}

#[tokio::test]
async fn a_copied_notebook_folder_opens_under_its_own_name() {
    let addr = serve("copies").await;
    let t = Some("right-token");
    call(addr, "POST", "/api/notebooks", t, r#"{"title":"Birds"}"#).await;
    let library = std::env::temp_dir()
        .join(format!("bowerbird-copies-{}", std::process::id()))
        .join("library");
    for copy in ["birds copy", "birds - Copy (2)"] {
        for sub in ["sources", "notes", "chats", ".bowerbird"] {
            std::fs::create_dir_all(library.join(copy).join(sub)).unwrap();
        }
        std::fs::copy(
            library.join("birds/notebook.toml"),
            library.join(copy).join("notebook.toml"),
        )
        .unwrap();
    }
    let listed = json_body(&call(addr, "GET", "/api/notebooks", t, "").await);
    assert_eq!(listed.as_array().unwrap().len(), 3);
    for path in ["birds%20copy", "birds%20-%20Copy%20(2)"] {
        let opened = call(addr, "GET", &format!("/api/notebooks/{path}"), t, "").await;
        assert!(opened.starts_with("HTTP/1.1 200"), "{path}: {opened}");
    }
    for path in ["..%2Fbirds", "%2E%2E", "birds%2F..%2Fbirds", "..%5Cbirds"] {
        let refused = call(addr, "GET", &format!("/api/notebooks/{path}"), t, "").await;
        assert!(refused.starts_with("HTTP/1.1 404"), "{path}: {refused}");
    }
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
async fn notebook_and_library_search_find_passages_and_notes() {
    let (addr, _) = history_notebook("search-api").await;
    let t = Some("right-token");
    call(
        addr,
        "POST",
        "/api/notebooks/history/notes",
        t,
        r#"{"title":"Diplomacy","body":"Westphalia reshaped European diplomacy."}"#,
    )
    .await;
    let nb_search = json_body(
        &call(
            addr,
            "GET",
            "/api/notebooks/history/search?q=Westphalia",
            t,
            "",
        )
        .await,
    );
    assert_eq!(nb_search["passages"].as_array().unwrap().len(), 1);
    assert_eq!(nb_search["notes"].as_array().unwrap().len(), 1);
    let lib_search = json_body(&call(addr, "GET", "/api/search?q=Westphalia", t, "").await);
    assert_eq!(lib_search["passages"].as_array().unwrap().len(), 1);
    assert_eq!(lib_search["notes"].as_array().unwrap().len(), 1);
    assert_eq!(lib_search["notebooks"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn settings_persist_profiles_helper_model_and_ocr_script() {
    let addr = serve("settings-extra").await;
    let t = Some("right-token");
    let body = r#"{"chat_model":"open_router::nvidia/nemotron-3.5-lightning:free","helper_model":"deepseek-flash","thinking":"low","ocr_script":"cyrillic","profiles":[{"name":"Fast","model":"deepseek-flash","thinking":"off"},{"name":"Plain","model":"deepseek-flash"}]}"#;
    let saved = call(addr, "PUT", "/api/settings", t, body).await;
    assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
    let view = json_body(&call(addr, "GET", "/api/settings", t, "").await);
    assert_eq!(view["helper_model"], "deepseek-flash");
    assert_eq!(view["ocr_script"], "cyrillic");
    assert_eq!(view["profiles"][0]["name"], "Fast");
    assert_eq!(view["profiles"][0]["thinking"], "off");
    assert_eq!(view["profiles"][1]["thinking"], "default");
}

#[tokio::test]
async fn bibtex_and_notebook_zip_exports_return_valid_downloads() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (addr, _) = history_notebook("exports-api").await;
    let t = Some("right-token");
    let bib = call(addr, "GET", "/api/notebooks/history/bibtex", t, "").await;
    assert!(bib.starts_with("HTTP/1.1 200"), "{bib}");
    assert!(bib.contains("title = {Westphalia}"), "{bib}");

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            b"GET /api/notebooks/history/export HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer right-token\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP header separator");
    let head = String::from_utf8_lossy(&raw[..split]);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/zip"),
        "{head}"
    );
    assert!(
        raw[split + 4..].starts_with(b"PK\x03\x04"),
        "response body should be a valid zip archive"
    );
    let cache = std::env::temp_dir()
        .join(format!("bowerbird-exports-api-{}", std::process::id()))
        .join("library/history/.bowerbird");
    let left_behind: Vec<_> = std::fs::read_dir(cache)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(left_behind.is_empty(), "{left_behind:?}");
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
async fn links_reach_this_machine_only_when_private_links_are_allowed() {
    let addr = serve_with("public-only", false).await;
    let t = Some("right-token");
    call(addr, "POST", "/api/notebooks", t, r#"{"title":"History"}"#).await;
    let body = r#"{"url":"http://127.0.0.1:9/westphalia.html"}"#;
    let added = call(addr, "POST", "/api/notebooks/history/sources/url", t, body).await;
    assert!(
        added.starts_with("HTTP/1.1 502") && added.contains("--allow-private-links"),
        "{added}"
    );
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
