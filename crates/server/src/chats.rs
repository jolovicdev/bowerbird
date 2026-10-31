//! Questions and answers: asking with streamed, cited answers, and the saved conversations.

use std::convert::Infallible;
use std::sync::PoisonError;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use bowerbird_core::ask::{self, Answer, Citation, Exchange, Timings};
use bowerbird_core::cite::{self, Piece};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::Notebook;
use bowerbird_core::retrieve::{self, Context};
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::sources::{FigureView, cited_figures};
use crate::{ApiError, ApiResult, AppState, Body, blocking, notebook_dir, open_notebook};

/// Earlier turns sent with a question, so follow-ups make sense.
const HISTORY_TURNS: usize = 3;

#[derive(Deserialize)]
pub(crate) struct Question {
    question: String,
    /// Let the model pick sections from the outline first.
    #[serde(default)]
    deep: bool,
    model: Option<String>,
    /// Conversation to continue. Without one, the question starts a new one.
    chat: Option<String>,
}

/// Streams the answer as server-sent events: `passages` (the conversation id,
/// and how many passages from which sources), `text` (a JSON string), `cite`
/// (a checked citation), then `done` (the saved turn) or `error`.
pub(crate) async fn ask_question(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
    Body(body): Body<Question>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let started = Instant::now();
    if body.question.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Type a question first.".into(),
        ));
    }
    {
        let (library, slug, chat) = (state.library.clone(), slug.clone(), body.chat.clone());
        blocking(move || {
            let nb = Notebook::open(&notebook_dir(&library, &slug)?)?;
            if let Some(chat) = &chat {
                nb.item_path("chats", chat, "jsonl")?;
            }
            usable_sources(&nb)
        })
        .await?;
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SseEvent>();
    tokio::spawn(async move {
        // The channel closes when the reader leaves. Dropping `run_ask` then
        // stops the model stream, so an answer nobody reads isn't billed to
        // the end or saved.
        let outcome = tokio::select! {
            outcome = run_ask(state, slug, body, started, tx.clone()) => outcome,
            () = tx.closed() => return,
        };
        if let Err(ApiError(_, message)) = outcome {
            send_event(&tx, "error", &serde_json::json!({ "error": message }));
        }
    });
    let events =
        futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|e| (Ok(e), rx)) },
        );
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

pub(crate) fn send_event(
    tx: &tokio::sync::mpsc::UnboundedSender<SseEvent>,
    name: &str,
    data: &(impl Serialize + ?Sized),
) {
    let _ = tx.send(
        SseEvent::default()
            .event(name)
            .data(serde_json::to_string(data).unwrap_or_default()),
    );
}

/// Refuses work that needs sources when every source is switched off.
pub(crate) fn usable_sources(nb: &Notebook) -> ApiResult<()> {
    if !nb.sources.is_empty() && nb.included().next().is_none() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Every source is switched off. Switch one on first.".into(),
        ));
    }
    Ok(())
}

/// A conversation's turns, oldest first. A line cut short by a crash
/// shouldn't hide the rest.
fn read_chat(nb: &Notebook, id: &str) -> ApiResult<Vec<Answer>> {
    match std::fs::read_to_string(nb.item_path("chats", id, "jsonl")?) {
        Ok(raw) => Ok(raw
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(bowerbird_core::Error::from(e).into()),
    }
}

async fn run_ask(
    state: AppState,
    slug: String,
    body: Question,
    started: Instant,
    tx: tokio::sync::mpsc::UnboundedSender<SseEvent>,
) -> ApiResult<()> {
    let question = body.question.trim().to_string();
    let model = body
        .model
        .unwrap_or_else(|| state.settings().chat_model().to_string());
    let client = ask::client(state.settings.clone())?;

    let (history, chat, search, foreign, deep) = {
        let (state, slug, deep, chat, question) = (
            state.clone(),
            slug.clone(),
            body.deep,
            body.chat,
            question.clone(),
        );
        blocking(move || {
            let dir = notebook_dir(&state.library, &slug)?;
            let nb = Notebook::open(&dir)?;
            let chat = chat.unwrap_or_else(|| nb.new_item_id("chats", "jsonl"));
            let log = read_chat(&nb, &chat)?;
            let history: Vec<Exchange> = log[log.len().saturating_sub(HISTORY_TURNS)..]
                .iter()
                .map(Exchange::from)
                .collect();
            let search =
                retrieve::search_text(&question, history.last().map(|e| e.question.as_str()));
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (index, _) = Index::open_synced(&nb)?;
            let foreign = retrieve::foreign_question(&index, &nb, &search)?;
            if !deep {
                return Ok((history, chat, search, foreign, None));
            }
            let ids: Vec<&str> = nb.included().map(|s| s.id.as_str()).collect();
            let outline = retrieve::outline(&index, &nb, &ids, true)?;
            Ok((history, chat, search, foreign, Some((nb, outline))))
        })
        .await?
    };
    // A question in another language than the sources is searched in theirs
    // as well; the answer still follows the question's language.
    let search = match foreign {
        Some(sample) => {
            let translated = ask::translate_for_search(&client, &model, &search, &sample).await?;
            format!("{translated} {search}")
        }
        None => search,
    };
    let picks = match &deep {
        Some((nb, outline)) => {
            Some(ask::pick_sections(&client, &model, nb, outline, &search).await?)
        }
        None => None,
    };

    let (nb, ctx, prompt) = {
        let (question, state) = (question.clone(), state.clone());
        blocking(move || -> ApiResult<(Notebook, Context, String)> {
            let lock = state.lock_for(&slug);
            let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let nb = Notebook::open(&notebook_dir(&state.library, &slug)?)?;
            let (index, _) = Index::open_synced(&nb)?;
            let mut ctx = match &picks {
                Some(picks) => retrieve::gather_with_sections(&index, &nb, &search, picks)?,
                None => retrieve::gather(&index, &nb, &search)?,
            };
            if ctx.passages.is_empty() {
                ctx = retrieve::gather_overview(&index, &nb)?;
            }
            let prompt = ask::prompt(&index, &nb, &ctx, &question)?;
            Ok((nb, ctx, prompt))
        })
        .await?
    };
    if ctx.passages.is_empty() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "This notebook has no readable text yet. Add a PDF, a link, or pasted text first."
                .into(),
        ));
    }
    let mut letters: Vec<&str> = Vec::new();
    for passage in &ctx.passages {
        if let Some(source) = nb.source(&passage.source_id)
            && !letters.contains(&source.letter.as_str())
        {
            letters.push(&source.letter);
        }
    }
    send_event(
        &tx,
        "passages",
        &serde_json::json!({ "chat": chat, "count": ctx.passages.len(), "sources": letters }),
    );

    let mut answer = ask::answer(
        &client,
        &model,
        &ask::system_prompt(&nb.chat),
        started,
        &question,
        prompt,
        &ctx,
        &history,
        |event| match event {
            ask::Event::Text(text) => send_event(&tx, "text", text),
            ask::Event::Citation(c) => send_event(&tx, "cite", c),
        },
    )
    .await?;
    answer.deep = body.deep;

    let turn = blocking(move || {
        nb.append_chat(&chat, &answer)?;
        TurnView::new(answer, &Index::open(&nb.index_path())?)
    })
    .await?;
    send_event(&tx, "done", &turn);
    Ok(())
}

/// Answer text split by the core's citation parser, so the UI never parses markers itself.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum PieceView {
    Text {
        text: String,
    },
    /// Index into `TurnView::citations`.
    Cite {
        cite: usize,
    },
}

#[derive(Serialize)]
pub(crate) struct TurnView {
    ts: u64,
    question: String,
    model: String,
    pieces: Vec<PieceView>,
    citations: Vec<Citation>,
    timings: Timings,
    deep: bool,
    /// Pictures in the passages the answer cites.
    figures: Vec<FigureView>,
}

impl TurnView {
    fn new(answer: Answer, index: &Index) -> ApiResult<TurnView> {
        let figures = cited_figures(index, &answer.citations)?;
        Ok(TurnView {
            figures,
            ..TurnView::from(answer)
        })
    }
}

/// Text with citation markers, split into text and numbered citations.
pub(crate) fn pieces(text: &str) -> Vec<PieceView> {
    let mut parser = cite::Parser::default();
    let mut pieces = parser.feed(text);
    pieces.extend(parser.finish());
    let mut next_cite = 0;
    pieces
        .into_iter()
        .map(|p| match p {
            Piece::Text(text) => PieceView::Text { text },
            Piece::Cite { .. } => {
                next_cite += 1;
                PieceView::Cite {
                    cite: next_cite - 1,
                }
            }
        })
        .collect()
}

impl From<Answer> for TurnView {
    fn from(a: Answer) -> Self {
        TurnView {
            ts: a.ts,
            question: a.question,
            model: a.model,
            pieces: pieces(&a.text),
            citations: a.citations,
            timings: a.timings,
            deep: a.deep,
            figures: Vec::new(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ChatSummary {
    id: String,
    /// The conversation's first question.
    title: String,
    turns: usize,
    updated: u64,
}

pub(crate) async fn list_chats(
    State(state): State<AppState>,
    UrlPath(slug): UrlPath<String>,
) -> ApiResult<Json<Vec<ChatSummary>>> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let mut out = Vec::new();
        for item in nb.items("chats", "jsonl")? {
            let turns = read_chat(&nb, &item.id)?;
            let Some(first) = turns.first() else {
                continue;
            };
            out.push(ChatSummary {
                title: first.question.clone(),
                turns: turns.len(),
                id: item.id,
                updated: item.updated,
            });
        }
        Ok(Json(out))
    })
    .await
}

/// A conversation's turns; none for a conversation not started yet.
pub(crate) async fn chat(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<Json<Vec<TurnView>>> {
    blocking(move || {
        let nb = open_notebook(&state, &slug)?;
        let index = Index::open(&nb.index_path())?;
        read_chat(&nb, &id)?
            .into_iter()
            .map(|answer| TurnView::new(answer, &index))
            .collect::<ApiResult<_>>()
            .map(Json)
    })
    .await
}

pub(crate) async fn delete_chat(
    State(state): State<AppState>,
    UrlPath((slug, id)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let lock = state.lock_for(&slug);
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        open_notebook(&state, &slug)?.delete_item("chats", &id, "jsonl")?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}
