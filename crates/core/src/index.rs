//! SQLite cache of a notebook's chunks with an FTS5 keyword index. Everything
//! here can be rebuilt from the notebook folder.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::chunk::{self, Chunk};
use crate::doc::{Doc, Figure, Section};
use crate::notebook::{Notebook, SourceKind};
use crate::{Error, Result};

/// Bumped when what extraction stores changes; an index from another version
/// is dropped and rebuilt from the notebook.
const VERSION: i64 = 3;
const DROP: &str = "
DROP TABLE IF EXISTS chunks_fts;
DROP TABLE IF EXISTS vectors;
DROP TABLE IF EXISTS chunks;
DROP TABLE IF EXISTS figures;
DROP TABLE IF EXISTS sections;
DROP TABLE IF EXISTS sources;
";
const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,
    pages INTEGER
);
CREATE TABLE IF NOT EXISTS sections (
    source_id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    depth INTEGER NOT NULL,
    title TEXT NOT NULL,
    start INTEGER NOT NULL,
    PRIMARY KEY (source_id, ord)
);
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    source_id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    page_start INTEGER,
    page_end INTEGER,
    section TEXT NOT NULL,
    text TEXT NOT NULL,
    UNIQUE (source_id, ord)
);
CREATE TABLE IF NOT EXISTS figures (
    source_id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    start INTEGER NOT NULL,
    src TEXT NOT NULL,
    file TEXT NOT NULL,
    caption TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    PRIMARY KEY (source_id, ord)
);
-- Keyed by source and position rather than chunk id: source ids are content
-- hashes, so a key always names the same text, even across a rebuild.
CREATE TABLE IF NOT EXISTS vectors (
    source_id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY (source_id, ord)
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text, section,
    content = 'chunks', content_rowid = 'id',
    tokenize = 'porter unicode61 remove_diacritics 2'
);
";

/// Notebooks smaller than this search every query term: posting lists are
/// short enough that pruning buys nothing.
const PRUNE_FROM_CHUNKS: u64 = 20_000;
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

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub chunk_id: i64,
    pub source_id: String,
    pub ord: u32,
    /// Higher is better.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRow {
    pub id: i64,
    pub source_id: String,
    pub ord: u32,
    pub start: usize,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    pub section: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    /// (source id, chunk count)
    pub added: Vec<(String, usize)>,
    pub removed: usize,
    /// (source id, error message)
    pub failed: Vec<(String, String)>,
    /// Scanned PDFs waiting for OCR, also listed in `failed`.
    pub scanned: Vec<String>,
}

pub struct Index {
    conn: Connection,
    chunk_total: u64,
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != VERSION {
            conn.execute_batch(DROP)?;
            conn.pragma_update(None, "user_version", VERSION)?;
        }
        conn.execute_batch(SCHEMA)?;
        let chunk_total = conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?;
        Ok(Index { conn, chunk_total })
    }

    /// Opens the notebook's index and brings it up to date with its sources.
    pub fn open_synced(nb: &Notebook) -> Result<(Index, SyncReport)> {
        let mut index = Index::open(&nb.index_path())?;
        let report = index.sync(nb)?;
        Ok((index, report))
    }

    pub fn source_ids(&self) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM sources")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Extracts sources the index hasn't seen and drops ones the notebook no
    /// longer lists. Source ids are content hashes, so a changed file is a new id.
    pub fn sync(&mut self, nb: &Notebook) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let indexed = self.source_ids()?;
        let wanted: HashSet<&str> = nb.sources.iter().map(|s| s.id.as_str()).collect();
        for stale in indexed.iter().filter(|id| !wanted.contains(id.as_str())) {
            self.remove(stale)?;
            report.removed += 1;
        }
        for source in nb.sources.iter().filter(|s| !indexed.contains(&s.id)) {
            match nb.extract(source) {
                Ok(doc) => {
                    let markdown = matches!(
                        source.kind,
                        SourceKind::Markdown
                            | SourceKind::Web
                            | SourceKind::Docx
                            | SourceKind::Epub
                            | SourceKind::Youtube
                            | SourceKind::Audio
                    );
                    let chunks = chunk::split(&doc, markdown);
                    self.insert(&source.id, &doc, &chunks)?;
                    report.added.push((source.id.clone(), chunks.len()));
                }
                Err(e) => {
                    if matches!(e, Error::Scanned) {
                        report.scanned.push(source.id.clone());
                    }
                    report.failed.push((source.id.clone(), e.to_string()));
                }
            }
        }
        Ok(report)
    }

    pub fn insert(&mut self, source_id: &str, doc: &Doc, chunks: &[Chunk]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sources (id, pages) VALUES (?1, ?2)",
            params![
                source_id,
                (!doc.page_starts.is_empty()).then_some(doc.page_starts.len())
            ],
        )?;
        {
            let mut section = tx.prepare("INSERT INTO sections (source_id, ord, depth, title, start) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for (ord, s) in doc.sections.iter().enumerate() {
                section.execute(params![source_id, ord, s.depth, s.title, s.start])?;
            }
            let mut figure = tx.prepare(
                "INSERT INTO figures (source_id, ord, start, src, file, caption, width, height)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (ord, f) in doc.figures.iter().enumerate() {
                figure.execute(params![
                    source_id, ord, f.start, f.src, f.file, f.caption, f.width, f.height
                ])?;
            }
            let mut chunk = tx.prepare(
                "INSERT INTO chunks (source_id, ord, start, end, page_start, page_end, section, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            let mut fts =
                tx.prepare("INSERT INTO chunks_fts (rowid, text, section) VALUES (?1, ?2, ?3)")?;
            for c in chunks {
                chunk.execute(params![
                    source_id,
                    c.ord,
                    c.start,
                    c.end,
                    c.page_start,
                    c.page_end,
                    c.section,
                    c.text
                ])?;
                fts.execute(params![tx.last_insert_rowid(), c.text, c.section])?;
            }
        }
        tx.commit()?;
        self.chunk_total += chunks.len() as u64;
        Ok(())
    }

    pub fn remove(&mut self, source_id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO chunks_fts (chunks_fts, rowid, text, section)
             SELECT 'delete', id, text, section FROM chunks WHERE source_id = ?1",
            [source_id],
        )?;
        let removed = tx.execute("DELETE FROM chunks WHERE source_id = ?1", [source_id])?;
        tx.execute("DELETE FROM sections WHERE source_id = ?1", [source_id])?;
        tx.execute("DELETE FROM figures WHERE source_id = ?1", [source_id])?;
        tx.execute("DELETE FROM vectors WHERE source_id = ?1", [source_id])?;
        tx.execute("DELETE FROM sources WHERE id = ?1", [source_id])?;
        tx.commit()?;
        self.chunk_total -= removed as u64;
        Ok(())
    }

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
    // Reads every vector per query: 11 ms at 5k chunks, 230 ms at 100k on an
    // 8-core machine. Past that, keep the vectors in memory or use an
    // approximate nearest-neighbor index.
    pub fn dense_search(
        &self,
        query: &[f32],
        limit: usize,
        source_ids: Option<&[&str]>,
    ) -> Result<Vec<Hit>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id, v.source_id, v.ord, v.vector
             FROM vectors v JOIN chunks c ON c.source_id = v.source_id AND c.ord = v.ord",
        )?;
        let mut rows = stmt.query([])?;
        let mut scored: Vec<Hit> = Vec::new();
        while let Some(row) = rows.next()? {
            let source_id: String = row.get(1)?;
            if source_ids.is_some_and(|ids| !ids.contains(&source_id.as_str())) {
                continue;
            }
            let vector: Vec<u8> = row.get(3)?;
            let score = vector
                .as_chunks::<4>()
                .0
                .iter()
                .zip(query)
                .map(|(b, q)| f32::from_le_bytes(*b) * q)
                .sum::<f32>();
            scored.push(Hit {
                chunk_id: row.get(0)?,
                source_id,
                ord: row.get(2)?,
                score: f64::from(score),
            });
        }
        scored.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        scored.truncate(limit);
        Ok(scored)
    }

    pub fn chunk(&self, source_id: &str, ord: u32) -> Result<Option<ChunkRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, source_id, ord, start, page_start, page_end, section, text
             FROM chunks WHERE source_id = ?1 AND ord = ?2",
        )?;
        Ok(stmt
            .query_row(params![source_id, ord], row_to_chunk)
            .optional()?)
    }

    /// Figures of a source that stand in `[start, end)`, in document order.
    pub fn figures_between(
        &self,
        source_id: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<Figure>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT start, src, file, caption, width, height FROM figures
             WHERE source_id = ?1 AND start >= ?2 AND start < ?3 ORDER BY start",
        )?;
        let end = i64::try_from(end).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(params![source_id, start, end], |r| {
                Ok(Figure {
                    start: r.get(0)?,
                    src: r.get(1)?,
                    file: r.get(2)?,
                    caption: r.get(3)?,
                    width: r.get(4)?,
                    height: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Chunks whose start falls in `[start, end)`, in document order.
    /// `usize::MAX` as `end` means "to the end of the source".
    pub fn chunks_between(
        &self,
        source_id: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<ChunkRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, source_id, ord, start, page_start, page_end, section, text
             FROM chunks WHERE source_id = ?1 AND start >= ?2 AND start < ?3 ORDER BY ord",
        )?;
        // SQLite integers are signed 64-bit.
        let end = i64::try_from(end).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(params![source_id, start, end], row_to_chunk)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn sections(&self, source_id: &str) -> Result<Vec<Section>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT title, depth, start FROM sections WHERE source_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok(Section {
                    title: r.get(0)?,
                    depth: r.get(1)?,
                    start: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Each section with its first chunk, so a reader can jump to it.
    /// Sections without text of their own are skipped.
    pub fn section_chunks(&self, source_id: &str) -> Result<Vec<(Section, u32)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT ord, start FROM chunks WHERE source_id = ?1 ORDER BY start")?;
        let starts: Vec<(u32, usize)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(self
            .sections(source_id)?
            .into_iter()
            .filter_map(|section| {
                let at = starts.partition_point(|&(_, start)| start < section.start);
                starts.get(at).map(|&(ord, _)| (section, ord))
            })
            .collect())
    }

    pub fn pages(&self, source_id: &str) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT pages FROM sources WHERE id = ?1",
                [source_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn chunk_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?)
    }

    /// Chunk count per source id.
    pub fn chunk_counts(&self) -> Result<HashMap<String, u32>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT source_id, count(*) FROM chunks GROUP BY source_id")?;
        let counts = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(counts)
    }
}

fn row_to_chunk(r: &rusqlite::Row) -> rusqlite::Result<ChunkRow> {
    Ok(ChunkRow {
        id: r.get(0)?,
        source_id: r.get(1)?,
        ord: r.get(2)?,
        start: r.get(3)?,
        page_start: r.get(4)?,
        page_end: r.get(5)?,
        section: r.get(6)?,
        text: r.get(7)?,
    })
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
    use crate::notebook::Notebook;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bowerbird-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn top(index: &Index, q: &str) -> Vec<(String, u32)> {
        index
            .search(q, 5)
            .unwrap()
            .into_iter()
            .map(|h| (h.source_id, h.ord))
            .collect()
    }

    #[test]
    fn an_index_from_another_version_is_rebuilt() {
        let dir = temp_dir("version");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text(
            "Satin",
            "# Satin bowerbird\nThe male collects blue bottle caps.\n",
        )
        .unwrap();
        drop(Index::open_synced(&nb).unwrap());
        let conn = Connection::open(nb.index_path()).unwrap();
        conn.pragma_update(None, "user_version", VERSION - 1)
            .unwrap();
        drop(conn);
        let (index, report) = Index::open_synced(&nb).unwrap();
        assert_eq!(report.added.len(), 1, "the source is extracted again");
        assert_eq!(index.chunk_count().unwrap(), 1);
    }

    #[test]
    fn deleted_index_rebuilds_to_identical_results() {
        let dir = temp_dir("rebuild");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text("Satin", "# Satin bowerbird\nThe male collects blue bottle caps and straws.\n## Diet\nFruit and insects.\n").unwrap();
        nb.add_text("Vogelkop", "# Vogelkop bowerbird\nIt sorts flowers, beetle wings and acorns into colored piles on moss.\n").unwrap();
        let (index, report) = Index::open_synced(&nb).unwrap();
        assert_eq!(report.added.len(), 2);
        let queries = [
            "blue bottle caps",
            "colored piles of flowers",
            "diet of fruit",
        ];
        let before: Vec<_> = queries.iter().map(|q| top(&index, q)).collect();
        assert!(
            before.iter().all(|hits| !hits.is_empty()),
            "every query should hit: {before:?}"
        );
        drop(index);

        std::fs::remove_file(nb.index_path()).unwrap();
        let (index, report) = Index::open_synced(&nb).unwrap();
        assert_eq!(report.added.len(), 2);
        let after: Vec<_> = queries.iter().map(|q| top(&index, q)).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn sync_drops_sources_removed_from_the_notebook() {
        let dir = temp_dir("sync-remove");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        nb.add_text("Satin", "Blue bottle caps everywhere.")
            .unwrap();
        let (_, _) = Index::open_synced(&nb).unwrap();
        nb.sources.clear();
        nb.save().unwrap();
        let (index, report) = Index::open_synced(&nb).unwrap();
        assert_eq!(report.removed, 1);
        assert!(index.search("bottle caps", 5).unwrap().is_empty());
    }

    #[test]
    fn each_section_points_at_its_first_chunk() {
        let dir = temp_dir("section-chunks");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        let source = nb
            .add_text("Satin", "# One\nBlue caps.\n# Two\nBlue straws.\n")
            .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let outline: Vec<(String, u32)> = index
            .section_chunks(&source.id)
            .unwrap()
            .into_iter()
            .map(|(s, ord)| (s.title, ord))
            .collect();
        assert_eq!(outline, [("One".to_string(), 0), ("Two".to_string(), 1)]);
    }

    #[test]
    fn chunks_between_accepts_open_ended_ranges() {
        let dir = temp_dir("open-range");
        let mut nb = Notebook::open_or_create(&dir, "Birds").unwrap();
        let source = nb
            .add_text("Satin", "# One\nBlue caps.\n# Two\nBlue straws.\n")
            .unwrap();
        let (index, _) = Index::open_synced(&nb).unwrap();
        let rows = index.chunks_between(&source.id, 0, usize::MAX).unwrap();
        assert_eq!(rows.len(), 2);
    }

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
}
