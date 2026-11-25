//! Study material written from the sources: flashcards, quizzes, mind maps,
//! tables, slide decks, and infographics. Each card, question, row, slide,
//! and fact carries quotes checked against their passages, like an answer's
//! citations, so a learner can see where it came from.

use serde::{Deserialize, Serialize};

use crate::ask::{self, Citation};
use crate::retrieve::Context;
use crate::{Error, Result};

mod parse;

use parse::parse;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Flashcards,
    Quiz,
    #[serde(rename = "mindmap")]
    MindMap,
    Table,
    Slides,
    Infographic,
    Audio,
}

/// How the two hosts of an audio overview talk about the sources.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AudioFormat {
    #[default]
    DeepDive,
    Brief,
    Critique,
    Debate,
}

/// One turn of an audio overview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Line {
    /// Which of the two hosts speaks: 0 or 1.
    pub host: u8,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<Citation>,
    /// Seconds into the recording where the line starts; 0 until recorded.
    #[serde(default)]
    pub start: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AudioOverview {
    pub format: AudioFormat,
    pub lines: Vec<Line>,
    /// Length of the recording in seconds; 0 until it is recorded.
    #[serde(default)]
    pub seconds: f32,
    /// Why the recording failed, when it did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub problem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Slide {
    pub title: String,
    /// Short points, shown in order.
    pub points: Vec<String>,
    /// What a presenter says over the slide.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub citations: Vec<Citation>,
}

/// The facts an infographic draws: big numbers, dated events, and takeaways.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Infographic {
    pub subtitle: String,
    pub figures: Vec<Fact>,
    pub timeline: Vec<Fact>,
    pub points: Vec<Fact>,
}

/// One fact on an infographic: a figure (value and label), an event (date
/// and text), or a takeaway (title and text).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    pub head: String,
    pub text: String,
    pub citation: Option<Citation>,
}

/// Facts collected from the sources into named columns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Row {
    /// One per column; empty where the sources don't say.
    pub cells: Vec<String>,
    pub citation: Option<Citation>,
}

/// Most slides a deck keeps, and facts in each part of an infographic.
const SLIDES_MAX: usize = 25;
const FACTS_MAX: usize = 8;

/// Most columns and rows a table keeps.
const TABLE_COLUMNS: usize = 8;
const TABLE_ROWS: usize = 60;

/// One topic in a mind map and the topics under it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MapNode {
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MapNode>,
}

/// Deepest level of a mind map below its root, and most topics under one.
const MAP_DEPTH: usize = 3;
const MAP_CHILDREN: usize = 8;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Easy,
    #[default]
    Medium,
    Hard,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub front: String,
    pub back: String,
    pub citation: Option<Citation>,
    /// Leitner box: 0 is new or missed; each higher box comes back later.
    #[serde(default)]
    pub level: u8,
    /// Unix seconds when the card is next due; 0 means now.
    #[serde(default)]
    pub due: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub question: String,
    pub choices: Vec<String>,
    /// Position of the right choice.
    pub answer: usize,
    pub hint: String,
    pub explanation: String,
    pub citation: Option<Citation>,
}

/// One run through a quiz.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attempt {
    pub ts: u64,
    /// The choice picked for each question, if any.
    pub answers: Vec<Option<usize>>,
    pub score: usize,
}

/// A flashcard deck or a quiz, saved as `studio/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StudySet {
    pub kind: Kind,
    pub title: String,
    pub created: u64,
    pub model: String,
    pub difficulty: Difficulty,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub topic: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<MapNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<Table>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slides: Vec<Slide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infographic: Option<Infographic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioOverview>,
}

pub struct Request {
    pub kind: Kind,
    pub count: usize,
    pub difficulty: Difficulty,
    /// What to focus on; empty means the whole notebook.
    pub topic: String,
    /// What a table collects, in the user's words. Tables only.
    pub describe: String,
    /// Audio overviews only.
    pub format: AudioFormat,
}

/// Most lines an audio overview keeps, about twenty minutes of speech.
const LINES_MAX: usize = 150;

const DAY: u64 = 86_400;
/// Days until a card in each box is due again.
const INTERVALS: [u64; 6] = [0, 1, 3, 7, 16, 35];

/// Passage labels mean nothing to a reader, who only sees the study set.
const NO_LABELS: &str = "Passage labels such as A3 belong only in cite fields. Never write a label in any text the reader sees, and don't refer to a passage or source by label or as \"the passage\"; state the facts themselves.";
/// Loose on purpose, like `ask::CITE_RULES`, so reasoning models don't spend
/// thousands of tokens counting words or verifying quotes before writing JSON.
const NO_QUOTE_COUNTS: &str = "Bowerbird checks every quote against the passages itself, so don't count words or check quotes.";
const FLASHCARDS_SYSTEM: &str = "You write flashcards that help the user study their own sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"cards\": [{\"front\": \"…\", \"back\": \"…\", \"cite\": \"A3|exact words\"}]}.
title names the deck in a few words. front is a question or a term; back answers it in at most two sentences. cite is the label of the passage that supports the back, a bar, then a short phrase copied from that passage.
Cover different parts of the material rather than asking about one fact twice. Write in the language of the sources.";

const QUIZ_SYSTEM: &str = "You write a multiple-choice quiz that tests understanding of the user's sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"questions\": [{\"question\": \"…\", \"choices\": [\"…\", \"…\", \"…\", \"…\"], \"answer\": 0, \"hint\": \"…\", \"explanation\": \"…\", \"cite\": \"A3|exact words\"}]}.
title names the quiz in a few words. Each question has four choices and exactly one right one; answer is its position, counting from 0. Wrong choices should sound plausible to someone who only skimmed. The hint points toward the answer without giving it away. The explanation says in one or two sentences why the right choice is right. cite is the label of the passage that supports the answer, a bar, then a short phrase copied from that passage.
Cover different parts of the material. Write in the language of the sources.";

const MAP_SYSTEM: &str = "You draw a mind map of the user's sources, using only the outline and passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"children\": [{\"title\": \"…\", \"children\": [ … ]}]}.
The root title names the main subject. Under it put four to seven main themes; under each theme, two to six subtopics; go one level deeper only where the sources give real detail. Every title is a short noun phrase of at most six words, specific to the sources rather than generic.
Write in the language of the sources.";

const TABLE_SYSTEM: &str = "You build a table from the user's sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"columns\": [\"…\", \"…\"], \"rows\": [{\"cells\": [\"…\", \"…\"], \"cite\": \"A3|exact words\"}]}.
title names the table in a few words. Choose two to six columns that fit what the user asks for, and add one row for each item the passages mention, up to 40 rows, in a sensible order such as by date. Keep each cell short: a name, a date, a number, or a phrase. Leave a cell empty when the passages don't say; never guess. cite is the label of the passage that supports the row, a bar, then a short phrase copied from that passage.
Write in the language of the sources.";

const SLIDES_SYSTEM: &str = "You make a slide deck that presents the user's sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"slides\": [{\"title\": \"…\", \"points\": [\"…\", \"…\"], \"notes\": \"…\", \"cites\": [\"A3|exact words\"]}]}.
The first slide introduces the subject and the last one sums up. Each slide has a short title, stated as a claim rather than a label where you can; two to five points of at most twelve words each; speaker notes of two to four sentences a presenter could say aloud; and one to three cites, each the label of a passage that supports the slide, a bar, then a short phrase copied from that passage.
Write in the language of the sources.";

const INFOGRAPHIC_SYSTEM: &str = "You lay out an infographic that summarizes the user's sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"subtitle\": \"…\", \"figures\": [{\"value\": \"…\", \"label\": \"…\", \"cite\": \"A3|exact words\"}], \"timeline\": [{\"date\": \"…\", \"text\": \"…\", \"cite\": \"…\"}], \"points\": [{\"title\": \"…\", \"text\": \"…\", \"cite\": \"…\"}]}.
title is a headline of at most eight words and subtitle one sentence. figures are two to four striking numbers the passages state: value is the number with at most one short unit word, such as \"109\", \"30 years\" or \"4.5 million\", and label says in a few words what it counts. timeline lists three to seven dated events in order when the sources have dates, and is empty when they don't. points are three or four key takeaways, each a title of a few words and one sentence. Every cite is the label of the passage that supports the item, a bar, then a short phrase copied from that passage. Use only facts the passages state.
Write in the language of the sources.";

const AUDIO_SYSTEM: &str = "You write the script of a spoken conversation between two hosts about the user's sources, using only the passages below.
Reply with JSON only, in the form {\"title\": \"…\", \"lines\": [{\"host\": 1, \"text\": \"…\", \"cites\": [\"A3|exact words\"]}]}.
title names the episode in a few words. host is 1 or 2. text is what that host says aloud, at most three sentences of plain spoken English: no lists, headings, symbols, stage directions, or sound effects, and abbreviations spelled out the way a listener would hear them. The hosts take turns, don't call each other by name, and talk to the listener as you. Open by saying what the episode covers and close with the takeaways. cites lists where the line's facts come from, each the label of the passage, a bar, then a short phrase copied from that passage; a line with no facts has an empty list. Use only facts the passages state.";

fn focus(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "Ask about key terms, people, places, dates, and definitions.",
        Difficulty::Medium => {
            "Mix facts with how and why: causes, effects, and how the ideas connect."
        }
        Difficulty::Hard => {
            "Ask for reasoning: compare, explain causes and consequences, and apply ideas to new cases."
        }
    }
}

/// The request line sent in place of a question, after the passages.
pub fn instruction(request: &Request) -> String {
    let topic = match request.topic.trim() {
        "" => String::new(),
        topic => format!(" Focus on: {topic}"),
    };
    match request.kind {
        Kind::Flashcards => format!(
            "Write {} flashcards. {}{topic}",
            request.count,
            focus(request.difficulty)
        ),
        Kind::Quiz => format!(
            "Write a quiz of {} questions. {}{topic}",
            request.count,
            focus(request.difficulty)
        ),
        Kind::MindMap => format!("Draw the mind map.{topic}"),
        Kind::Table => format!("Build this table: {}.{topic}", request.describe.trim()),
        Kind::Slides => format!("Make a deck of {} slides.{topic}", request.count),
        Kind::Infographic => format!("Lay out the infographic.{topic}"),
        Kind::Audio => {
            let shape = match request.format {
                AudioFormat::DeepDive => {
                    "a deep dive: host 1 explains, host 2 asks the questions a curious listener would and connects the ideas, with examples from the sources"
                }
                AudioFormat::Brief => "a brief: only the essentials, quickly",
                AudioFormat::Critique => {
                    "a critique: the hosts review the sources like expert reviewers, weighing their strengths, weaknesses, gaps, and what a reader should question"
                }
                AudioFormat::Debate => {
                    "a debate: the hosts take opposing positions the sources support, argue them fairly, and end by saying where they agree"
                }
            };
            format!("Write {shape}, in about {} lines.{topic}", request.count)
        }
    }
}

/// Asks the model for a deck or quiz over the passages in `prompt`, which
/// comes from `ask::prompt` with `instruction(request)` as its question.
pub async fn write(
    client: &genai::Client,
    model: &str,
    request: &Request,
    prompt: String,
    ctx: &Context,
    now: u64,
) -> Result<StudySet> {
    write_streaming(client, model, request, prompt, ctx, now, |_| {}, |_| {}).await
}

/// Streaming variant of `write` that reports live reasoning chunks and
/// cumulative output character count as the model builds the study set.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is a distinct input or progress callback"
)]
pub async fn write_streaming(
    client: &genai::Client,
    model: &str,
    request: &Request,
    prompt: String,
    ctx: &Context,
    now: u64,
    on_thinking: impl FnMut(&str),
    mut on_writing: impl FnMut(usize),
) -> Result<StudySet> {
    let system = match request.kind {
        Kind::Flashcards => FLASHCARDS_SYSTEM,
        Kind::Quiz => QUIZ_SYSTEM,
        Kind::MindMap => MAP_SYSTEM,
        Kind::Table => TABLE_SYSTEM,
        Kind::Slides => SLIDES_SYSTEM,
        Kind::Infographic => INFOGRAPHIC_SYSTEM,
        Kind::Audio => AUDIO_SYSTEM,
    };
    let system = if request.kind == Kind::MindMap {
        format!("{system}\n{NO_LABELS}")
    } else {
        format!("{system}\n{NO_LABELS}\n{NO_QUOTE_COUNTS}")
    };
    let mut chars = 0usize;
    let reply = ask::complete_streaming(client, model, &system, prompt, on_thinking, |chunk| {
        chars += chunk.chars().count();
        on_writing(chars);
    })
    .await?;
    let mut set = parse(&reply, request, ctx, now).ok_or_else(|| {
        let what = match request.kind {
            Kind::Flashcards => "flashcards",
            Kind::Quiz => "quiz questions",
            Kind::MindMap => "mind map",
            Kind::Table => "table",
            Kind::Slides => "slides",
            Kind::Infographic => "infographic",
            Kind::Audio => "script",
        };
        Error::Model(format!(
            "The model's reply had no usable {what}. Try again, or pick another model in Settings."
        ))
    })?;
    set.model = model.to_string();
    Ok(set)
}

impl StudySet {
    /// Cards due for review at `now`.
    pub fn due(&self, now: u64) -> usize {
        self.cards.iter().filter(|c| c.due <= now).count()
    }

    /// Moves a card up a box when the learner knew it, back to the first box
    /// when they didn't.
    pub fn review(&mut self, card: usize, knew: bool, now: u64) -> Result<()> {
        let card = self
            .cards
            .get_mut(card)
            .ok_or_else(|| Error::Invalid("This deck has no such card.".into()))?;
        card.level = if knew {
            (card.level + 1).min(INTERVALS.len() as u8 - 1)
        } else {
            0
        };
        card.due = now + INTERVALS[usize::from(card.level)] * DAY;
        Ok(())
    }

    /// Records a run through the quiz and returns its score.
    pub fn record(&mut self, answers: Vec<Option<usize>>, now: u64) -> Result<usize> {
        if answers.len() != self.questions.len() {
            return Err(Error::Invalid(
                "The answers don't match this quiz's questions.".into(),
            ));
        }
        let score = answers
            .iter()
            .zip(&self.questions)
            .filter(|(a, q)| **a == Some(q.answer))
            .count();
        self.attempts.push(Attempt {
            ts: now,
            answers,
            score,
        });
        Ok(score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieve::Passage;

    pub(super) fn ctx() -> Context {
        Context {
            hits: Vec::new(),
            passages: vec![Passage {
                label: "A0".into(),
                source_id: "a".into(),
                ord: 0,
                page_start: Some(4),
                page_end: Some(4),
                section: String::new(),
                text: "The Treaty of Westphalia was signed in 1648, ending the Thirty Years' War."
                    .into(),
            }],
        }
    }

    pub(super) fn request(kind: Kind) -> Request {
        Request {
            kind,
            count: 10,
            difficulty: Difficulty::Medium,
            topic: String::new(),
            describe: String::new(),
            format: AudioFormat::Debate,
        }
    }

    #[test]
    fn known_cards_come_back_later_and_missed_ones_now() {
        let mut set = parse(
            r#"{"cards": [{"front": "Q", "back": "A"}, {"front": "Q2", "back": "A2"}]}"#,
            &request(Kind::Flashcards),
            &ctx(),
            0,
        )
        .unwrap();
        let now = 1_000_000;
        assert_eq!(set.due(now), 2);
        set.review(0, true, now).unwrap();
        assert_eq!(set.cards[0].due, now + DAY);
        set.review(0, true, now).unwrap();
        assert_eq!(set.cards[0].due, now + 3 * DAY);
        assert_eq!(set.due(now), 1);
        set.review(0, false, now).unwrap();
        assert_eq!((set.cards[0].level, set.cards[0].due), (0, now));
        assert!(set.review(9, true, now).is_err());
    }

    #[test]
    fn quiz_attempts_are_scored() {
        let reply = r#"{"questions": [
            {"question": "When?", "choices": ["1648", "1618"], "answer": 0},
            {"question": "Which war?", "choices": ["Thirty Years'", "Hundred Years'"], "answer": 0}
        ]}"#;
        let mut set = parse(reply, &request(Kind::Quiz), &ctx(), 0).unwrap();
        let right: Vec<Option<usize>> = set.questions.iter().map(|q| Some(q.answer)).collect();
        assert_eq!(set.record(right, 5).unwrap(), 2);
        assert_eq!(set.record(vec![None, None], 6).unwrap(), 0);
        let first = &set.questions[0];
        let wrong = (first.answer + 1) % first.choices.len();
        let one_right = vec![Some(wrong), Some(set.questions[1].answer)];
        assert_eq!(set.record(one_right, 7).unwrap(), 1);
        assert!(set.record(vec![None], 8).is_err());
        assert_eq!(set.attempts.len(), 3);
    }
}
