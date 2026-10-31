//! Builds the prompt from passages, streams the model's answer, and checks
//! every citation against the passage it points to.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use genai::ModelIden;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, AuthResolver};
use serde::{Deserialize, Serialize};

use crate::cite::{self, Piece};
use crate::index::Index;
use crate::notebook::{ChatStyle, Goal, Length, Notebook};
use crate::retrieve::{self, Context, Passage, SectionRef};
use crate::retry;
use crate::settings::Settings;
use crate::{Error, Result};

pub const DEFAULT_MODEL: &str = "deepseek-flash";
const MAX_DEEP_SECTIONS: usize = 6;

pub(crate) const CITE_RULES: &str = "Cite every sentence that states something from the passages with one or more markers in the form [[A3|exact words]]. A3 is the passage label. Exact words is a quote of 3 to 15 words copied character for character from that passage.
Put markers right after the sentence they support, never inside headings, quotes, or code.";

const TUTOR: &str = "Answer as a tutor helping the user learn this material. Build up ideas step by step, explain each term where it first appears, and connect new points to what the user asked before. Where it helps, end with one short question that checks understanding or points to the next step. When the user asks for the answer to an exercise, guide them toward it rather than handing it over.";

const GUIDE_SYSTEM: &str = "You write the opening page of a research notebook: a short overview of the user's sources, using only the passages below.
Write one or two paragraphs, about 120 words in all, on what the sources cover and how they relate. Bold the three to six most important terms with **double asterisks**.
Then write exactly three questions a reader could ask next, each on its own line starting with \"? \". The questions have no citation markers.
Write in the language of the sources.";

/// Request sent with the guide's passages in place of a question.
pub const GUIDE_REQUEST: &str = "Write the overview and the three questions.";

const SEARCH_LANGUAGE_SYSTEM: &str = "You translate a question into the language of the user's sources, so their text can be searched for it. Write names and terms as the sources would. Reply with the translated question only.";

const DEEP_SYSTEM: &str = "You pick which sections of the user's sources most likely answer a question.
Reply with JSON only, in the form {\"sections\": [\"A§3\", \"B§12\"]}, listing at most 6 section ids from the outline, best first.";

/// The system prompt for answers in a notebook with this style.
pub fn system_prompt(style: &ChatStyle) -> String {
    let length = match style.length {
        Length::Shorter => "Keep answers short: a few sentences or a short list.",
        Length::Default => "Be direct and concise.",
        Length::Longer => {
            "Answer thoroughly: cover each relevant point in detail, with headings or lists where they help."
        }
    };
    let mut out = format!(
        "You answer questions using only the passages below, taken from the user's own sources.
{CITE_RULES}
If the passages don't contain the answer, say what is missing instead of guessing.
Write math in plain text with Unicode symbols, not LaTeX.
Answer in the language of the question. {length}"
    );
    match style.goal {
        Goal::Default => {}
        Goal::Tutor => out.push_str(&format!("\n{TUTOR}")),
        Goal::Custom if style.custom.trim().is_empty() => {}
        Goal::Custom => out.push_str(&format!(
            "\nThe user asked for answers written this way:\n{}\nFollow that, but always cite as described above.",
            style.custom.trim()
        )),
    }
    out
}

pub fn guide_system() -> String {
    format!("{GUIDE_SYSTEM}\n{CITE_RULES}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Citation {
    /// Passage the citation resolved to. Differs from what the model wrote
    /// when the quote was found in another passage.
    pub label: String,
    pub letter: String,
    pub source_id: Option<String>,
    pub page: Option<u32>,
    pub quote: String,
    pub verified: bool,
}

pub enum Event<'a> {
    Text(&'a str),
    Citation(&'a Citation),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timings {
    /// From the question arriving to the model request being sent.
    pub request_ms: f64,
    pub first_token_ms: Option<f64>,
    pub total_ms: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Answer {
    pub ts: u64,
    pub question: String,
    pub model: String,
    /// Model output including citation markers.
    pub text: String,
    pub citations: Vec<Citation>,
    pub timings: Timings,
    /// Answered with Deep search.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deep: bool,
}

/// Settings shared with the model client, which reads keys at request time,
/// so a key saved in Settings works without a restart.
pub type SharedSettings = Arc<RwLock<Settings>>;

/// An earlier question and its answer, sent along so follow-ups make sense.
pub struct Exchange {
    pub question: String,
    pub answer: String,
}

impl From<&Answer> for Exchange {
    fn from(answer: &Answer) -> Self {
        Exchange {
            question: answer.question.clone(),
            answer: plain_text(&answer.text),
        }
    }
}

/// Answer text without citation markers. Old labels point at passages the
/// next prompt may not include, so the model shouldn't see them.
pub fn plain_text(text: &str) -> String {
    let mut parser = cite::Parser::default();
    let mut pieces = parser.feed(text);
    pieces.extend(parser.finish());
    pieces
        .into_iter()
        .filter_map(|p| match p {
            Piece::Text(t) => Some(t),
            Piece::Cite { .. } => None,
        })
        .collect()
}

pub fn client(settings: SharedSettings) -> Result<genai::Client> {
    let resolver = AuthResolver::from_resolver_fn(move |iden: ModelIden| {
        // Providers without a key variable (local Ollama) need no key.
        let Some(env_name) = iden.adapter_kind.default_key_env_name() else {
            return Ok(None);
        };
        let settings = settings.read().unwrap_or_else(PoisonError::into_inner);
        match settings.key(env_name) {
            Some(key) => Ok(Some(AuthData::from_single(key))),
            None => Err(genai::resolver::Error::ApiKeyEnvNotFound {
                env_name: env_name.to_string(),
            }),
        }
    });
    genai::Client::builder()
        .with_auth_resolver(resolver)
        .build()
        .map_err(|e| Error::Model(e.to_string()))
}

/// The provider's display name for a model id, e.g. "DeepSeek".
pub fn provider_name(model: &str) -> &'static str {
    AdapterKind::from_model(model).map_or("the model provider", |k| k.as_str())
}

/// Turns a model-call failure into a sentence that says what went wrong and
/// where to fix it.
fn status(error: &genai::Error) -> Option<u16> {
    use genai::Error as G;
    match error {
        G::WebModelCall { webc_error, .. } | G::WebAdapterCall { webc_error, .. } => {
            webc_error.status().map(|s| s.as_u16())
        }
        G::HttpError { status, .. } => Some(status.as_u16()),
        _ => None,
    }
}

/// A model call that failed on the way, which trying again can fix.
fn transient(error: &genai::Error) -> bool {
    use genai::Error as G;
    matches!(
        error,
        G::WebModelCall { .. } | G::WebAdapterCall { .. } | G::HttpError { .. }
    ) && retry::transient_status(status(error))
}

pub(crate) fn model_error(model: &str, error: genai::Error) -> Error {
    use genai::Error as G;
    let provider = provider_name(model);
    let status = status(&error);
    let message = match (&error, status) {
        (G::Resolver { .. } | G::RequiresApiKey { .. } | G::NoAuthData { .. }, _) => {
            format!("No API key for {provider}. Add one in Settings.")
        }
        (_, Some(401 | 403)) => format!("{provider} rejected the API key. Check it in Settings."),
        (_, Some(404)) => {
            format!("{provider} has no model called “{model}”. Check the model name in Settings.")
        }
        (_, Some(429)) => {
            format!("{provider} is limiting requests right now. Wait a minute, then try again.")
        }
        (_, Some(code)) if code >= 500 => {
            format!("{provider} had a problem ({code}). Try again in a moment.")
        }
        (_, Some(code)) => format!("{provider} refused the request ({code})."),
        (G::WebModelCall { .. } | G::WebAdapterCall { .. } | G::WebStream { .. }, None) => {
            format!("Bowerbird couldn't reach {provider}. Check your internet connection.")
        }
        _ => format!("The request to {provider} failed: {error}"),
    };
    Error::Model(message)
}

/// The user message: source outline, labeled passages, then the question.
pub fn prompt(index: &Index, nb: &Notebook, ctx: &Context, question: &str) -> Result<String> {
    let mut ids: Vec<&str> = Vec::new();
    for p in &ctx.passages {
        if !ids.contains(&p.source_id.as_str()) {
            ids.push(&p.source_id);
        }
    }
    let mut out = String::from("Sources:\n");
    out.push_str(&retrieve::outline(index, nb, &ids, false)?);
    let off: Vec<String> = nb
        .sources
        .iter()
        .filter(|s| s.excluded)
        .map(|s| format!("{}. {}", s.letter, s.title))
        .collect();
    if !off.is_empty() {
        out.push_str(&format!(
            "\nThe user switched these sources off, so none of their passages are below. If the question seems to be about one of them, say that it is switched off:\n{}\n",
            off.join("\n")
        ));
    }
    out.push_str("\nPassages:\n");
    for p in &ctx.passages {
        out.push_str(&format!("\n[{}]", p.label));
        match (p.page_start, p.page_end) {
            (Some(a), Some(b)) if a != b => out.push_str(&format!(" pages {a}-{b}")),
            (Some(a), _) => out.push_str(&format!(" page {a}")),
            _ => {}
        }
        if !p.section.is_empty() {
            out.push_str(&format!(" | {}", p.section));
        }
        out.push('\n');
        out.push_str(p.text.trim());
        out.push('\n');
    }
    out.push_str(&format!("\nQuestion: {question}\n"));
    Ok(out)
}

/// Streams the answer. `started` is when the user sent the question, so the
/// timings include retrieval and prompt building.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is a separate input the caller already holds"
)]
pub async fn answer(
    client: &genai::Client,
    model: &str,
    system: &str,
    started: Instant,
    question: &str,
    prompt: String,
    ctx: &Context,
    history: &[Exchange],
    mut on: impl FnMut(Event),
) -> Result<Answer> {
    let mut messages = vec![ChatMessage::system(system)];
    for exchange in history {
        messages.push(ChatMessage::user(exchange.question.clone()));
        messages.push(ChatMessage::assistant(exchange.answer.clone()));
    }
    messages.push(ChatMessage::user(prompt));
    let request = ChatRequest::new(messages);
    let mut timings = Timings {
        request_ms: ms_since(started),
        ..Default::default()
    };
    // Retried only until the reply starts; a reply cut off midway is reported.
    let response = retry::with_retries(
        || client.exec_chat_stream(model, request.clone(), None),
        transient,
    )
    .await
    .map_err(|e| model_error(model, e))?;

    let mut stream = response.stream;
    let mut parser = cite::Parser::default();
    let mut text = String::new();
    let mut citations = Vec::new();
    while let Some(event) = stream.next().await {
        let pieces = match event.map_err(|e| model_error(model, e))? {
            ChatStreamEvent::Chunk(chunk) => {
                timings
                    .first_token_ms
                    .get_or_insert_with(|| ms_since(started));
                text.push_str(&chunk.content);
                parser.feed(&chunk.content)
            }
            ChatStreamEvent::End(_) => break,
            _ => continue,
        };
        emit(pieces, ctx, &mut citations, &mut on);
    }
    emit(parser.finish(), ctx, &mut citations, &mut on);
    timings.total_ms = ms_since(started);

    Ok(Answer {
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        question: question.to_string(),
        model: model.to_string(),
        text,
        citations,
        timings,
        deep: false,
    })
}

fn emit(
    pieces: Vec<Piece>,
    ctx: &Context,
    citations: &mut Vec<Citation>,
    on: &mut impl FnMut(Event),
) {
    for piece in pieces {
        match piece {
            Piece::Text(t) => on(Event::Text(&t)),
            Piece::Cite { label, quote } => {
                let citation = resolve(&label, quote, &ctx.passages);
                on(Event::Citation(&citation));
                citations.push(citation);
            }
        }
    }
}

/// Checks the quote against the cited passage first, then the same source's
/// other passages, then every passage.
pub fn resolve(label: &str, quote: String, passages: &[Passage]) -> Citation {
    let letter = cite::split_label(label).map_or("", |(l, _)| l).to_string();
    let cited = passages.iter().find(|p| p.label == label);
    let same_source = passages
        .iter()
        .filter(|p| cited.is_some_and(|c| c.source_id == p.source_id));
    let found = cited
        .into_iter()
        .chain(same_source)
        .chain(passages.iter())
        .find(|p| cite::verify(&quote, &p.text));
    match found {
        Some(p) => Citation {
            label: p.label.clone(),
            letter: cite::split_label(&p.label)
                .map_or("", |(l, _)| l)
                .to_string(),
            source_id: Some(p.source_id.clone()),
            page: p.page_start,
            quote,
            verified: true,
        },
        None => Citation {
            label: label.to_string(),
            letter,
            source_id: cited.map(|p| p.source_id.clone()),
            page: cited.and_then(|p| p.page_start),
            quote,
            verified: false,
        },
    }
}

/// The notebook guide: an overview with citations, and questions to start with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guide {
    /// Ids of the sources it was written from, so a changed set can be noticed.
    pub sources: Vec<String>,
    pub model: String,
    /// Overview including citation markers.
    pub text: String,
    pub citations: Vec<Citation>,
    pub questions: Vec<String>,
}

/// Splits the guide's question lines from its overview and checks the
/// overview's citations again, since question lines may have held some.
pub fn guide_from(answer: Answer, sources: Vec<String>, ctx: &Context) -> Guide {
    let mut text = String::new();
    let mut questions = Vec::new();
    for line in answer.text.lines() {
        match line.trim_start().strip_prefix("? ") {
            Some(q) if !q.trim().is_empty() => questions.push(plain_text(q).trim().to_string()),
            _ => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    let mut parser = cite::Parser::default();
    let mut pieces = parser.feed(&text);
    pieces.extend(parser.finish());
    let citations = pieces
        .into_iter()
        .filter_map(|p| match p {
            Piece::Cite { label, quote } => Some(resolve(&label, quote, &ctx.passages)),
            Piece::Text(_) => None,
        })
        .collect();
    Guide {
        sources,
        model: answer.model,
        text: text.trim().to_string(),
        citations,
        questions,
    }
}

/// One request and the model's whole reply, for replies read as data.
pub(crate) async fn complete(
    client: &genai::Client,
    model: &str,
    system: &str,
    user: String,
) -> Result<String> {
    let request = ChatRequest::new(vec![ChatMessage::system(system), ChatMessage::user(user)]);
    let response =
        retry::with_retries(|| client.exec_chat(model, request.clone(), None), transient)
            .await
            .map_err(|e| model_error(model, e))?;
    Ok(response.first_text().unwrap_or_default().to_string())
}

/// The question in the language of the sources whose openings are in
/// `sample` (from `retrieve::foreign_question`), for searching them.
pub async fn translate_for_search(
    client: &genai::Client,
    model: &str,
    question: &str,
    sample: &str,
) -> Result<String> {
    let reply = complete(
        client,
        model,
        SEARCH_LANGUAGE_SYSTEM,
        format!("The sources begin:\n{sample}\n\nQuestion: {question}"),
    )
    .await?;
    Ok(reply.trim().to_string())
}

/// Deep search: the model reads every source's outline and names the
/// sections to read. `outline` comes from `retrieve::outline` with section ids.
pub async fn pick_sections(
    client: &genai::Client,
    model: &str,
    nb: &Notebook,
    outline: &str,
    question: &str,
) -> Result<Vec<SectionRef>> {
    let reply = complete(
        client,
        model,
        DEEP_SYSTEM,
        format!("Outline:\n{outline}\nQuestion: {question}"),
    )
    .await?;
    Ok(parse_picks(nb, &reply))
}

#[derive(Deserialize)]
struct Picks {
    sections: Vec<String>,
}

/// The outermost JSON object in a reply, which models often wrap in prose
/// or a code fence.
pub(crate) fn json_object(reply: &str) -> Option<&str> {
    match (reply.find('{'), reply.rfind('}')) {
        (Some(a), Some(b)) if a < b => Some(&reply[a..=b]),
        _ => None,
    }
}

fn parse_picks(nb: &Notebook, reply: &str) -> Vec<SectionRef> {
    let Some(json) = json_object(reply) else {
        return Vec::new();
    };
    let Ok(picks) = serde_json::from_str::<Picks>(json) else {
        return Vec::new();
    };
    picks
        .sections
        .iter()
        .filter_map(|id| retrieve::parse_section_id(nb, id))
        .take(MAX_DEEP_SECTIONS)
        .collect()
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage(label: &str, source: &str, text: &str) -> Passage {
        Passage {
            label: label.into(),
            source_id: source.into(),
            ord: cite::split_label(label).unwrap().1,
            page_start: Some(1),
            page_end: Some(1),
            section: String::new(),
            text: text.into(),
        }
    }

    #[test]
    fn resolve_moves_a_misattributed_quote_to_the_passage_that_has_it() {
        let passages = vec![
            passage("A1", "a", "Satin bowerbirds prefer blue objects."),
            passage("A2", "a", "Females inspect the bower before mating."),
            passage("B0", "b", "The Vogelkop bowerbird sorts finds by color."),
        ];
        let c = resolve("A1", "sorts finds by color".into(), &passages);
        assert!(c.verified);
        assert_eq!((c.label.as_str(), c.letter.as_str()), ("B0", "B"));

        let c = resolve("A1", "males paint the bower with berries".into(), &passages);
        assert!(!c.verified);
        assert_eq!(c.label, "A1");
    }

    #[test]
    fn answer_style_reaches_the_system_prompt() {
        let plain = system_prompt(&ChatStyle::default());
        assert!(plain.contains("Be direct and concise.") && !plain.contains("tutor"));
        let tutor = system_prompt(&ChatStyle {
            goal: Goal::Tutor,
            length: Length::Longer,
            ..Default::default()
        });
        assert!(tutor.contains("as a tutor") && tutor.contains("Answer thoroughly"));
        let custom = system_prompt(&ChatStyle {
            goal: Goal::Custom,
            custom: "Answer as a museum guide would.".into(),
            length: Length::Shorter,
        });
        assert!(custom.contains("museum guide") && custom.contains("Keep answers short"));
        assert!(custom.contains("[[A3|exact words]]"), "citation rules stay");
    }

    #[test]
    fn guide_questions_are_split_from_the_overview() {
        let ctx = Context {
            hits: Vec::new(),
            passages: vec![passage(
                "A0",
                "a",
                "The Peace of Westphalia ended the Thirty Years' War.",
            )],
        };
        let text = "The sources cover **Westphalia** [[A0|ended the Thirty Years' War]].\n\n\
                    ? Why did the war last thirty years [[A0|Peace of Westphalia ended]]\n\
                    ? Who signed the peace?\n? What changed after 1648?";
        let answer = Answer {
            ts: 0,
            question: String::new(),
            model: "m".into(),
            text: text.into(),
            citations: Vec::new(),
            timings: Timings::default(),
            deep: false,
        };
        let guide = guide_from(answer, vec!["a".into()], &ctx);
        assert_eq!(guide.questions.len(), 3);
        assert_eq!(guide.questions[0], "Why did the war last thirty years");
        assert!(!guide.text.contains('?'));
        assert_eq!(guide.citations.len(), 1);
        assert!(guide.citations[0].verified);
    }

    #[test]
    fn the_prompt_names_switched_off_sources() {
        let dir = std::env::temp_dir().join(format!("bowerbird-prompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        nb.add_text("Westphalia", "The peace was signed in 1648.")
            .unwrap();
        let notes = nb
            .add_text("Lecture notes", "Revise the treaty terms.")
            .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let ctx = retrieve::gather_overview(&index, &nb).unwrap();
        assert!(
            !prompt(&index, &nb, &ctx, "What is the exam tip?")
                .unwrap()
                .contains("switched these sources off")
        );
        nb.set_excluded(&notes.id, true).unwrap();
        let ctx = retrieve::gather_overview(&index, &nb).unwrap();
        let text = prompt(&index, &nb, &ctx, "What is the exam tip?").unwrap();
        assert!(text.contains("switched these sources off") && text.contains("B. Lecture notes"));
    }
}
