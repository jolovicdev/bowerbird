//! Live checks against the services Bowerbird depends on: the default model,
//! YouTube's caption clients, web page extraction, and DuckDuckGo. They run
//! only with BOWERBIRD_LIVE=1. Reads DEEPSEEK_API_KEY from the environment,
//! else from the workspace .env.

use std::time::Instant;

use bowerbird_core::ask::{self, DEFAULT_MODEL};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::Notebook;
use bowerbird_core::settings::Settings;
use bowerbird_core::{retrieve, search, web, youtube};
use genai::resolver::{AuthData, AuthResolver};

const KEY: &str = "DEEPSEEK_API_KEY";

fn key() -> String {
    if let Ok(key) = std::env::var(KEY) {
        return key;
    }
    let env_file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env");
    std::fs::read_to_string(env_file)
        .ok()
        .and_then(|raw| {
            raw.lines().find_map(|l| {
                l.strip_prefix(KEY)
                    .and_then(|r| r.strip_prefix('='))
                    .map(|v| v.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| panic!("{KEY} is not set and not in .env"))
}

fn live() -> bool {
    let on = std::env::var("BOWERBIRD_LIVE").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set BOWERBIRD_LIVE=1 to call live services");
    }
    on
}

#[tokio::test]
async fn answers_with_a_verified_citation() {
    if !live() {
        return;
    }
    let key = key();
    let client = genai::Client::builder()
        .with_auth_resolver(AuthResolver::from_resolver_fn(move |_| {
            Ok(Some(AuthData::from_single(key.clone())))
        }))
        .build()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("bowerbird-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut nb = Notebook::open_or_create(&dir, "Live").unwrap();
    nb.add_text(
        "Vogelkop bowerbird",
        "# Vogelkop bowerbird\nThe Vogelkop bowerbird builds a hut of sticks over a mat of moss.\n\
         ## Decorations\nIn front of the hut it sorts its finds into separate piles by color: \
         red fruit, black beetle wings, and blue berries each get their own pile.\n",
    )
    .unwrap();
    let (index, _) = Index::open_synced(&nb).unwrap();

    let question = "How does the Vogelkop bowerbird arrange its decorations?";
    let started = Instant::now();
    let ctx = retrieve::gather(&index, &nb, question).unwrap();
    let prompt = ask::prompt(&index, &nb, &ctx, question).unwrap();
    let answer = ask::answer(
        &client,
        DEFAULT_MODEL,
        &ask::system_prompt(&nb.chat),
        started,
        question,
        prompt,
        &ctx,
        &[],
        |_| {},
    )
    .await
    .unwrap();
    assert!(
        answer
            .citations
            .iter()
            .any(|c| c.verified && c.letter == "A"),
        "expected a verified citation of source A: {answer:#?}"
    );
}

#[tokio::test]
async fn youtube_captions_still_download() {
    if !live() {
        return;
    }
    let transcript = youtube::transcript("dQw4w9WgXcQ").await.unwrap();
    assert!(
        transcript.markdown.to_lowercase().contains("never gonna"),
        "captions came back without the lyrics: {}",
        &transcript.markdown[..transcript.markdown.len().min(300)]
    );
}

#[tokio::test]
async fn web_pages_still_extract() {
    if !live() {
        return;
    }
    let url = "https://en.wikipedia.org/wiki/Peace_of_Westphalia";
    let page = web::fetch(url, web::Reach::Public).await.unwrap();
    let (title, doc) = web::extract(&page.html().unwrap(), Some(url)).unwrap();
    assert!(title.contains("Westphalia"), "{title}");
    assert!(doc.text.contains("Osnabrück"), "article text is missing");
    assert!(!doc.sections.is_empty(), "headings were not kept");
}

#[tokio::test]
async fn duckduckgo_still_returns_results() {
    if !live() {
        return;
    }
    let hits = search::search(&Settings::default(), "Peace of Westphalia 1648")
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| h.url.starts_with("http")), "{hits:?}");
}
