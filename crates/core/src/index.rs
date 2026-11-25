//! SQLite cache of a notebook's chunks with an FTS5 keyword index. Everything
//! here can be rebuilt from the notebook folder.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::chunk::{self, Chunk};
use crate::doc::{Doc, Figure, Section};
use crate::notebook::{Notebook, SourceKind};
use crate::{Error, Result};

mod search;

pub use search::query_terms;

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
-- Search by meaning reads every vector on each question; mapped reads skip a
-- copy per page and make that scan several times faster.
PRAGMA mmap_size = 1073741824;
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
            let read = guarded(|| {
                let doc = nb.extract(source)?;
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
                Ok((doc, chunks))
            });
            match read {
                Ok((doc, chunks)) => {
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

/// Runs `read`, turning a panic into a failure of the one source being read.
/// The source stays listed, so a panic would fail every later open of its
/// notebook, whose page is where it can be removed.
fn guarded<T>(read: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or_else(|_| {
        Err(Error::Invalid(
            "Bowerbird hit a bug reading this source. Remove it, and please report the problem."
                .into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::Notebook;

    pub(super) fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bowerbird-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    pub(super) fn top(index: &Index, q: &str) -> Vec<(String, u32)> {
        index
            .search(q, 5)
            .unwrap()
            .into_iter()
            .map(|h| (h.source_id, h.ord))
            .collect()
    }

    #[test]
    fn a_panic_reading_a_source_is_that_source_s_failure() {
        assert_eq!(guarded(|| Ok(1)).unwrap(), 1);
        let failed = guarded(|| -> Result<u32> { panic!("a bug in a reader") });
        assert!(failed.unwrap_err().to_string().contains("hit a bug"));
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
}
