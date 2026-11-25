//! Web search, for finding new sources. SearXNG, run by the user, is the
//! recommended backend. DuckDuckGo's HTML page needs no setup but may refuse
//! automated searches. Brave, Tavily, and Exa take the user's API key.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::settings::Settings;
use crate::{Error, Result, ask};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    DuckDuckGo,
    SearXng,
    Brave,
    Tavily,
    Exa,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchSettings {
    #[serde(default)]
    pub backend: Backend,
    /// Base address of the user's SearXNG, such as `http://localhost:8888`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searxng_url: Option<String>,
}

/// Search services that take a key, as (name, key variable).
pub const KEYS: [(&str, &str); 3] = [
    ("Brave Search", "BRAVE_SEARCH_API_KEY"),
    ("Tavily", "TAVILY_API_KEY"),
    ("Exa", "EXA_API_KEY"),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

const TIMEOUT: Duration = Duration::from_secs(15);
/// Results kept from each query.
pub const PER_QUERY: usize = 6;

const QUERIES_SYSTEM: &str = "You write web search queries that find good sources for the user's research question.
Reply with JSON only, in the form {\"queries\": [\"…\", \"…\", \"…\"]}.
Write three queries that approach the question from different sides, each specific enough to find in-depth articles, papers, or reference pages rather than news snippets. Write them in the language of the question.";

/// Three search queries for a research question, written by the model.
pub async fn queries(client: &genai::Client, model: &str, question: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Queries {
        queries: Vec<String>,
    }
    let reply = ask::complete(client, model, QUERIES_SYSTEM, question.to_string()).await?;
    let parsed = ask::json_object(&reply)
        .and_then(|json| serde_json::from_str::<Queries>(json).ok())
        .map(|q| q.queries)
        .unwrap_or_default();
    let queries: Vec<String> = parsed
        .into_iter()
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .take(5)
        .collect();
    Ok(if queries.is_empty() {
        vec![question.to_string()]
    } else {
        queries
    })
}

fn client() -> Result<reqwest::Client> {
    crate::http::client()
        .timeout(TIMEOUT)
        .user_agent(concat!(
            "Mozilla/5.0 (compatible; Bowerbird/",
            env!("CARGO_PKG_VERSION"),
            ")"
        ))
        .build()
        .map_err(|e| Error::Http(e.to_string()))
}

/// Searches with the backend chosen in Settings.
pub async fn search(settings: &Settings, query: &str) -> Result<Vec<Hit>> {
    let http = client()?;
    let key = |env: &str, name: &str| {
        settings
            .key(env)
            .ok_or_else(|| Error::Invalid(format!("No API key for {name}. Add one in Settings.")))
    };
    let hits = match settings.search.backend {
        Backend::DuckDuckGo => duckduckgo(&http, query).await?,
        Backend::SearXng => {
            let base = settings
                .search
                .searxng_url
                .as_deref()
                .filter(|u| !u.trim().is_empty())
                .ok_or_else(|| {
                    Error::Invalid("Add your SearXNG address in Settings first.".into())
                })?;
            let url = url(
                &format!("{}/search", base.trim().trim_end_matches('/')),
                &[("q", query), ("format", "json")],
            )?;
            parse_searxng(&text(|| http.get(url.clone()), "SearXNG").await?)
        }
        Backend::Brave => {
            let token = key("BRAVE_SEARCH_API_KEY", "Brave Search")?;
            let url = url(
                "https://api.search.brave.com/res/v1/web/search",
                &[("q", query)],
            )?;
            parse_brave(
                &text(
                    || http.get(url.clone()).header("X-Subscription-Token", &token),
                    "Brave Search",
                )
                .await?,
            )
        }
        Backend::Tavily => {
            let token = key("TAVILY_API_KEY", "Tavily")?;
            let body = serde_json::json!({ "query": query, "max_results": PER_QUERY });
            parse_tavily(
                &text(
                    || {
                        http.post("https://api.tavily.com/search")
                            .bearer_auth(&token)
                            .header(reqwest::header::CONTENT_TYPE, "application/json")
                            .body(body.to_string())
                    },
                    "Tavily",
                )
                .await?,
            )
        }
        Backend::Exa => {
            let token = key("EXA_API_KEY", "Exa")?;
            let body = serde_json::json!({ "query": query, "numResults": PER_QUERY });
            parse_exa(
                &text(
                    || {
                        http.post("https://api.exa.ai/search")
                            .header("x-api-key", &token)
                            .header(reqwest::header::CONTENT_TYPE, "application/json")
                            .body(body.to_string())
                    },
                    "Exa",
                )
                .await?,
            )
        }
    };
    Ok(hits.into_iter().take(PER_QUERY).collect())
}

fn url(base: &str, params: &[(&str, &str)]) -> Result<reqwest::Url> {
    reqwest::Url::parse_with_params(base, params)
        .map_err(|e| Error::Invalid(format!("{base} isn't a usable address: {e}")))
}

async fn text(request: impl Fn() -> reqwest::RequestBuilder, service: &str) -> Result<String> {
    let response = crate::retry::with_retries(
        || request().send(),
        |e: &reqwest::Error| crate::retry::transient_status(e.status().map(|s| s.as_u16())),
    )
    .await
    .map_err(|e| {
        Error::Http(if e.is_timeout() {
            format!("{service} took too long to answer.")
        } else {
            format!(
                "Bowerbird couldn't reach {service}. Check the address and your internet connection."
            )
        })
    })?;
    let status = response.status();
    match status.as_u16() {
        401 | 403 if service == "SearXNG" => {
            return Err(Error::Http(
                "Your SearXNG doesn't allow JSON results. Add json under search: formats in its settings.yml."
                    .into(),
            ));
        }
        401 | 403 => {
            return Err(Error::Http(format!(
                "{service} rejected the API key. Check it in Settings."
            )));
        }
        429 => {
            return Err(Error::Http(format!(
                "{service} is limiting searches right now. Wait a minute, then try again."
            )));
        }
        _ if !status.is_success() => {
            return Err(Error::Http(format!(
                "{service} answered with an error ({status})."
            )));
        }
        _ => {}
    }
    response
        .text()
        .await
        .map_err(|e| Error::Http(format!("{service} broke off its answer: {e}")))
}

fn hits(value: Option<&serde_json::Value>, title: &str, url: &str, snippet: &str) -> Vec<Hit> {
    let field = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .map(|item| Hit {
            title: field(item, title),
            url: field(item, url),
            snippet: field(item, snippet),
        })
        .filter(|h| h.url.starts_with("http"))
        .collect()
}

fn json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_default()
}

fn parse_searxng(raw: &str) -> Vec<Hit> {
    hits(json(raw).get("results"), "title", "url", "content")
}

fn parse_brave(raw: &str) -> Vec<Hit> {
    hits(
        json(raw).get("web").and_then(|w| w.get("results")),
        "title",
        "url",
        "description",
    )
}

fn parse_tavily(raw: &str) -> Vec<Hit> {
    hits(json(raw).get("results"), "title", "url", "content")
}

fn parse_exa(raw: &str) -> Vec<Hit> {
    hits(json(raw).get("results"), "title", "url", "text")
}

/// Reads the results out of a page; None when it finds none.
type PageReader = fn(&str) -> Option<Vec<Hit>>;

/// DuckDuckGo's two pages of plain results, each with the function that
/// reads it. They are laid out differently and change at different times,
/// so the second is read when the first has no results Bowerbird can find.
const DUCKDUCKGO_PAGES: [(&str, PageReader); 2] = [
    ("https://html.duckduckgo.com/html/", parse_duckduckgo),
    ("https://lite.duckduckgo.com/lite/", parse_duckduckgo_lite),
];

/// What DuckDuckGo's page for a search it won't run has all over it.
const DUCKDUCKGO_REFUSAL: &str = "anomaly-modal";

async fn duckduckgo(http: &reqwest::Client, query: &str) -> Result<Vec<Hit>> {
    for (address, parse) in DUCKDUCKGO_PAGES {
        let url = url(address, &[("q", query)])?;
        let page = text(|| http.get(url.clone()), "DuckDuckGo").await?;
        if let Some(hits) = parse(&page) {
            return Ok(hits);
        }
        // A refusal covers both pages, and asking again only extends it.
        if page.contains(DUCKDUCKGO_REFUSAL) {
            return Err(Error::Http(
                "DuckDuckGo is refusing automated searches right now. Set up SearXNG or add a search API key in Settings."
                    .into(),
            ));
        }
    }
    Err(Error::Http(
        "Bowerbird couldn't read DuckDuckGo's results. Its pages may have changed: look for a newer Bowerbird, or set up SearXNG or add a search API key in Settings."
            .into(),
    ))
}

/// Results from DuckDuckGo's HTML page; None when Bowerbird finds none in it.
fn parse_duckduckgo(page: &str) -> Option<Vec<Hit>> {
    let hits: Vec<Hit> = page
        .split("class=\"result__a\"")
        .skip(1)
        .filter_map(|block| {
            let snippet = block
                .split_once("class=\"result__snippet\"")
                .map(|(_, rest)| inner_text(rest, "</a>"))
                .unwrap_or_default();
            duckduckgo_hit(
                &attribute(block, "href")?,
                inner_text(block, "</a>"),
                snippet,
            )
        })
        .collect();
    (!hits.is_empty()).then_some(hits)
}

/// Results from DuckDuckGo's lite page, a table with a `result-link` anchor
/// in one row and a `result-snippet` cell in the next.
fn parse_duckduckgo_lite(page: &str) -> Option<Vec<Hit>> {
    let hits: Vec<Hit> = page
        // The class comes last in these anchors, so each is read from its start.
        .split("<a ")
        .skip(1)
        .filter(|anchor| {
            anchor
                .split('>')
                .next()
                .is_some_and(|tag| tag.contains("result-link"))
        })
        .filter_map(|anchor| {
            let snippet = anchor
                .split_once("result-snippet")
                .map(|(_, rest)| inner_text(rest, "</td>"))
                .unwrap_or_default();
            duckduckgo_hit(
                &attribute(anchor, "href")?,
                inner_text(anchor, "</a>"),
                snippet,
            )
        })
        .collect();
    (!hits.is_empty()).then_some(hits)
}

/// A result, with its address taken out of DuckDuckGo's redirect, which
/// carries the target in `uddg`.
fn duckduckgo_hit(href: &str, title: String, snippet: String) -> Option<Hit> {
    let url = decode_entities(href);
    let url = reqwest::Url::parse(&format!("https:{}", url.trim_start_matches("https:")))
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "uddg")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or(url);
    (url.starts_with("http") && !title.is_empty()).then_some(Hit {
        title,
        url,
        snippet,
    })
}

/// The value of `name="…"` in the tag that `rest` starts inside.
fn attribute(rest: &str, name: &str) -> Option<String> {
    let tag = &rest[..rest.find('>')?];
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(tag[start..end].to_string())
}

/// The text between the end of the current tag and its closing tag.
fn inner_text(rest: &str, closing: &str) -> String {
    let Some(open) = rest.find('>') else {
        return String::new();
    };
    let body = &rest[open + 1..];
    let body = &body[..body.find(closing).unwrap_or(body.len())];
    let mut text = String::new();
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    decode_entities(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
    )
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Results of several queries in rank order, each address once.
pub fn merge(lists: Vec<Vec<Hit>>) -> Vec<Hit> {
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    let mut out: Vec<Hit> = Vec::new();
    for rank in 0..longest {
        for list in &lists {
            if let Some(hit) = list.get(rank) {
                let key = hit
                    .url
                    .split('#')
                    .next()
                    .unwrap_or(&hit.url)
                    .trim_end_matches('/');
                if !out.iter().any(|h| {
                    h.url
                        .split('#')
                        .next()
                        .unwrap_or(&h.url)
                        .trim_end_matches('/')
                        == key
                }) {
                    out.push(hit.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_results_lose_the_redirect_and_entities() {
        let page = r#"<div class="result"><h2><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FPeace_of_Westphalia&amp;rut=abc">Peace of <b>Westphalia</b> - Wikipedia</a></h2>
            <a class="result__snippet" href="x">The treaties of <b>1648</b> ended the Thirty Years&#x27; War &amp; more.</a></div>
            <div class="result"><a class="result__a" href="https://example.org/direct">Direct link</a></div>"#;
        let hits = parse_duckduckgo(page).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].url,
            "https://en.wikipedia.org/wiki/Peace_of_Westphalia"
        );
        assert_eq!(hits[0].title, "Peace of Westphalia - Wikipedia");
        assert_eq!(
            hits[0].snippet,
            "The treaties of 1648 ended the Thirty Years' War & more."
        );
        assert_eq!(hits[1].url, "https://example.org/direct");
        assert!(parse_duckduckgo("<html>If this persists, please email us.</html>").is_none());
    }

    #[test]
    fn duckduckgo_s_lite_page_gives_the_same_results_from_its_table() {
        // Two results cut from the lite page as DuckDuckGo served it.
        let page = r#"<tr><td valign="top">1.&nbsp;</td><td>
            <a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FPeace_of_Westphalia&amp;rut=e9a4" class='result-link'>Peace of Westphalia - Wikipedia</a></td></tr>
            <tr><td>&nbsp;&nbsp;&nbsp;</td><td class='result-snippet'>
              The <b>Peace</b> <b>of</b> <b>Westphalia</b> was a series of treaties that ended the Thirty Years&#x27; War in <b>1648</b>.
            </td></tr>
            <tr><td>&nbsp;&nbsp;&nbsp;</td><td><span class='link-text'>en.wikipedia.org/wiki/Peace_of_Westphalia</span></td></tr>
            <tr><td valign="top">2.&nbsp;</td><td>
            <a rel="nofollow" href="https://example.org/direct" class='result-link'>Direct link</a></td></tr>
            <tr><td><a href="/lite/?q=next">Next page</a></td></tr>"#;
        let hits = parse_duckduckgo_lite(page).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(
            hits[0].url,
            "https://en.wikipedia.org/wiki/Peace_of_Westphalia"
        );
        assert_eq!(hits[0].title, "Peace of Westphalia - Wikipedia");
        assert_eq!(
            hits[0].snippet,
            "The Peace of Westphalia was a series of treaties that ended the Thirty Years' War in 1648."
        );
        assert_eq!(hits[1].url, "https://example.org/direct");
        assert_eq!(hits[1].snippet, "");
        assert!(parse_duckduckgo_lite(r#"<a href="/x">No results here</a>"#).is_none());
    }

    /// Calls DuckDuckGo, so it runs only with BOWERBIRD_LIVE=1. Each page is
    /// read by itself: while one still gives results, a search hides that
    /// the other has changed.
    #[tokio::test]
    async fn each_duckduckgo_page_still_gives_results() {
        if std::env::var("BOWERBIRD_LIVE").as_deref() != Ok("1") {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to call DuckDuckGo");
            return;
        }
        let http = client().unwrap();
        for (address, parse) in DUCKDUCKGO_PAGES {
            let url = url(address, &[("q", "Peace of Westphalia 1648")]).unwrap();
            // A page that didn't arrive, or a refusal, says nothing about the
            // page's layout. The workflow run shows the warning.
            let page = match text(|| http.get(url.clone()), "DuckDuckGo").await {
                Ok(page) if !page.contains(DUCKDUCKGO_REFUSAL) => page,
                Ok(_) => {
                    println!("::warning title=Skipped: {address}::It refused the search.");
                    continue;
                }
                Err(e) => {
                    println!("::warning title=Skipped: {address}::{e}");
                    continue;
                }
            };
            let hits = parse(&page).unwrap_or_default();
            assert!(hits.len() >= 3, "{address} gave {} results", hits.len());
            assert!(
                hits.iter().all(|hit| hit.url.starts_with("http")),
                "{hits:?}"
            );
        }
    }

    #[test]
    fn api_results_are_read_from_each_service_shape() {
        let searxng = r#"{"results": [{"url": "https://a.org/x", "title": "A", "content": "about a"}, {"url": "ftp://no", "title": "B"}]}"#;
        assert_eq!(
            parse_searxng(searxng),
            [Hit {
                title: "A".into(),
                url: "https://a.org/x".into(),
                snippet: "about a".into()
            }]
        );
        let brave = r#"{"web": {"results": [{"url": "https://b.org", "title": "B", "description": "about b"}]}}"#;
        assert_eq!(parse_brave(brave)[0].snippet, "about b");
        let tavily =
            r#"{"results": [{"url": "https://t.org", "title": "T", "content": "about t"}]}"#;
        assert_eq!(parse_tavily(tavily)[0].url, "https://t.org");
        let exa = r#"{"results": [{"url": "https://e.org", "title": "E"}]}"#;
        assert_eq!(parse_exa(exa)[0].title, "E");
        assert!(parse_searxng("not json").is_empty());
    }

    #[test]
    fn merged_results_alternate_queries_and_keep_each_page_once() {
        let hit = |url: &str| Hit {
            title: url.into(),
            url: url.into(),
            snippet: String::new(),
        };
        let merged = merge(vec![
            vec![hit("https://a.org/1"), hit("https://a.org/2")],
            vec![hit("https://a.org/1/"), hit("https://b.org/1#history")],
            vec![hit("https://b.org/1")],
        ]);
        let urls: Vec<&str> = merged.iter().map(|h| h.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://a.org/1", "https://b.org/1", "https://a.org/2"]
        );
    }
}
