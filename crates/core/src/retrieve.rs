//! Picks the passages a question gets answered from: keyword hits fused with
//! hits by meaning, their neighbors in the same section, and for Deep search,
//! whole sections the model asked for.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::Result;
use crate::embed::Embedder;
use crate::index::{ChunkRow, Hit, Index, query_terms};
use crate::notebook::{Notebook, Source};

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

/// Passages for a question: its hits, each with its neighbors. A question no
/// keyword matches, such as "Summarize the key points" or one worded unlike
/// the sources, also gets an overview of every source in the room the
/// passages nearest in meaning leave.
pub fn gather(index: &Index, nb: &Notebook, question: &str) -> Result<Context> {
    let (hits, keyword_matched) = rank(index, nb, question)?;
    let mut picker = Picker::default();
    pick_hits(index, &hits, &mut picker)?;
    if !keyword_matched {
        pick_overview(index, nb, &mut picker)?;
    }
    Ok(Context {
        passages: picker.into_passages(nb),
        hits,
    })
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

/// An overview of every source: the opening of each section, then the text
/// that follows, with the budget shared evenly between sources.
pub fn gather_overview(index: &Index, nb: &Notebook) -> Result<Context> {
    let mut picker = Picker::default();
    pick_overview(index, nb, &mut picker)?;
    Ok(Context {
        passages: picker.into_passages(nb),
        hits: Vec::new(),
    })
}

/// Adds an overview of every source in the budget `picker` has left.
fn pick_overview(index: &Index, nb: &Notebook, picker: &mut Picker) -> Result<()> {
    let share = (CHAR_BUDGET - picker.used) / nb.included().count().max(1);
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
    Ok(())
}

/// The top hits in the sources answers use, plus the best hit of each source
/// that ranks among the candidates but not in the top `HITS`, and whether any
/// keyword matched. Keyword hits are fused with hits by meaning once the
/// search model and vectors are there.
pub fn rank(index: &Index, nb: &Notebook, question: &str) -> Result<(Vec<Hit>, bool)> {
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
    let dense = match embedder.as_deref() {
        Some(embedder) => {
            index.dense_search(&embedder.query(question)?, CANDIDATES, filter.as_deref())?
        }
        None => Vec::new(),
    };
    Ok((diversify(fuse(&keyword, &dense)), !keyword.is_empty()))
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
fn pick_hits(index: &Index, hits: &[Hit], picker: &mut Picker) -> Result<()> {
    let mut hit_rows = Vec::new();
    for hit in hits {
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
    Ok(())
}

/// Keyword hits first, then the chunks of each picked section, within the budget.
pub fn gather_with_sections(
    index: &Index,
    nb: &Notebook,
    question: &str,
    picks: &[SectionRef],
) -> Result<Context> {
    let (hits, _) = rank(index, nb, question)?;
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

/// One passage hit returned by interactive notebook/library search.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PassageHit {
    pub label: String,
    pub source_id: String,
    pub source_title: String,
    pub letter: String,
    pub ord: u32,
    pub page_start: Option<u32>,
    pub section: String,
    pub snippet: String,
    pub score: f64,
}

/// Similarity below which a passage found only by meaning is left out of
/// interactive search. The search model always has a nearest passage, so
/// without a floor every query finds something. Measured on a notebook of 12
/// English and Serbian sources: the best passage scored 0.32 to 0.76 for 14
/// keyword and concept queries, and 0.08 to 0.24 for 13 off-topic and
/// nonsense ones. Answers don't use it: `rank` wants the nearest passages
/// whatever their score.
const MEANING_FLOOR: f64 = 0.28;

/// Characters of a passage shown with a search hit.
const SNIPPET_CHARS: usize = 220;

/// The query as a vector for `search_passages`, or None while the notebook
/// has no vectors or the search model isn't downloaded.
pub fn query_vector(index: &Index, query: &str) -> Result<Option<Vec<f32>>> {
    // Loading the model takes a second or two; skip it while nothing is embedded.
    if index.vector_count()? == 0 {
        return Ok(None);
    }
    Embedder::shared()
        .map(|embedder| embedder.query(query))
        .transpose()
}

/// Interactive search over a notebook's passages, without a model call:
/// keyword hits fused with the passages near `vector` in meaning. `vector`
/// comes from `query_vector`, which a search over several notebooks computes
/// once. When neither finds anything, passages that contain the query as
/// typed are returned, which finds part of a word.
pub fn search_passages(
    index: &Index,
    nb: &Notebook,
    query: &str,
    vector: Option<&[f32]>,
    limit: usize,
    include_excluded: bool,
) -> Result<Vec<PassageHit>> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let filter = (!include_excluded && nb.sources.iter().any(|s| s.excluded))
        .then(|| nb.included().map(|s| s.id.as_str()).collect::<Vec<&str>>());
    let fetch_limit = limit.max(CANDIDATES);
    let keyword = match &filter {
        Some(ids) => index.search_in(query, fetch_limit, ids)?,
        None => index.search(query, fetch_limit)?,
    };
    let mut dense = match vector {
        Some(vector) => index.dense_search(vector, fetch_limit, filter.as_deref())?,
        None => Vec::new(),
    };
    dense.retain(|hit| hit.score >= MEANING_FLOOR);
    let mut out = Vec::new();
    for hit in fuse(&keyword, &dense).into_iter().take(limit) {
        let Some(source) = nb.source(&hit.source_id) else {
            continue;
        };
        if let Some(row) = index.chunk(&hit.source_id, hit.ord)? {
            out.push(passage_hit(source, row, query, hit.score));
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }
    let needle = query.to_lowercase();
    let searched = nb
        .sources
        .iter()
        .filter(|s| include_excluded || !s.excluded);
    for source in searched {
        for row in index.chunks_between(&source.id, 0, usize::MAX)? {
            if row.text.to_lowercase().contains(&needle)
                || row.section.to_lowercase().contains(&needle)
            {
                out.push(passage_hit(source, row, query, 0.1));
                if out.len() == limit {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn passage_hit(source: &Source, row: ChunkRow, query: &str, score: f64) -> PassageHit {
    PassageHit {
        label: format!("{}{}", source.letter, row.ord),
        source_id: source.id.clone(),
        source_title: source.title.clone(),
        letter: source.letter.clone(),
        ord: row.ord,
        page_start: row.page_start,
        snippet: snippet_around(&row.text, query, SNIPPET_CHARS),
        section: row.section,
        score,
    }
}

/// Short single-line excerpt centered around the first match of `query` (or
/// its first matching term), falling back to the start of `text`.
pub fn snippet_around(text: &str, query: &str, max_chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let total = flat.chars().count();
    if total <= max_chars {
        return flat;
    }
    let lower = flat.to_lowercase();
    let needle = query.trim().to_lowercase();
    let match_byte = (!needle.is_empty())
        .then(|| lower.find(&needle))
        .flatten()
        .or_else(|| {
            query_terms(query)
                .into_iter()
                .find_map(|term| lower.find(&term))
        });
    let match_char = match_byte.map_or(0, |b| lower[..b].chars().count());
    let start_char = match_char.saturating_sub(max_chars / 3);
    let end_char = (start_char + max_chars).min(total);
    let start_char = end_char.saturating_sub(max_chars);
    let excerpt: String = flat
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .collect();
    let mut out = String::new();
    if start_char > 0 {
        out.push('…');
    }
    out.push_str(excerpt.trim());
    if end_char < total {
        out.push('…');
    }
    out
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
        let passages = gather(&index, &nb, "Summarize the key points")
            .unwrap()
            .passages;
        assert_eq!(passages, gather_overview(&index, &nb).unwrap().passages);
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

    /// Downloads the search model (about 200 MB), so it runs only with
    /// BOWERBIRD_LIVE=1.
    #[tokio::test]
    async fn questions_worded_unlike_the_sources_are_found_by_meaning() {
        if std::env::var("BOWERBIRD_LIVE").as_deref() != Ok("1") {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to download the search model");
            return;
        }
        crate::embed::download_for_tests().await;
        let dir = std::env::temp_dir().join(format!("bowerbird-meaning-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text(
            "Tax",
            "Value added tax is charged at each stage of production.",
        )
        .unwrap();
        let bowers = nb
            .add_text(
                "Bowers",
                "The male gathers blue bottle caps and berries to decorate his bower.",
            )
            .unwrap();
        let (mut index, _) = Index::open_synced(&nb).unwrap();
        let embedder = Embedder::shared().unwrap();
        let rows = index.unembedded(10).unwrap();
        let vectors = embedder.documents(&rows).unwrap();
        index.store_vectors(&rows, &vectors).unwrap();

        let question = "Which colours does the bird collect?";
        assert!(index.search(question, CANDIDATES).unwrap().is_empty());
        let (hits, keyword_matched) = rank(&index, &nb, question).unwrap();
        assert!(!keyword_matched);
        assert_eq!(
            hits.first().map(|h| h.source_id.as_str()),
            Some(bowers.id.as_str())
        );
        let passages = gather(&index, &nb, question).unwrap().passages;
        assert!(
            passages.iter().any(|p| p.label == "A0"),
            "the overview still comes along"
        );
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

    #[test]
    fn interactive_passage_search_finds_keywords_and_partial_substrings() {
        let dir = std::env::temp_dir().join(format!("bowerbird-search-p-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text(
            "Satin",
            "# Courtship\nThe satin bowerbird gathers ultramarine bottle caps.",
        )
        .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let hits = search_passages(&index, &nb, "ultramarine", None, 5, false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "A0");
        assert!(hits[0].snippet.contains("ultramarine"));
        // Substring fallback finds fragments FTS5 tokenization doesn't match.
        let sub = search_passages(&index, &nb, "ramari", None, 5, false).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].label, "A0");
    }

    /// Downloads the search model (about 200 MB), so it runs only with
    /// BOWERBIRD_LIVE=1.
    #[tokio::test]
    async fn interactive_search_by_meaning_leaves_out_passages_far_from_the_query() {
        if std::env::var("BOWERBIRD_LIVE").as_deref() != Ok("1") {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to download the search model");
            return;
        }
        crate::embed::download_for_tests().await;
        let dir = std::env::temp_dir().join(format!("bowerbird-floor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text(
            "Tax",
            "Value added tax is charged at each stage of production.",
        )
        .unwrap();
        nb.add_text(
            "Bowers",
            "The male gathers ultramarine bottle caps and berries to decorate his bower.",
        )
        .unwrap();
        let (mut index, _) = Index::open_synced(&nb).unwrap();
        let rows = index.unembedded(10).unwrap();
        let vectors = Embedder::shared().unwrap().documents(&rows).unwrap();
        index.store_vectors(&rows, &vectors).unwrap();
        let labels = |query: &str| -> Vec<String> {
            let vector = query_vector(&index, query).unwrap();
            assert!(vector.is_some(), "the notebook has vectors");
            search_passages(&index, &nb, query, vector.as_deref(), 5, false)
                .unwrap()
                .into_iter()
                .map(|hit| hit.label)
                .collect()
        };
        // No word in common with the source: only its meaning finds it, and
        // the tax passage is too far from the question to come along.
        assert_eq!(labels("Which colours does the bird collect?"), ["B0"]);
        assert!(labels("asdfgh").is_empty());
        // Part of a word matches no keyword and nothing in meaning.
        assert_eq!(labels("ramari"), ["B0"]);
    }
}
