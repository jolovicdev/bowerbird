//! Splits a `Doc` into chunks that never cross a section boundary, so every
//! chunk has exactly one heading path.

use text_splitter::{ChunkConfig, MarkdownSplitter, TextSplitter};

use crate::doc::Doc;

/// Characters per chunk. Small enough for precise citations, large enough to
/// hold a full claim.
const CHUNK_CHARS: std::ops::Range<usize> = 400..1000;
/// Longest text handed to the splitter at once. Without paragraph or sentence
/// breaks the splitter rescans the rest of its input for every chunk, so one
/// huge unbroken section would take quadratic time.
const MAX_WINDOW: usize = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub ord: u32,
    pub start: usize,
    pub end: usize,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    /// Heading path such as "Methods > Data".
    pub section: String,
    pub text: String,
}

pub fn split(doc: &Doc, markdown: bool) -> Vec<Chunk> {
    let md = MarkdownSplitter::new(ChunkConfig::new(CHUNK_CHARS));
    let plain = TextSplitter::new(ChunkConfig::new(CHUNK_CHARS));

    let mut spans: Vec<(usize, String)> = vec![(0, String::new())];
    let mut stack: Vec<(u8, &str)> = Vec::new();
    for s in &doc.sections {
        while stack.last().is_some_and(|(depth, _)| *depth >= s.depth) {
            stack.pop();
        }
        stack.push((s.depth, &s.title));
        let path = stack
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>()
            .join(" > ");
        spans.push((s.start, path));
    }

    let mut chunks = Vec::new();
    for (i, (start, path)) in spans.iter().enumerate() {
        let end = spans.get(i + 1).map_or(doc.text.len(), |next| next.0);
        if end <= *start {
            continue;
        }
        let mut pieces: Vec<(usize, &str)> = Vec::new();
        for (window_start, window) in windows(&doc.text[*start..end]) {
            let found: Vec<(usize, &str)> = if markdown {
                md.chunk_indices(window).collect()
            } else {
                plain.chunk_indices(window).collect()
            };
            pieces.extend(
                found
                    .into_iter()
                    .map(|(o, piece)| (window_start + o, piece)),
            );
        }
        for (offset, piece) in pieces {
            let chunk_start = start + offset;
            let chunk_end = chunk_start + piece.len();
            chunks.push(Chunk {
                ord: chunks.len() as u32,
                start: chunk_start,
                end: chunk_end,
                page_start: doc.page_at(chunk_start),
                page_end: doc.page_at(chunk_end.saturating_sub(1)),
                section: path.clone(),
                text: piece.to_string(),
            });
        }
    }
    chunks
}

/// Cuts `text` into pieces of at most `MAX_WINDOW` bytes, preferring a
/// paragraph break, then a line break, then whitespace in the second half of
/// each window. Returns each piece with its offset.
fn windows(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    while text.len() - start > MAX_WINDOW {
        let limit = text.floor_char_boundary(start + MAX_WINDOW);
        let window = &text[start..limit];
        let late = |i: &usize| *i > window.len() / 2;
        let cut = window
            .rfind("\n\n")
            .filter(late)
            .map(|i| i + 2)
            .or_else(|| window.rfind('\n').filter(late).map(|i| i + 1))
            .or_else(|| {
                window
                    .rfind(char::is_whitespace)
                    .filter(late)
                    .map(|i| i + 1)
            })
            .unwrap_or(window.len());
        out.push((start, &text[start..start + cut]));
        start += cut;
    }
    out.push((start, &text[start..]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{self, Doc};

    #[test]
    fn chunks_carry_section_paths() {
        let doc = doc::markdown("Preface text.\n# Methods\nWe counted.\n## Data\nCaps and straws.\n# Results\nBlue wins.\n".into());
        let got: Vec<_> = split(&doc, true)
            .into_iter()
            .map(|c| (c.section, c.text))
            .collect();
        assert_eq!(
            got,
            vec![
                ("".to_string(), "Preface text.".to_string()),
                ("Methods".to_string(), "# Methods\nWe counted.".to_string()),
                (
                    "Methods > Data".to_string(),
                    "## Data\nCaps and straws.".to_string()
                ),
                ("Results".to_string(), "# Results\nBlue wins.".to_string()),
            ]
        );
    }

    #[test]
    fn long_unbroken_text_goes_to_the_splitter_in_bounded_windows() {
        let text = "lorem ipsum dolor ".repeat(5_000);
        let parts = windows(&text);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|(_, w)| w.len() <= MAX_WINDOW));
        assert!(
            parts[..parts.len() - 1]
                .iter()
                .all(|(_, w)| w.ends_with(' ')),
            "cuts land after whitespace"
        );
        let joined: String = parts.iter().map(|(_, w)| *w).collect();
        assert_eq!(joined, text);
        assert!(parts.iter().all(|(o, w)| &text[*o..*o + w.len()] == *w));
    }

    #[test]
    fn chunks_record_the_pages_they_span() {
        let page_one = "a ".repeat(150);
        let page_two = "b ".repeat(150);
        let text = format!("{page_one}\n\n{page_two}\n\n");
        let doc = Doc {
            page_starts: vec![0, page_one.len() + 2],
            sections: vec![],
            figures: vec![],
            text,
        };
        let chunks = split(&doc, false);
        assert_eq!(
            chunks.len(),
            1,
            "both short pages fit one chunk: {chunks:?}"
        );
        assert_eq!(
            (chunks[0].page_start, chunks[0].page_end),
            (Some(1), Some(2))
        );
    }
}
