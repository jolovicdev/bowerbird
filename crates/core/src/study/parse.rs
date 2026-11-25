//! Reads a model's JSON reply into a study set, keeping only the entries
//! that are complete and checking each quote against its passage.

use serde::Deserialize;

use super::{
    AudioOverview, Card, FACTS_MAX, Fact, Infographic, Kind, LINES_MAX, Line, MAP_CHILDREN,
    MAP_DEPTH, MapNode, Question, Request, Row, SLIDES_MAX, Slide, StudySet, TABLE_COLUMNS,
    TABLE_ROWS, Table,
};
use crate::ask::{self, Citation};
use crate::retrieve::Context;

#[derive(Deserialize, Default)]
struct Raw {
    #[serde(default)]
    title: String,
    #[serde(default)]
    children: Vec<serde_json::Value>,
    #[serde(default)]
    columns: Vec<serde_json::Value>,
    #[serde(default)]
    rows: Vec<serde_json::Value>,
    #[serde(default)]
    slides: Vec<serde_json::Value>,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    figures: Vec<serde_json::Value>,
    #[serde(default)]
    timeline: Vec<serde_json::Value>,
    #[serde(default)]
    points: Vec<serde_json::Value>,
    #[serde(default)]
    cards: Vec<serde_json::Value>,
    #[serde(default)]
    questions: Vec<serde_json::Value>,
    #[serde(default)]
    lines: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawCard {
    front: String,
    back: String,
    #[serde(default)]
    cite: String,
}

#[derive(Deserialize)]
struct RawQuestion {
    question: String,
    choices: Vec<String>,
    answer: usize,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    cite: String,
}

/// Reads the model's JSON, dropping items that are malformed instead of
/// failing the whole set. None when nothing usable is left.
pub(super) fn parse(reply: &str, request: &Request, ctx: &Context, now: u64) -> Option<StudySet> {
    let raw: Raw = serde_json::from_str(ask::json_object(reply)?).ok()?;
    let citation = |cite: &str| {
        let (label, quote) = cite.split_once('|')?;
        Some(ask::resolve(
            label.trim(),
            quote.trim().to_string(),
            &ctx.passages,
        ))
    };
    let cards: Vec<Card> = raw
        .cards
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawCard>(v).ok())
        .filter(|c| !c.front.trim().is_empty() && !c.back.trim().is_empty())
        .map(|c| Card {
            citation: citation(&c.cite),
            front: c.front.trim().to_string(),
            back: c.back.trim().to_string(),
            level: 0,
            due: 0,
        })
        .collect();
    let questions: Vec<Question> = raw
        .questions
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawQuestion>(v).ok())
        .filter(|q| q.choices.len() >= 2 && q.answer < q.choices.len())
        .enumerate()
        .map(|(i, q)| {
            // Models favor the first positions for the right choice; rotating
            // each question by a different amount spreads it out.
            let n = q.choices.len();
            let shift = (now as usize + i * 3) % n;
            let mut choices = q.choices;
            choices.rotate_left(shift);
            Question {
                citation: citation(&q.cite),
                question: q.question.trim().to_string(),
                answer: (q.answer + n - shift) % n,
                choices,
                hint: q.hint.trim().to_string(),
                explanation: q.explanation.trim().to_string(),
            }
        })
        .collect();
    let map = (request.kind == Kind::MindMap).then(|| MapNode {
        title: raw.title.trim().to_string(),
        children: map_children(raw.children, 1),
    });
    let table = (request.kind == Kind::Table).then(|| table(raw.columns, raw.rows, &citation));
    let slides: Vec<Slide> = raw
        .slides
        .iter()
        .filter_map(|v| {
            let title = v.get("title").map(cell).filter(|t| !t.is_empty())?;
            let list = |key: &str| -> Vec<String> {
                v.get(key)
                    .and_then(|p| p.as_array())
                    .into_iter()
                    .flatten()
                    .map(cell)
                    .filter(|p| !p.is_empty())
                    .collect()
            };
            Some(Slide {
                title,
                points: list("points"),
                notes: v.get("notes").map(cell).unwrap_or_default(),
                citations: list("cites").iter().filter_map(|c| citation(c)).collect(),
            })
        })
        .take(SLIDES_MAX)
        .collect();
    let infographic = (request.kind == Kind::Infographic).then(|| {
        let facts = |items: &[serde_json::Value], head: &str, text: &str| -> Vec<Fact> {
            items
                .iter()
                .filter_map(|v| {
                    let head = v.get(head).map(cell).unwrap_or_default();
                    let text = v.get(text).map(cell).unwrap_or_default();
                    let cite = v.get("cite").map(cell).unwrap_or_default();
                    (!head.is_empty() && !text.is_empty()).then(|| Fact {
                        head,
                        text,
                        citation: citation(&cite),
                    })
                })
                .take(FACTS_MAX)
                .collect()
        };
        Infographic {
            subtitle: raw.subtitle.trim().to_string(),
            figures: facts(&raw.figures, "value", "label"),
            timeline: facts(&raw.timeline, "date", "text"),
            points: facts(&raw.points, "title", "text"),
        }
    });
    let audio = (request.kind == Kind::Audio).then(|| AudioOverview {
        format: request.format,
        lines: raw
            .lines
            .iter()
            .filter_map(|v| {
                let text = v.get("text").map(cell).filter(|t| !t.is_empty())?;
                let cites = v.get("cites").and_then(|c| c.as_array());
                Some(Line {
                    host: u8::from(v.get("host").map(cell).as_deref() == Some("2")),
                    text,
                    citations: cites
                        .into_iter()
                        .flatten()
                        .filter_map(|c| citation(&cell(c)))
                        .collect(),
                    start: 0.0,
                })
            })
            .take(LINES_MAX)
            .collect(),
        ..AudioOverview::default()
    });
    let usable = match request.kind {
        Kind::Flashcards => !cards.is_empty(),
        Kind::Quiz => !questions.is_empty(),
        Kind::MindMap => map.as_ref().is_some_and(|m| !m.children.is_empty()),
        Kind::Table => table
            .as_ref()
            .is_some_and(|t| !t.columns.is_empty() && !t.rows.is_empty()),
        Kind::Slides => !slides.is_empty(),
        Kind::Infographic => infographic
            .as_ref()
            .is_some_and(|i| i.figures.len() + i.timeline.len() + i.points.len() > 0),
        Kind::Audio => audio.as_ref().is_some_and(|a| !a.lines.is_empty()),
    };
    usable.then(|| StudySet {
        kind: request.kind,
        title: match (raw.title.trim(), request.kind) {
            ("", Kind::Quiz) => "Quiz".to_string(),
            ("", Kind::MindMap) => "Mind map".to_string(),
            ("", Kind::Table) => "Table".to_string(),
            ("", Kind::Slides) => "Slides".to_string(),
            ("", Kind::Infographic) => "Infographic".to_string(),
            ("", Kind::Flashcards) => "Flashcards".to_string(),
            ("", Kind::Audio) => "Audio overview".to_string(),
            (title, _) => title.to_string(),
        },
        created: now,
        model: String::new(),
        difficulty: request.difficulty,
        topic: request.topic.trim().to_string(),
        cards,
        questions,
        attempts: Vec::new(),
        map,
        table,
        slides,
        infographic,
        audio,
    })
}

/// A cell's text: models sometimes write numbers or nulls instead of strings.
fn cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The model's columns and rows, each row cut or padded to the columns and
/// rows with nothing in them dropped.
fn table(
    columns: Vec<serde_json::Value>,
    rows: Vec<serde_json::Value>,
    citation: &impl Fn(&str) -> Option<Citation>,
) -> Table {
    let columns: Vec<String> = columns
        .iter()
        .map(cell)
        .filter(|c| !c.is_empty())
        .take(TABLE_COLUMNS)
        .collect();
    let rows = rows
        .iter()
        .filter_map(|row| {
            let mut cells: Vec<String> = row.get("cells")?.as_array()?.iter().map(cell).collect();
            cells.resize(columns.len(), String::new());
            let cite = row.get("cite").and_then(|c| c.as_str()).unwrap_or_default();
            (cells.iter().any(|c| !c.is_empty())).then(|| Row {
                cells,
                citation: citation(cite),
            })
        })
        .take(TABLE_ROWS)
        .collect();
    Table { columns, rows }
}

/// Topics from the model's JSON, without untitled ones and cut to a size a
/// page can show.
fn map_children(values: Vec<serde_json::Value>, depth: usize) -> Vec<MapNode> {
    if depth > MAP_DEPTH {
        return Vec::new();
    }
    values
        .into_iter()
        .filter_map(|v| {
            let title = v.get("title")?.as_str()?.trim().to_string();
            let children = match v.get("children") {
                Some(serde_json::Value::Array(c)) => map_children(c.clone(), depth + 1),
                _ => Vec::new(),
            };
            (!title.is_empty()).then_some(MapNode { title, children })
        })
        .take(MAP_CHILDREN)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::AudioFormat;
    use crate::study::tests::{ctx, request};

    #[test]
    fn malformed_cards_are_dropped_and_quotes_checked() {
        let reply = r#"Here you go: {"title": "Westphalia", "cards": [
            {"front": "When was the Treaty of Westphalia signed?", "back": "In 1648.", "cite": "A0|was signed in 1648, ending"},
            {"front": "Who won?", "back": "Sweden.", "cite": "A0|Sweden won the war outright"},
            {"front": "", "back": "no front"},
            {"front": 3}
        ]}"#;
        let set = parse(reply, &request(Kind::Flashcards), &ctx(), 0).unwrap();
        assert_eq!(set.title, "Westphalia");
        assert_eq!(set.cards.len(), 2);
        let first = set.cards[0].citation.as_ref().unwrap();
        assert!(first.verified && first.page == Some(4));
        assert!(!set.cards[1].citation.as_ref().unwrap().verified);
        assert!(parse("no json here", &request(Kind::Flashcards), &ctx(), 0).is_none());
    }

    #[test]
    fn quiz_choices_rotate_with_the_right_answer() {
        let reply = r#"{"questions": [
            {"question": "When?", "choices": ["1648", "1618", "1555", "1789"], "answer": 0, "cite": "A0|signed in 1648"},
            {"question": "Bad", "choices": ["x"], "answer": 3}
        ]}"#;
        for now in 0..4 {
            let set = parse(reply, &request(Kind::Quiz), &ctx(), now).unwrap();
            assert_eq!(set.questions.len(), 1);
            let q = &set.questions[0];
            assert_eq!(q.choices[q.answer], "1648");
        }
        let positions: std::collections::HashSet<usize> = (0..4)
            .map(|now| {
                parse(reply, &request(Kind::Quiz), &ctx(), now)
                    .unwrap()
                    .questions[0]
                    .answer
            })
            .collect();
        assert_eq!(positions.len(), 4, "the right answer moves around");
    }

    #[test]
    fn mind_maps_drop_untitled_topics_and_stop_at_a_readable_depth() {
        let reply = r#"{"title": "Thirty Years' War", "children": [
            {"title": "Causes", "children": [{"title": "Defenestration", "children": [
                {"title": "Revolt", "children": [{"title": "Too deep"}]}]}]},
            {"title": "  ", "children": [{"title": "Orphan"}]},
            {"title": "Peace of Westphalia"},
            {"name": "no title"}
        ]}"#;
        let set = parse(reply, &request(Kind::MindMap), &ctx(), 0).unwrap();
        let map = set.map.unwrap();
        assert_eq!(set.title, "Thirty Years' War");
        let themes: Vec<&str> = map.children.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(themes, ["Causes", "Peace of Westphalia"]);
        let revolt = &map.children[0].children[0].children[0];
        assert_eq!(revolt.title, "Revolt");
        assert!(revolt.children.is_empty(), "levels below the third are cut");
        assert!(parse(r#"{"title": "Empty"}"#, &request(Kind::MindMap), &ctx(), 0).is_none());
    }

    #[test]
    fn table_rows_fit_their_columns_and_cite_their_passage() {
        let reply = r#"{"title": "Treaties", "columns": ["Treaty", "Year", "", "Signed by"], "rows": [
            {"cells": ["Westphalia", 1648, "Holy Roman Empire, France, Sweden", "extra"], "cite": "A0|signed in 1648, ending"},
            {"cells": ["Augsburg", null], "cite": "A0|not in the passage at all"},
            {"cells": ["", ""]},
            {"no cells": true}
        ]}"#;
        let set = parse(reply, &request(Kind::Table), &ctx(), 0).unwrap();
        let table = set.table.unwrap();
        assert_eq!(table.columns, ["Treaty", "Year", "Signed by"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            table.rows[0].cells,
            ["Westphalia", "1648", "Holy Roman Empire, France, Sweden"]
        );
        assert_eq!(table.rows[1].cells, ["Augsburg", "", ""]);
        assert!(table.rows[0].citation.as_ref().unwrap().verified);
        assert!(!table.rows[1].citation.as_ref().unwrap().verified);
        assert!(
            parse(
                r#"{"columns": ["A"], "rows": []}"#,
                &request(Kind::Table),
                &ctx(),
                0
            )
            .is_none()
        );
    }

    #[test]
    fn slides_keep_titled_slides_and_check_their_quotes() {
        let reply = r#"{"title": "The Peace of 1648", "slides": [
            {"title": "The war ended in 1648", "points": ["Signed at Westphalia", "", 1648], "notes": "Say when.", "cites": ["A0|was signed in 1648, ending", "nonsense"]},
            {"title": "", "points": ["untitled slides are dropped"]},
            {"title": "What changed", "points": []}
        ]}"#;
        let set = parse(reply, &request(Kind::Slides), &ctx(), 0).unwrap();
        assert_eq!(set.slides.len(), 2);
        assert_eq!(set.slides[0].points, ["Signed at Westphalia", "1648"]);
        assert_eq!(set.slides[0].citations.len(), 1);
        assert!(set.slides[0].citations[0].verified);
        assert!(parse(r#"{"slides": []}"#, &request(Kind::Slides), &ctx(), 0).is_none());
    }

    #[test]
    fn infographics_keep_complete_facts_with_their_sources() {
        let reply = r#"{"title": "Westphalia", "subtitle": "How the war ended.",
            "figures": [{"value": "1648", "label": "year of the peace", "cite": "A0|signed in 1648"}, {"value": "", "label": "no value"}],
            "timeline": [{"date": "1648", "text": "Peace signed"}],
            "points": [{"title": "Sovereignty", "text": "States gained it.", "cite": "A0|not in the passage"}]}"#;
        let set = parse(reply, &request(Kind::Infographic), &ctx(), 0).unwrap();
        let info = set.infographic.unwrap();
        assert_eq!(info.subtitle, "How the war ended.");
        assert_eq!(info.figures.len(), 1);
        assert!(info.figures[0].citation.as_ref().unwrap().verified);
        assert_eq!(info.timeline[0].head, "1648");
        assert!(info.timeline[0].citation.is_none());
        assert!(!info.points[0].citation.as_ref().unwrap().verified);
        assert!(
            parse(
                r#"{"title": "Empty", "figures": []}"#,
                &request(Kind::Infographic),
                &ctx(),
                0
            )
            .is_none()
        );
    }

    #[test]
    fn audio_scripts_keep_spoken_lines_with_their_hosts_and_sources() {
        let reply = r#"{"title": "Westphalia", "lines": [
            {"host": 1, "text": "Today, how the war ended.", "cites": []},
            {"host": "2", "text": "It ended in 1648.", "cites": ["A0|signed in 1648, ending", "A0|signed in 1650"]},
            {"host": 1, "text": "  "},
            {"host": 2, "text": "And then?"}]}"#;
        let set = parse(reply, &request(Kind::Audio), &ctx(), 0).unwrap();
        let audio = set.audio.unwrap();
        assert_eq!(audio.format, AudioFormat::Debate);
        let hosts: Vec<u8> = audio.lines.iter().map(|l| l.host).collect();
        assert_eq!(hosts, [0, 1, 1]);
        assert_eq!(audio.lines[1].citations.len(), 2);
        assert!(audio.lines[1].citations[0].verified);
        assert!(!audio.lines[1].citations[1].verified);
        assert_eq!(audio.seconds, 0.0);
        assert!(
            parse(
                r#"{"title": "Silent", "lines": []}"#,
                &request(Kind::Audio),
                &ctx(),
                0
            )
            .is_none()
        );
    }
}
