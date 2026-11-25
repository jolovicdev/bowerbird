//! Searching the index: by keyword through FTS5, and by meaning through
//! the vectors stored beside the chunks.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};

use super::{ChunkRow, Hit, Index, PRUNE_FROM_CHUNKS, row_to_chunk};
use crate::{Error, Result};

/// Share of chunks above which a term counts as too common to search.
const COMMON_SHARE: f64 = 0.1;

/// Common English words that only add noise to keyword queries.
const STOPWORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as",
    "at", "be", "because", "been", "before", "being", "between", "both", "but", "by", "can",
    "could", "did", "do", "does", "doing", "during", "each", "few", "for", "from", "further",
    "had", "has", "have", "having", "he", "her", "here", "hers", "him", "his", "how", "i", "if",
    "in", "into", "is", "it", "its", "just", "me", "more", "most", "my", "no", "nor", "not", "now",
    "of", "off", "on", "once", "only", "or", "other", "our", "out", "over", "own", "same", "she",
    "should", "so", "some", "such", "than", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "those", "through", "to", "too", "under", "until", "up", "very",
    "was", "we", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will",
    "with", "would", "you", "your",
];

impl Index {
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        let Some(fts) = self.fts_query(query)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            // Rank inside FTS5 first so only the top rows get joined.
            "SELECT c.id, c.source_id, c.ord, top.score
             FROM (SELECT rowid, bm25(chunks_fts, 1.0, 0.5) AS score
                   FROM chunks_fts WHERE chunks_fts MATCH ?1
                   ORDER BY score, rowid LIMIT ?2) AS top
             JOIN chunks c ON c.id = top.rowid
             ORDER BY top.score, c.id",
        )?;
        let hits = stmt
            .query_map(params![fts, limit], hit_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(hits)
    }

    /// Like `search`, limited to some sources. Every match is joined before
    /// ranking, which is slower on large notebooks, so callers use it only
    /// when some sources are left out.
    pub fn search_in(&self, query: &str, limit: usize, source_ids: &[&str]) -> Result<Vec<Hit>> {
        let Some(fts) = self.fts_query(query)? else {
            return Ok(Vec::new());
        };
        let ids = serde_json::to_string(source_ids).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id, c.source_id, c.ord, bm25(chunks_fts, 1.0, 0.5) AS score
             FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid
             WHERE chunks_fts MATCH ?1 AND c.source_id IN (SELECT value FROM json_each(?3))
             ORDER BY score, c.id LIMIT ?2",
        )?;
        let hits = stmt
            .query_map(params![fts, limit, ids], hit_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(hits)
    }

    /// The FTS5 query for a question's terms, or None when it has no terms.
    fn fts_query(&self, query: &str) -> Result<Option<String>> {
        let mut terms = query_terms(query);
        if terms.is_empty() {
            return Ok(None);
        }
        if self.chunk_total >= PRUNE_FROM_CHUNKS {
            let max_rows = (self.chunk_total as f64 * COMMON_SHARE) as i64;
            terms = self.drop_common_terms(terms, max_rows)?;
        }
        Ok(Some(
            terms
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(" OR "),
        ))
    }

    /// Share of a question's search terms that occur anywhere in the index.
    /// Low when the question is in another language than the sources.
    pub fn known_share(&self, query: &str) -> Result<f64> {
        let terms = query_terms(query);
        if terms.is_empty() || self.chunk_total == 0 {
            return Ok(1.0);
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM chunks_fts WHERE chunks_fts MATCH ?1)")?;
        let mut known = 0;
        for term in &terms {
            if stmt.query_row([format!("\"{term}\"")], |r| r.get::<_, bool>(0))? {
                known += 1;
            }
        }
        Ok(f64::from(known) / terms.len() as f64)
    }

    /// Leaves out terms matching `max_rows` chunks or more. Scoring their long
    /// posting lists dominates query time on large notebooks, while their low
    /// IDF barely moves the ranking. Keeps every term if all are that common.
    fn drop_common_terms(&self, terms: Vec<String>, max_rows: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT count(*) FROM (SELECT 1 FROM chunks_fts WHERE chunks_fts MATCH ?1 LIMIT ?2)",
        )?;
        let mut kept = Vec::new();
        for term in &terms {
            let rows: i64 =
                stmt.query_row(params![format!("\"{term}\""), max_rows], |r| r.get(0))?;
            if rows < max_rows {
                kept.push(term.clone());
            }
        }
        Ok(if kept.is_empty() { terms } else { kept })
    }

    /// Chunks without a vector, in the order they were indexed.
    pub fn unembedded(&self, limit: usize) -> Result<Vec<ChunkRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, source_id, ord, start, page_start, page_end, section, text
             FROM chunks c WHERE NOT EXISTS
               (SELECT 1 FROM vectors v WHERE v.source_id = c.source_id AND v.ord = c.ord)
             ORDER BY id LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], row_to_chunk)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Saves each row's vector, skipping rows whose source was removed
    /// while they were being embedded.
    pub fn store_vectors(&mut self, rows: &[ChunkRow], vectors: &[Vec<f32>]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO vectors (source_id, ord, vector)
                 SELECT ?1, ?2, ?3 WHERE EXISTS
                   (SELECT 1 FROM chunks WHERE source_id = ?1 AND ord = ?2)",
            )?;
            for (row, vector) in rows.iter().zip(vectors) {
                let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
                insert.execute(params![row.source_id, row.ord, bytes])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn vector_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM vectors", [], |r| r.get(0))?)
    }

    /// The chunks closest in meaning to a normalized query vector, among
    /// those embedded so far. `source_ids` limits the search to some sources.
    // Every vector is read and scored on each search. Optimized and warm, on
    // an 8-core machine: 2 ms at 10k chunks, 41 ms at 100k and 412 ms at
    // 500k, where 352 ms of it is reading rows of 3 KB each. Reading through
    // a memory map halves the 500k figure. Past a few hundred thousand
    // chunks, the step after that is a compact copy of the vectors to scan
    // (8-bit, packed per source), with the nearest few scored exactly from
    // this table. `vector_search_at_scale` below times all of this.
    pub fn dense_search(
        &self,
        query: &[f32],
        limit: usize,
        source_ids: Option<&[&str]>,
    ) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Named sources are read through the primary key, so the sources
        // left out cost nothing.
        let mut stmt;
        let mut rows = match source_ids {
            Some(ids) => {
                let ids = serde_json::to_string(ids).map_err(|e| Error::Invalid(e.to_string()))?;
                stmt = self.conn.prepare_cached(
                    "SELECT source_id, ord, vector FROM vectors
                     WHERE source_id IN (SELECT value FROM json_each(?1))",
                )?;
                stmt.query([ids])?
            }
            None => {
                stmt = self
                    .conn
                    .prepare_cached("SELECT source_id, ord, vector FROM vectors")?;
                stmt.query([])?
            }
        };
        // Only the best few are kept, and only they are copied out of the
        // row: vectors exist only for stored chunks, so chunk ids are looked
        // up for those few alone.
        let mut best: Vec<Near> = Vec::with_capacity(limit);
        let mut worst = 0;
        while let Some(row) = rows.next()? {
            let source_id = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
            let ord: u32 = row.get(1)?;
            let vector = row.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?;
            let score = dot(vector, query);
            if best.len() < limit {
                best.push((score, source_id.to_string(), ord));
            } else if nearer((score, source_id, ord), key(&best[worst])).is_lt() {
                best[worst] = (score, source_id.to_string(), ord);
            } else {
                continue;
            }
            worst = (0..best.len())
                .max_by(|&a, &b| nearer(key(&best[a]), key(&best[b])))
                .unwrap_or(0);
        }
        best.sort_by(|a, b| nearer(key(a), key(b)));
        let mut chunk_id = self
            .conn
            .prepare_cached("SELECT id FROM chunks WHERE source_id = ?1 AND ord = ?2")?;
        let mut hits = Vec::with_capacity(best.len());
        for (score, source_id, ord) in best {
            if let Some(id) = chunk_id
                .query_row(params![source_id, ord], |r| r.get(0))
                .optional()?
            {
                hits.push(Hit {
                    chunk_id: id,
                    source_id,
                    ord,
                    score: f64::from(score),
                });
            }
        }
        Ok(hits)
    }
}

/// A vector's score, source and position in it.
type Near = (f32, String, u32);

fn key(near: &Near) -> (f32, &str, u32) {
    (near.0, &near.1, near.2)
}

/// Orders the higher score first. Equal scores go by source and position, so
/// the same search gives the same order every time.
fn nearer(a: (f32, &str, u32), b: (f32, &str, u32)) -> std::cmp::Ordering {
    b.0.total_cmp(&a.0)
        .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
}

/// The dot product of a stored vector, which is little-endian `f32` bytes,
/// and `query`. Eight running totals instead of one: a single total fixes
/// the order of the additions, and the compiler then can't do them side by
/// side.
fn dot(stored: &[u8], query: &[f32]) -> f32 {
    const LANES: usize = 8;
    let stored = stored.as_chunks::<4>().0;
    let shared = stored.len().min(query.len());
    let (stored, stored_tail) = stored[..shared].as_chunks::<LANES>();
    let (query, query_tail) = query[..shared].as_chunks::<LANES>();
    let mut totals = [0f32; LANES];
    for (bytes, q) in stored.iter().zip(query) {
        for lane in 0..LANES {
            totals[lane] += f32::from_le_bytes(bytes[lane]) * q[lane];
        }
    }
    let tail: f32 = stored_tail
        .iter()
        .zip(query_tail)
        .map(|(bytes, q)| f32::from_le_bytes(*bytes) * q)
        .sum();
    totals.iter().sum::<f32>() + tail
}

/// The question's distinct non-stopword terms, which get ORed in the FTS5
/// query. They are alphanumeric only, so quoting them is enough to escape them.
fn hit_from_row(r: &rusqlite::Row) -> rusqlite::Result<Hit> {
    Ok(Hit {
        chunk_id: r.get(0)?,
        source_id: r.get(1)?,
        ord: r.get(2)?,
        score: -r.get::<_, f64>(3)?,
    })
}

pub fn query_terms(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(&w.as_str()))
        .filter(|w| w.chars().count() > 1 || w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::*;
    use crate::notebook::Notebook;

    #[test]
    fn vectors_rank_by_similarity_and_go_with_their_source() {
        let dir = temp_dir("vectors");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        let satin = nb
            .add_text("Satin", "# One\nBlue caps.\n# Two\nBlue straws.\n")
            .unwrap();
        let vogelkop = nb.add_text("Vogelkop", "Moss mats.").unwrap();
        let (mut index, _) = Index::open_synced(&nb).unwrap();
        let rows = index.unembedded(10).unwrap();
        assert_eq!(rows.len(), 3);
        let vectors = vec![vec![1.0, 0.0], vec![0.6, 0.8], vec![0.0, 1.0]];
        index.store_vectors(&rows, &vectors).unwrap();
        assert!(index.unembedded(10).unwrap().is_empty());

        let near = |q: &[f32], ids: Option<&[&str]>| -> Vec<(String, u32)> {
            index
                .dense_search(q, 3, ids)
                .unwrap()
                .into_iter()
                .map(|h| (h.source_id, h.ord))
                .collect()
        };
        assert_eq!(
            near(&[0.0, 1.0], None),
            [
                (vogelkop.id.clone(), 0),
                (satin.id.clone(), 1),
                (satin.id.clone(), 0)
            ]
        );
        assert_eq!(
            near(&[0.0, 1.0], Some(&[satin.id.as_str()])),
            [(satin.id.clone(), 1), (satin.id.clone(), 0)]
        );
        let best_two: Vec<(String, u32)> = index
            .dense_search(&[0.0, 1.0], 2, None)
            .unwrap()
            .into_iter()
            .map(|h| (h.source_id, h.ord))
            .collect();
        assert_eq!(best_two, [(vogelkop.id.clone(), 0), (satin.id.clone(), 1)]);

        index.remove(&satin.id).unwrap();
        assert_eq!(index.vector_count().unwrap(), 1);
    }

    #[test]
    fn query_terms_drop_stopwords_and_punctuation() {
        assert_eq!(
            query_terms("What is the 3% rule, again?"),
            vec!["3", "rule"]
        );
        assert!(query_terms("the of and").is_empty());
    }

    #[test]
    fn common_terms_are_dropped_unless_nothing_would_remain() {
        let dir = temp_dir("common-terms");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        let sections: String = (0..6)
            .map(|i| format!("# Part {i}\nThe bower is built of sticks.\n"))
            .collect();
        nb.add_text(
            "Bowers",
            &format!("{sections}# Satin\nThe satin bower is blue.\n"),
        )
        .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let terms = |ts: &[&str]| ts.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            index
                .drop_common_terms(terms(&["bower", "satin"]), 3)
                .unwrap(),
            terms(&["satin"])
        );
        assert_eq!(
            index.drop_common_terms(terms(&["bower"]), 3).unwrap(),
            terms(&["bower"])
        );
    }

    #[test]
    fn a_question_in_another_language_shares_few_terms_with_the_sources() {
        let dir = temp_dir("known-share");
        let mut nb = Notebook::open_or_create(&dir, "History").unwrap();
        nb.add_text(
            "Westphalia",
            "The Peace of Westphalia ended the Thirty Years' War in 1648 and weakened the Emperor.",
        )
        .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let english = index
            .known_share("What did the Peace of Westphalia end?")
            .unwrap();
        let serbian = index
            .known_share("Koje su bile glavne posledice Vestfalskog mira za carstvo?")
            .unwrap();
        assert!(english > 0.9, "{english}");
        assert!(serbian < 0.2, "{serbian}");
    }

    /// Unit vectors from a seed, so two runs search the same index.
    fn seeded_vectors(seed: u64, dims: usize) -> impl Iterator<Item = Vec<f32>> {
        let mut state = seed;
        std::iter::repeat_with(move || {
            let mut v: Vec<f32> = (0..dims)
                .map(|_| {
                    // A 64-bit linear congruential step; its top bits are the random ones.
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5
                })
                .collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= norm);
            v
        })
    }

    /// Stores vectors for sources named `s0`, `s1`, …, each `per_source` long,
    /// until the index holds `total`. No chunks are stored, so searches find
    /// the nearest vectors and return no hits, which is all a timing needs.
    fn grow(
        index: &mut Index,
        vectors: &mut impl Iterator<Item = Vec<f32>>,
        total: usize,
        per_source: usize,
    ) {
        let have = index.vector_count().unwrap() as usize;
        let tx = index.conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare("INSERT INTO vectors (source_id, ord, vector) VALUES (?1, ?2, ?3)")
                .unwrap();
            for (n, vector) in (have..total).zip(vectors) {
                let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
                insert
                    .execute(params![
                        format!("s{}", n / per_source),
                        n % per_source,
                        bytes
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    #[test]
    fn the_nearest_vectors_match_a_plain_scan_in_order_and_score() {
        let dir = temp_dir("vector-exact");
        let mut nb = Notebook::open_or_create(&dir, "Many").unwrap();
        for n in 0..120 {
            nb.add_text(&format!("Source {n}"), &format!("Text number {n}."))
                .unwrap();
        }
        let (mut index, _) = Index::open_synced(&nb).unwrap();
        let rows = index.unembedded(1000).unwrap();
        assert_eq!(rows.len(), 120);
        let vectors: Vec<Vec<f32>> = seeded_vectors(3, 768).take(rows.len()).collect();
        index.store_vectors(&rows, &vectors).unwrap();
        let query = seeded_vectors(5, 768).next().unwrap();

        // The plain scan: every score worked out in double precision, then sorted.
        let mut plain: Vec<(f64, &str, u32)> = rows
            .iter()
            .zip(&vectors)
            .map(|(row, vector)| {
                let score = vector
                    .iter()
                    .zip(&query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum();
                (score, row.source_id.as_str(), row.ord)
            })
            .collect();
        plain.sort_by(|a, b| b.0.total_cmp(&a.0));

        let found = index.dense_search(&query, 10, None).unwrap();
        assert_eq!(found.len(), 10);
        for (hit, (score, source_id, ord)) in found.iter().zip(&plain) {
            assert_eq!((hit.source_id.as_str(), hit.ord), (*source_id, *ord));
            assert!((hit.score - score).abs() < 1e-5, "{} vs {score}", hit.score);
        }

        // Limited to every third source, it is the plain scan of those alone.
        let some: Vec<&str> = nb
            .sources
            .iter()
            .step_by(3)
            .map(|s| s.id.as_str())
            .collect();
        let found: Vec<(String, u32)> = index
            .dense_search(&query, 10, Some(&some))
            .unwrap()
            .into_iter()
            .map(|hit| (hit.source_id, hit.ord))
            .collect();
        let expected: Vec<(String, u32)> = plain
            .iter()
            .filter(|(_, source_id, _)| some.contains(source_id))
            .take(10)
            .map(|(_, source_id, ord)| (source_id.to_string(), *ord))
            .collect();
        assert_eq!(found, expected);

        assert!(index.dense_search(&query, 0, None).unwrap().is_empty());
    }

    #[test]
    fn a_dot_product_is_the_same_summed_in_lanes_or_in_one_go() {
        for dims in [1, 7, 8, 13, 768] {
            let stored = seeded_vectors(1, dims).next().unwrap();
            let query = seeded_vectors(2, dims).next().unwrap();
            let bytes: Vec<u8> = stored.iter().flat_map(|x| x.to_le_bytes()).collect();
            let plain: f32 = stored.iter().zip(&query).map(|(a, b)| a * b).sum();
            assert!(
                (dot(&bytes, &query) - plain).abs() < 1e-6,
                "{dims} dimensions"
            );
        }
    }

    fn median_ms(mut run: impl FnMut()) -> f64 {
        let mut times: Vec<f64> = (0..5)
            .map(|_| {
                let started = std::time::Instant::now();
                run();
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    }

    /// Times the search by meaning as an index grows, and the bare read of
    /// its rows, which no scoring loop can beat. Run by hand, optimized:
    ///
    /// ```text
    /// cargo test -p bowerbird-core --release --lib -- --ignored vector_search_at_scale --nocapture
    /// ```
    #[test]
    #[ignore = "a benchmark: writes about 2 GB and takes a few minutes"]
    fn vector_search_at_scale() {
        const DIMS: usize = 768;
        let dir = temp_dir("vector-scale");
        std::fs::create_dir_all(&dir).unwrap();
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let mut vectors = seeded_vectors(7, DIMS);
        let query = seeded_vectors(11, DIMS).next().unwrap();
        for total in [10_000, 100_000, 500_000] {
            grow(&mut index, &mut vectors, total, 1000);
            let read = median_ms(|| {
                let mut stmt = index
                    .conn
                    .prepare_cached("SELECT source_id, ord, vector FROM vectors")
                    .unwrap();
                let mut rows = stmt.query([]).unwrap();
                let mut bytes = 0;
                while let Some(row) = rows.next().unwrap() {
                    bytes += row.get_ref(2).unwrap().as_blob().unwrap().len();
                }
                assert_eq!(bytes, total * DIMS * 4);
            });
            let search = median_ms(|| {
                index.dense_search(&query, 24, None).unwrap();
            });
            let one_in_ten: Vec<String> = (0..total / 1000)
                .step_by(10)
                .map(|n| format!("s{n}"))
                .collect();
            let some: Vec<&str> = one_in_ten.iter().map(String::as_str).collect();
            let filtered = median_ms(|| {
                index.dense_search(&query, 24, Some(&some)).unwrap();
            });
            println!(
                "{total:>7} vectors: search {search:6.1} ms, of which reading rows {read:6.1} ms; a tenth of the sources {filtered:6.1} ms"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
