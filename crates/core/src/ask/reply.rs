//! Reads a streamed reply: gives up on one that stalls, and checks each
//! citation against the passage it names as the text arrives.

use std::time::{Duration, Instant};

use futures::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::ChatStreamEvent;

use super::client::provider_name;
use super::failure::{empty_reply_error, model_error};
use super::{Citation, Event, Timings, ms_since, plain_text};
use crate::cite::{self, Piece};
use crate::retrieve::{Context, Passage};
use crate::{Error, Result};

/// How long a provider may send nothing before Bowerbird stops waiting. A
/// thinking model streams its reasoning as it goes, so a silence this long
/// means the stream died or the request never started.
pub(super) const SILENCE: Duration = Duration::from_secs(120);

/// How long a cloud model may keep thinking before writing any part of the
/// answer. Stops runaway reasoning traces that keep streaming tokens without
/// ever starting the reply.
const THINKING_LIMIT: Duration = Duration::from_secs(180);

/// Rolling tail of recent reasoning kept to spot repetition loops.
const THOUGHT_WINDOW: usize = 8 * 1024;

/// Bytes of new reasoning between two checks for a repetition loop.
const LOOP_CHECK_EVERY: usize = 64;

/// No limit for Ollama or the user's own server, which run on the user's own
/// hardware: reading a long prompt on a CPU can take minutes before the first
/// word.
pub(super) fn silence_limit(model: &str) -> Duration {
    if provider_name(model) == AdapterKind::Ollama.as_str() {
        Duration::MAX
    } else {
        SILENCE
    }
}

/// How long a model may think before starting an answer; unlimited on local hardware.
pub(super) fn thinking_limit(model: &str) -> Duration {
    if provider_name(model) == AdapterKind::Ollama.as_str() {
        Duration::MAX
    } else {
        THINKING_LIMIT
    }
}

/// Waits for the next stream event within the time left since `since`, or
/// without a deadline when `limit` is `Duration::MAX`.
pub(super) async fn next_event(
    stream: &mut genai::chat::ChatStream,
    limit: Duration,
    since: Instant,
) -> std::result::Result<Option<genai::Result<ChatStreamEvent>>, tokio::time::error::Elapsed> {
    if limit == Duration::MAX {
        Ok(stream.next().await)
    } else {
        tokio::time::timeout(limit.saturating_sub(since.elapsed()), stream.next()).await
    }
}

/// Reads a streamed reply, passing its text and citations to `on` as they
/// arrive. Gives up once the provider has sent nothing for `limit`, stops a
/// thinking model that loops or thinks past `max_thinking` without starting
/// the answer, and fails on a reply without text.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is a separate input the caller already holds"
)]
pub(super) async fn read_reply(
    stream: &mut genai::chat::ChatStream,
    model: &str,
    server: Option<&str>,
    limit: Duration,
    max_thinking: Duration,
    started: Instant,
    timings: &mut Timings,
    ctx: &Context,
    on: &mut impl FnMut(Event),
) -> Result<(String, Vec<Citation>)> {
    let mut parser = cite::Parser::default();
    let mut text = String::new();
    let mut citations = Vec::new();
    let mut stop = None;
    let provider = server.unwrap_or(provider_name(model));
    let mut watch = StallWatch::new(provider, limit, max_thinking);
    loop {
        let event = match watch.next(stream).await? {
            None => break,
            Some(event) => event.map_err(|e| model_error(model, server, e))?,
        };
        let pieces = match event {
            ChatStreamEvent::Chunk(chunk) => {
                watch.text(&chunk.content);
                timings
                    .first_token_ms
                    .get_or_insert_with(|| ms_since(started));
                text.push_str(&chunk.content);
                parser.feed(&chunk.content)
            }
            ChatStreamEvent::ReasoningChunk(chunk) => {
                if !chunk.content.is_empty() {
                    on(Event::Thinking(&chunk.content));
                }
                watch.thought(&chunk.content)?;
                continue;
            }
            ChatStreamEvent::End(end) => {
                stop = end.captured_stop_reason;
                break;
            }
            _ => continue,
        };
        emit(pieces, ctx, &mut citations, on);
    }
    emit(parser.finish(), ctx, &mut citations, on);
    // Citation marks alone give the reader nothing to read either.
    if plain_text(&text).trim().is_empty() {
        return Err(empty_reply_error(provider, stop));
    }
    Ok((text, citations))
}

/// Watches a streamed reply for the ways it stalls before an answer: the
/// provider goes silent, the model thinks past its limit, or its reasoning
/// repeats itself. Once the answer has begun, only silence is watched.
pub(super) struct StallWatch<'a> {
    provider: &'a str,
    silence: Duration,
    max_thinking: Duration,
    last_activity: Instant,
    thinking_started: Option<Instant>,
    thought_tail: String,
    since_loop_check: usize,
    answering: bool,
}

impl<'a> StallWatch<'a> {
    pub(super) fn new(provider: &'a str, silence: Duration, max_thinking: Duration) -> Self {
        StallWatch {
            provider,
            silence,
            max_thinking,
            last_activity: Instant::now(),
            thinking_started: None,
            thought_tail: String::new(),
            since_loop_check: 0,
            answering: false,
        }
    }

    /// The next stream event, or an error once the wait for it ran out.
    pub(super) async fn next(
        &mut self,
        stream: &mut genai::chat::ChatStream,
    ) -> Result<Option<genai::Result<ChatStreamEvent>>> {
        let thinking_since = self
            .thinking_started
            .filter(|_| !self.answering && self.max_thinking != Duration::MAX);
        let next = match thinking_since {
            Some(since) => {
                let thinking_left = self.max_thinking.saturating_sub(since.elapsed());
                let wait = if self.silence == Duration::MAX {
                    thinking_left
                } else {
                    self.silence
                        .saturating_sub(self.last_activity.elapsed())
                        .min(thinking_left)
                };
                tokio::time::timeout(wait, stream.next()).await
            }
            None => next_event(stream, self.silence, self.last_activity).await,
        };
        match next {
            Ok(event) => Ok(event),
            Err(_) if thinking_since.is_some_and(|t| t.elapsed() >= self.max_thinking) => {
                Err(self.thought_too_long())
            }
            Err(_) => Err(Error::Model(format!(
                "{} sent nothing for {} s, so Bowerbird stopped waiting. Try again, or choose another model in Settings.",
                self.provider,
                self.silence.as_secs()
            ))),
        }
    }

    /// Notes a piece of the answer.
    pub(super) fn text(&mut self, piece: &str) {
        if piece.is_empty() {
            return;
        }
        self.last_activity = Instant::now();
        self.answering = true;
        self.thought_tail.clear();
    }

    /// Notes a piece of reasoning, and fails once reasoning before the answer
    /// has run past its limit or repeats itself.
    pub(super) fn thought(&mut self, piece: &str) -> Result<()> {
        if piece.is_empty() {
            return Ok(());
        }
        self.last_activity = Instant::now();
        if self.answering {
            return Ok(());
        }
        let since = *self.thinking_started.get_or_insert_with(Instant::now);
        if self.max_thinking != Duration::MAX && since.elapsed() >= self.max_thinking {
            return Err(self.thought_too_long());
        }
        self.thought_tail.push_str(piece);
        if self.thought_tail.len() > THOUGHT_WINDOW * 2 {
            let cut = self
                .thought_tail
                .ceil_char_boundary(self.thought_tail.len() - THOUGHT_WINDOW);
            self.thought_tail.drain(..cut);
        }
        self.since_loop_check += piece.len();
        if self.since_loop_check >= LOOP_CHECK_EVERY {
            self.since_loop_check = 0;
            if reasoning_is_looping(&self.thought_tail) {
                return Err(Error::Model(format!(
                    "{} got stuck repeating its reasoning before writing an answer. Try again, lower Thinking in Settings, or choose another model.",
                    self.provider
                )));
            }
        }
        Ok(())
    }

    fn thought_too_long(&self) -> Error {
        Error::Model(format!(
            "{} thought for {} s without starting an answer. Lower Thinking in Settings, or choose another model.",
            self.provider,
            self.max_thinking.as_secs()
        ))
    }
}

/// True when the end of a reasoning trace repeats the same phrase or block
/// verbatim back-to-back, catching models stuck in a thinking loop while
/// ignoring formatting lines like `---`.
fn reasoning_is_looping(tail: &str) -> bool {
    let bytes = tail.as_bytes();
    let n = bytes.len();
    let max_period = (n / 3).min(1500);
    for period in 20..=max_period {
        if bytes[n - 1] != bytes[n - 1 - period] || bytes[n - period] != bytes[n - 2 * period] {
            continue;
        }
        let reps = if period >= 100 { 3 } else { 4 };
        if reps * period > n || !tail.is_char_boundary(n - period) {
            continue;
        }
        let unit = &bytes[n - period..n];
        if (2..=reps).all(|k| &bytes[n - k * period..n - (k - 1) * period] == unit)
            && has_words(&tail[n - period..n])
        {
            return true;
        }
    }
    false
}

/// Enough distinct text to be a repeated thought rather than punctuation or a
/// repeated single letter.
fn has_words(unit: &str) -> bool {
    let mut alnum = 0usize;
    let mut seen = [0u32; 8];
    let mut distinct = 0usize;
    for c in unit.chars() {
        if c.is_alphanumeric() {
            alnum += 1;
            let code = c as u32;
            if distinct < seen.len() && !seen[..distinct].contains(&code) {
                seen[distinct] = code;
                distinct += 1;
            }
        }
    }
    alnum >= 12 && distinct >= 6
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
/// other passages, since a quote can run across a passage boundary. A quote
/// found only in another source stays unverified: the answer attributed it to
/// the wrong source.
pub fn resolve(label: &str, quote: String, passages: &[Passage]) -> Citation {
    let letter = cite::split_label(label).map_or("", |(l, _)| l).to_string();
    let cited = passages.iter().find(|p| p.label == label);
    let same_source = passages
        .iter()
        .filter(|p| cite::split_label(&p.label).is_some_and(|(l, _)| l == letter));
    let found = cited
        .into_iter()
        .chain(same_source)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use genai::ModelIden;
    use genai::resolver::AuthData;

    use super::*;
    use crate::ask::client::{client, reply_starts};
    use crate::ask::testing::*;

    /// The reply `target` streams, read the way an answer is.
    async fn read_with(
        target: genai::ServiceTarget,
        limit: Duration,
        max_thinking: Duration,
    ) -> Result<Vec<String>> {
        let client = client(Arc::default()).unwrap();
        let mut stream = client
            .exec_chat_stream(target, question(), None)
            .await
            .unwrap()
            .stream;
        let ctx = Context {
            hits: Vec::new(),
            passages: Vec::new(),
        };
        let mut events = Vec::new();
        read_reply(
            &mut stream,
            "open_router::x",
            None,
            limit,
            max_thinking,
            Instant::now(),
            &mut Timings::default(),
            &ctx,
            &mut |event| {
                events.push(match event {
                    Event::Text(text) => format!("text {text}"),
                    Event::Citation(c) => format!("cite {}", c.label),
                    Event::Thinking(text) => format!("thinking {text}"),
                })
            },
        )
        .await?;
        Ok(events)
    }

    async fn read(target: genai::ServiceTarget, limit: Duration) -> Result<Vec<String>> {
        read_with(target, limit, THINKING_LIMIT).await
    }

    #[test]
    fn resolve_looks_for_a_quote_only_in_the_cited_source() {
        let passages = vec![
            passage("A1", "a", "Satin bowerbirds prefer blue objects."),
            passage("A2", "a", "Females inspect the bower before mating."),
            passage("B0", "b", "The Vogelkop bowerbird sorts finds by color."),
        ];
        let c = resolve("A1", "inspect the bower before mating".into(), &passages);
        assert!(c.verified);
        assert_eq!(c.label, "A2");
        let c = resolve("A7", "inspect the bower before mating".into(), &passages);
        assert!(c.verified);
        assert_eq!(c.label, "A2");

        let c = resolve("A1", "sorts finds by color".into(), &passages);
        assert!(!c.verified);
        assert_eq!((c.label.as_str(), c.letter.as_str()), ("A1", "A"));

        let c = resolve("A1", "males paint the bower with berries".into(), &passages);
        assert!(!c.verified);
        assert_eq!(c.label, "A1");
    }

    #[tokio::test]
    async fn a_thinking_model_is_reported_before_its_answer_arrives() {
        let reasoning = |text: &str| {
            format!(
                "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning\":\"{text}\"}}}}]}}\n\n"
            )
        };
        let reply = format!(
            "{}{}data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"Blue.\"}}}}]}}\n\ndata: [DONE]\n\n",
            reasoning("The user asks"),
            reasoning(" about color.")
        );
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", &reply))).await;
        assert_eq!(
            read(target, SILENCE).await.unwrap(),
            [
                "thinking The user asks",
                "thinking  about color.",
                "text Blue."
            ]
        );
    }

    #[tokio::test]
    async fn an_answer_that_ends_without_text_says_why() {
        let thought =
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"Let me see.\"}}]}\n\n";
        for (finish, message) in [
            (
                "content_filter",
                "OpenRouter ended the answer before writing any of it: a content filter stopped it.",
            ),
            (
                "length",
                "OpenRouter ended the answer before writing any of it: the model hit its length limit.",
            ),
            ("stop", "OpenRouter sent an empty answer."),
        ] {
            let reply = format!(
                "{thought}data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\ndata: [DONE]\n\n"
            );
            let target = provider(AdapterKind::OpenRouter, Some(("200 OK", &reply))).await;
            assert_eq!(
                read(target, SILENCE).await.unwrap_err().to_string(),
                format!("{message} Try again, or choose another model in Settings.")
            );
        }
    }

    #[tokio::test]
    async fn an_answer_of_citations_alone_has_nothing_to_read() {
        let reply = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"[[A1|signed in 1648]] \"}}]}\n\ndata: [DONE]\n\n";
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", reply))).await;
        assert_eq!(
            read(target, SILENCE).await.unwrap_err().to_string(),
            "OpenRouter sent an empty answer. Try again, or choose another model in Settings."
        );
    }

    #[tokio::test]
    async fn a_provider_that_goes_silent_is_given_up_on() {
        let limit = Duration::from_secs(1);
        assert_eq!(
            read(silent_provider().await, limit)
                .await
                .unwrap_err()
                .to_string(),
            "OpenRouter sent nothing for 1 s, so Bowerbird stopped waiting. Try again, or choose another model in Settings."
        );
        let client = client(Arc::default()).unwrap();
        assert!(
            !reply_starts(&client, silent_provider().await, limit)
                .await
                .unwrap()
        );
        assert_eq!(silence_limit("open_router::x"), SILENCE);
        assert_eq!(silence_limit("llama3:8b"), Duration::MAX);
        assert_eq!(thinking_limit("open_router::x"), THINKING_LIMIT);
        assert_eq!(thinking_limit("llama3:8b"), Duration::MAX);
    }

    #[tokio::test]
    async fn a_reasoning_loop_stops_instead_of_waiting_forever() {
        let phrase = "Wait, let me double-check passage A3 for the exact quote. ";
        let chunk = format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning\":\"{phrase}\"}}}}]}}\n\n"
        );
        let reply = format!("{}{}[DONE]\n\n", chunk.repeat(6), "data: ");
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", &reply))).await;
        assert_eq!(
            read(target, SILENCE).await.unwrap_err().to_string(),
            "OpenRouter got stuck repeating its reasoning before writing an answer. Try again, lower Thinking in Settings, or choose another model."
        );
    }

    #[test]
    fn normal_reasoning_and_rules_are_not_mistaken_for_a_loop() {
        assert!(!reasoning_is_looping(&"-".repeat(120)));
        assert!(!reasoning_is_looping(
            "Let me check A1 for influenza. Now check A2 for asthma. Now check A3 for bronchitis. Now check A4 for pneumonia."
        ));
        assert!(reasoning_is_looping(
            &"Wait, let me check passage A3 again. ".repeat(4)
        ));
        assert!(reasoning_is_looping(&format!(
            "{}Wait, let",
            "Wait, let me check passage A3 again. ".repeat(4)
        )));
        assert!(reasoning_is_looping(
            &"等一下，让我重新检查段落A3中的确切措辞。".repeat(4)
        ));
    }

    #[tokio::test]
    async fn a_model_that_thinks_too_long_without_answering_is_stopped() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let _ = conn.read(&mut request).await;
            let _ = conn
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await;
            for step in 0..30 {
                let frame = format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning\":\"Considering point number {step} from the source. \"}}}}]}}\n\n"
                );
                if conn.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        });
        let target = genai::ServiceTarget {
            endpoint: genai::resolver::Endpoint::from_owned(endpoint),
            auth: AuthData::from_single("key"),
            model: ModelIden::new(AdapterKind::OpenRouter, "some-model"),
        };
        assert_eq!(
            read_with(target, SILENCE, Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string(),
            "OpenRouter thought for 1 s without starting an answer. Lower Thinking in Settings, or choose another model."
        );
    }

    #[tokio::test]
    async fn a_mid_stream_provider_error_shows_the_providers_message() {
        let reply = "data: {\"error\":{\"message\":\"Provider returned error\",\"code\":502}}\n\n";
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", reply))).await;
        assert_eq!(
            read(target, SILENCE).await.unwrap_err().to_string(),
            "OpenRouter had a problem (502). Try again in a moment."
        );
        let reply =
            "data: {\"error\":{\"message\":\"Moderation flagged this request\",\"code\":403}}\n\n";
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", reply))).await;
        assert_eq!(
            read(target, SILENCE).await.unwrap_err().to_string(),
            "OpenRouter refused the request (403): Moderation flagged this request"
        );
    }
}
