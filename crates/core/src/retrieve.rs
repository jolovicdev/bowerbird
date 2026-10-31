//! Picks the passages a question gets answered from: keyword hits fused with
//! hits by meaning, their neighbors in the same section, and for Deep search,
//! whole sections the model asked for.

use std::collections::{HashMap, HashSet};

use crate::Result;
use crate::embed::Embedder;
use crate::index::{ChunkRow, Hit, Index, query_terms};
use crate::notebook::Notebook;

pub const HITS: usize = 8;
/// Keyword candidates considered when ranking.
const CANDIDATES: usize = 24;
/// Extra hits so a source without a top-8 hit still gets its best passage.
/// Questions that span two sources otherwise lose the weaker one.
const DIVERSITY_EXTRA: usize = 3;
/// Upper bound on passage text sent with one question.
pub const CHAR_BUDGET: usize = 40_000;
const OUTLINE_LINES_PER_SOURCE: usize = 40;
/// Reciprocal rank fusion's damping constant, from Cormack et al. (2009).
const RRF_K: f64 = 60.0;
/// Weight of the keyword list against the list by meaning in the fusion.
/// Questions worded differently from their source are found more often with
/// meaning weighted higher, while keyword hits still lift exact names and
/// numbers.
const KEYWORD_WEIGHT: f64 = 0.3;

#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    /// Source letter + chunk ord, e.g. "B12". The model cites this.
    pub label: String,
    pub source_id: String,
    pub ord: u32,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    pub section: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub hits: Vec<Hit>,
    pub passages: Vec<Passage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRef {
    pub source_id: String,
    /// Position in the source's section list.
    pub ord: usize,
}

#[derive(Default)]
struct Picker {
    rows: Vec<ChunkRow>,
    seen: HashSet<(String, u32)>,
    used: usize,
}

impl Picker {
    fn add(&mut self, row: ChunkRow) {
        if self.used + row.text.len() > CHAR_BUDGET
            || !self.seen.insert((row.source_id.clone(), row.ord))
        {
            return;
        }
        self.used += row.text.len();
        self.rows.push(row);
    }

    fn into_passages(self, nb: &Notebook) -> Vec<Passage> {
        let position = |id: &str| {
            nb.sources
                .iter()
                .position(|s| s.id == id)
                .unwrap_or(usize::MAX)
        };
        let mut rows = self.rows;
        rows.sort_by_key(|r| (position(&r.source_id), r.ord));
        rows.into_iter()
            .filter_map(|r| {
                let letter = &nb.source(&r.source_id).filter(|s| !s.excluded)?.letter;
                Some(Passage {
                    label: format!("{letter}{}", r.ord),
                    source_id: r.source_id,
                    ord: r.ord,
                    page_start: r.page_start,
                    page_end: r.page_end,
                    section: r.section,
                    text: r.text,
                })
            })
            .collect()
    }
}

pub fn gather(index: &Index, nb: &Notebook, question: &str) -> Result<Context> {
    gather_from_hits(index, nb, rank(index, nb, question)?)
}

/// What to search for a question. A short follow-up such as "why?" has too
/// few words of its own, so the previous question's words are searched too.
pub fn search_text(question: &str, previous: Option<&str>) -> String {
    match previous {
        Some(previous) if query_terms(question).len() < 3 => format!("{previous} {question}"),
        _ => question.to_string(),
    }
}

/// Below this share of a question's terms found in the sources, the question
/// is taken to be in another language than they are.
const KNOWN_SHARE_MIN: f64 = 0.5;
/// Characters of each source's opening shown to the model to name its language.
const SAMPLE_CHARS: usize = 300;

/// When few of the question's words occur in the sources, as when it is asked
/// in another language, the openings of a few sources, from which the model
/// can tell what language to search in.
pub fn foreign_question(index: &Index, nb: &Notebook, search: &str) -> Result<Option<String>> {
    if index.known_share(search)? >= KNOWN_SHARE_MIN {
        return Ok(None);
    }
    let mut openings = Vec::new();
    for source in nb.included().take(3) {
        if let Some(row) = index.chunk(&source.id, 0)? {
            openings.push(row.text.chars().take(SAMPLE_CHARS).collect::<String>());
        }
    }
    Ok((!openings.is_empty()).then(|| openings.join("\n\n")))
}

/// Passages for a question no keyword matches, such as "Summarize the key
/// points": the opening of each section in every source, then the text that
/// follows, with the budget shared evenly between sources.
pub fn gather_overview(index: &Index, nb: &Notebook) -> Result<Context> {
    let mut picker = Picker::default();
    let share = CHAR_BUDGET / nb.included().count().max(1);
    for source in nb.included() {
        let rows = index.chunks_between(&source.id, 0, usize::MAX)?;
        let opens_section = |i: usize| i == 0 || rows[i].section != rows[i - 1].section;
        let order = (0..rows.len())
            .filter(|&i| opens_section(i))
            .chain((0..rows.len()).filter(|&i| !opens_section(i)));
        let mut used = 0;
        for i in order {
            if used + rows[i].text.len() > share {
                continue;
            }
            used += rows[i].text.len();
            picker.add(rows[i].clone());
        }
    }
    Ok(Context {
        passages: picker.into_passages(nb),
        hits: Vec::new(),
    })
}

/// The top hits in the sources answers use, plus the best hit of each source
/// that ranks among the candidates but not in the top `HITS`. Keyword hits are
/// fused with hits by meaning once the search model and vectors are there.
pub fn rank(index: &Index, nb: &Notebook, question: &str) -> Result<Vec<Hit>> {
    // Loading the model takes a second or two; skip it while nothing is embedded.
    let embedder = if index.vector_count()? > 0 {
        Embedder::shared()
    } else {
        None
    };
    let filter = nb
        .sources
        .iter()
        .any(|s| s.excluded)
        .then(|| nb.included().map(|s| s.id.as_str()).collect::<Vec<&str>>());
    let keyword = match &filter {
        Some(ids) => index.search_in(question, CANDIDATES, ids)?,
        None => index.search(question, CANDIDATES)?,
    };
    // A question no keyword matches, such as "Summarize the key points", is
    // answered from an overview of every source, not from the few nearest chunks.
    if keyword.is_empty() {
        return Ok(keyword);
    }
    let dense = match embedder.as_deref() {
        Some(embedder) => {
            index.dense_search(&embedder.query(question)?, CANDIDATES, filter.as_deref())?
        }
        None => Vec::new(),
    };
    Ok(diversify(fuse(&keyword, &dense)))
}

/// Weighted reciprocal rank fusion: each list adds weight / (k + rank) to a
/// chunk's score.
pub fn fuse(keyword: &[Hit], dense: &[Hit]) -> Vec<Hit> {
    if dense.is_empty() {
        return keyword.to_vec();
    }
    let mut scores: HashMap<i64, Hit> = HashMap::new();
    for (list, weight) in [(keyword, KEYWORD_WEIGHT), (dense, 1.0)] {
        for (rank, hit) in list.iter().enumerate() {
            let entry = scores.entry(hit.chunk_id).or_insert(Hit {
                score: 0.0,
                ..hit.clone()
            });
            entry.score += weight / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut fused: Vec<Hit> = scores.into_values().collect();
    fused.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    fused
}

pub fn diversify(candidates: Vec<Hit>) -> Vec<Hit> {
    let mut out: Vec<Hit> = candidates.iter().take(HITS).cloned().collect();
    for hit in candidates.into_iter().skip(HITS) {
        if out.len() == HITS + DIVERSITY_EXTRA {
            break;
        }
        if !out.iter().any(|h| h.source_id == hit.source_id) {
            out.push(hit);
        }
    }
    out
}

/// Ranked hits first, then each hit's neighbors within the same section.
fn gather_from_hits(index: &Index, nb: &Notebook, hits: Vec<Hit>) -> Result<Context> {
    let mut picker = Picker::default();
    let mut hit_rows = Vec::new();
    for hit in &hits {
        if let Some(row) = index.chunk(&hit.source_id, hit.ord)? {
            hit_rows.push(row.clone());
            picker.add(row);
        }
    }
    for row in &hit_rows {
        for ord in [row.ord.checked_sub(1), row.ord.checked_add(1)]
            .into_iter()
            .flatten()
        {
            if let Some(neighbor) = index.chunk(&row.source_id, ord)?
                && neighbor.section == row.section
            {
                picker.add(neighbor);
            }
        }
    }
    Ok(Context {
        passages: picker.into_passages(nb),
        hits,
    })
}

/// Keyword hits first, then the chunks of each picked section, within the budget.
pub fn gather_with_sections(
    index: &Index,
    nb: &Notebook,
    question: &str,
    picks: &[SectionRef],
) -> Result<Context> {
    let hits = rank(index, nb, question)?;
    let mut picker = Picker::default();
    for hit in &hits {
        if let Some(row) = index.chunk(&hit.source_id, hit.ord)? {
            picker.add(row);
        }
    }
    for pick in picks {
        let sections = index.sections(&pick.source_id)?;
        let Some(section) = sections.get(pick.ord) else {
            continue;
        };
        let end = sections[pick.ord + 1..]
            .iter()
            .find(|s| s.depth <= section.depth)
            .map_or(usize::MAX, |s| s.start);
        for row in index.chunks_between(&pick.source_id, section.start, end)? {
            picker.add(row);
        }
    }
    Ok(Context {
        passages: picker.into_passages(nb),
        hits,
    })
}

/// Source list with each source's headings, for the prompt. With
/// `section_ids`, headings carry ids like `A§3` that Deep search can pick.
pub fn outline(
    index: &Index,
    nb: &Notebook,
    source_ids: &[&str],
    section_ids: bool,
) -> Result<String> {
    let mut out = String::new();
    for source in nb
        .sources
        .iter()
        .filter(|s| source_ids.contains(&s.id.as_str()))
    {
        let pages = index
            .pages(&source.id)?
            .map(|p| format!(", {p} pages"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{}. {} ({:?}{pages})\n",
            source.letter, source.title, source.kind
        ));
        let sections = index.sections(&source.id)?;
        let limit = if section_ids {
            usize::MAX
        } else {
            OUTLINE_LINES_PER_SOURCE
        };
        for (ord, s) in sections.iter().enumerate().take(limit) {
            let indent = "  ".repeat(s.depth as usize);
            if section_ids {
                out.push_str(&format!("{indent}{}§{ord} {}\n", source.letter, s.title));
            } else {
                out.push_str(&format!("{indent}{}\n", s.title));
            }
        }
        if sections.len() > limit {
            out.push_str(&format!("  ({} more headings)\n", sections.len() - limit));
        }
    }
    Ok(out)
}

/// "B§12" -> section 12 of source B.
pub fn parse_section_id(nb: &Notebook, id: &str) -> Option<SectionRef> {
    let (letter, ord) = id.trim().split_once('§')?;
    Some(SectionRef {
        source_id: nb.source_by_letter(letter)?.id.clone(),
        ord: ord.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(source: &str, ord: u32) -> Hit {
        Hit {
            chunk_id: i64::from(ord),
            source_id: source.into(),
            ord,
            score: 0.0,
        }
    }

    #[test]
    fn diversify_gives_each_ranked_source_a_passage() {
        let mut candidates: Vec<Hit> = (0..12).map(|i| hit("a", i)).collect();
        candidates.insert(10, hit("b", 0));
        candidates.push(hit("b", 1));
        candidates.push(hit("c", 0));
        let sources: Vec<(String, u32)> = diversify(candidates)
            .into_iter()
            .map(|h| (h.source_id, h.ord))
            .collect();
        let mut expected: Vec<(String, u32)> = (0..8).map(|i| ("a".to_string(), i)).collect();
        expected.push(("b".into(), 0));
        expected.push(("c".into(), 0));
        assert_eq!(sources, expected);
    }

    #[test]
    fn fusion_ranks_chunks_both_searches_found_first() {
        let keyword = [hit("a", 0), hit("a", 1), hit("a", 2)];
        let dense = [hit("a", 2), hit("a", 3)];
        let order: Vec<u32> = fuse(&keyword, &dense).into_iter().map(|h| h.ord).collect();
        assert_eq!(order, [2, 3, 0, 1]);
        assert_eq!(fuse(&keyword, &[]), keyword);
    }

    #[test]
    fn short_follow_ups_search_with_the_previous_question() {
        let previous = Some("How do bowerbirds decorate bowers?");
        assert!(search_text("why blue?", previous).starts_with("How do bowerbirds"));
        assert_eq!(
            search_text("Which colors do satin bowerbirds prefer?", previous),
            "Which colors do satin bowerbirds prefer?"
        );
        assert_eq!(search_text("why?", None), "why?");
    }

    #[test]
    fn overview_reads_every_source_when_no_keyword_matches() {
        let dir = std::env::temp_dir().join(format!("bowerbird-overview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        // Longer than the whole budget, so without a fair share it would crowd out B.
        let long = "Satin bowerbirds paint their bowers with chewed plants.\n\n".repeat(1_000);
        nb.add_text(
            "Satin",
            &format!("# Bowers\n{long}# Diet\nFruit and insects.\n"),
        )
        .unwrap();
        nb.add_text("Vogelkop", "Moss mats sorted by color.")
            .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        assert!(
            gather(&index, &nb, "Summarize the key points")
                .unwrap()
                .passages
                .is_empty()
        );
        let passages = gather_overview(&index, &nb).unwrap().passages;
        assert!(
            passages.iter().any(|p| p.label == "B0"),
            "every source gets passages"
        );
        assert!(
            passages
                .iter()
                .any(|p| p.text.contains("Fruit and insects")),
            "each section's opening comes before the rest of a long section"
        );
        let satin: usize = passages
            .iter()
            .filter(|p| p.label.starts_with('A'))
            .map(|p| p.text.len())
            .sum();
        assert!(satin <= CHAR_BUDGET / 2);
    }

    #[test]
    fn switched_off_sources_are_not_searched_or_summarized() {
        let dir = std::env::temp_dir().join(format!("bowerbird-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        // More matching text in A than the whole budget, so an unfiltered
        // search or overview would hold only A.
        let a = nb
            .add_text(
                "Treaties",
                &"The treaty of Westphalia ended the war.\n\n".repeat(1_200),
            )
            .unwrap();
        nb.add_text("Utrecht", "The treaty of Utrecht ended another war.")
            .unwrap();
        nb.set_excluded(&a.id, true).unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let passages = gather(&index, &nb, "treaty war").unwrap().passages;
        assert!(!passages.is_empty());
        assert!(passages.iter().all(|p| p.label.starts_with('B')));
        let overview = gather_overview(&index, &nb).unwrap().passages;
        assert!(!overview.is_empty() && overview.iter().all(|p| p.label.starts_with('B')));
    }
}
