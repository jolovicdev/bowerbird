//! Extracted text of one source, with page boundaries and its heading tree.

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    /// 1 = top level.
    pub depth: u8,
    /// Byte offset into `Doc::text` where the section begins.
    pub start: usize,
}

/// A picture in a source, kept where it appeared in the text.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    /// Byte offset into `Doc::text`.
    pub start: usize,
    /// Address the picture came from.
    pub src: String,
    /// File name in the source's picture folder; empty until it is saved.
    pub file: String,
    /// Its alt text, else the short line after it, which is usually the caption.
    pub caption: String,
    pub width: u32,
    pub height: u32,
}

/// Pictures smaller than this are icons, flags and logos.
pub(crate) const FIGURE_MIN_WIDTH: u32 = 100;
pub(crate) const FIGURE_MIN_HEIGHT: u32 = 80;
/// A caption longer than this is body text, not a caption.
pub(crate) const CAPTION_CHARS: usize = 200;

/// A picture's caption: its alt text, else the short line that follows it.
pub(crate) fn caption(alt: &str, after: &str) -> String {
    match alt.trim() {
        "" => after
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .filter(|l| l.chars().count() <= CAPTION_CHARS && !l.starts_with(['#', '-', '|']))
            .unwrap_or_default()
            .to_string(),
        alt => alt.to_string(),
    }
}

/// Stands in for a picture while extracted text is cleaned up:
/// `MARK`, the picture's number, `MARK`. See `Doc::take_marks`.
pub(crate) const MARK: char = '\u{FFFC}';

#[derive(Debug, Clone)]
pub struct Doc {
    pub text: String,
    /// Byte offset where each page starts. Empty for sources without pages.
    pub page_starts: Vec<usize>,
    /// Sorted by `start`.
    pub sections: Vec<Section>,
    /// Sorted by `start`.
    pub figures: Vec<Figure>,
}

impl Doc {
    pub fn plain(text: String) -> Doc {
        Doc {
            text,
            page_starts: Vec::new(),
            sections: Vec::new(),
            figures: Vec::new(),
        }
    }

    /// Removes the picture marks from the text and section titles, returning
    /// each picture's number and the offset where it stood. Offsets of
    /// sections and pages after a mark move back by the mark's length.
    pub(crate) fn take_marks(&mut self) -> Vec<(usize, usize)> {
        if !self.text.contains(MARK) {
            return Vec::new();
        }
        let mut text = String::with_capacity(self.text.len());
        let mut found = Vec::new();
        // (offset in the old text where a mark ends, bytes removed up to there)
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        let mut rest = self.text.as_str();
        let mut consumed = 0;
        while let Some(at) = rest.find(MARK) {
            text.push_str(&rest[..at]);
            let after = &rest[at + MARK.len_utf8()..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            match (after[..digits].parse(), after[digits..].starts_with(MARK)) {
                (Ok(number), true) => {
                    let mut len = MARK.len_utf8() * 2 + digits;
                    // A mark alone on its line takes its line breaks with it.
                    if text.is_empty() || text.ends_with('\n') {
                        len += rest[at + len..]
                            .bytes()
                            .take(2)
                            .take_while(|&b| b == b'\n')
                            .count();
                    }
                    found.push((number, text.len()));
                    let removed = cuts.last().map_or(0, |c| c.1) + len;
                    cuts.push((consumed + at + len, removed));
                    consumed += at + len;
                    rest = &rest[at + len..];
                }
                // A lone mark is not a picture; drop the character.
                _ => {
                    let removed = cuts.last().map_or(0, |c| c.1) + MARK.len_utf8();
                    cuts.push((consumed + at + MARK.len_utf8(), removed));
                    consumed += at + MARK.len_utf8();
                    rest = after;
                }
            }
        }
        text.push_str(rest);
        let shift = |offset: usize| {
            let i = cuts.partition_point(|c| c.0 <= offset);
            offset - if i == 0 { 0 } else { cuts[i - 1].1 }
        };
        for section in &mut self.sections {
            section.start = shift(section.start);
            section.title = strip_marks(&section.title);
        }
        for page in &mut self.page_starts {
            *page = shift(*page);
        }
        self.text = text;
        found
    }

    /// 1-based page number containing `offset`.
    pub fn page_at(&self, offset: usize) -> Option<u32> {
        if self.page_starts.is_empty() {
            return None;
        }
        let idx = self.page_starts.partition_point(|&s| s <= offset);
        Some(idx.max(1) as u32)
    }
}

/// Parses ATX headings (`#` to `######`) outside fenced code blocks.
pub fn markdown(text: String) -> Doc {
    let mut sections = Vec::new();
    let mut fence: Option<&str> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if !(1..=6).contains(&hashes) {
            continue;
        }
        let rest = &trimmed[hashes..];
        if !(rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r'])) {
            continue;
        }
        let title = rest.trim().trim_end_matches('#').trim();
        if !title.is_empty() {
            sections.push(Section {
                title: title.to_string(),
                depth: hashes as u8,
                start,
            });
        }
    }
    Doc {
        text,
        page_starts: Vec::new(),
        sections,
        figures: Vec::new(),
    }
}

/// `text` without picture marks.
pub(crate) fn strip_marks(text: &str) -> String {
    let mut doc = Doc::plain(text.to_string());
    doc.take_marks();
    doc.text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_headings_skip_fenced_code() {
        let text = "# Intro\nHello\n```\n# not a heading\n```\n## Details ##\nBody\n#hashtag\n";
        let doc = markdown(text.to_string());
        let titles: Vec<_> = doc
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s.depth))
            .collect();
        assert_eq!(titles, vec![("Intro", 1), ("Details", 2)]);
        assert_eq!(&doc.text[doc.sections[1].start..][..10], "## Details");
    }

    #[test]
    fn picture_marks_leave_the_text_and_keep_their_places() {
        let m = MARK;
        let mut doc = markdown(format!("Intro {m}0{m}text\n## Map {m}1{m}here\nBody {m}\n"));
        let found = doc.take_marks();
        assert_eq!(doc.text, "Intro text\n## Map here\nBody \n");
        assert_eq!(found, vec![(0, 6), (1, 18)]);
        assert_eq!(doc.sections[0].start, 11);
        assert_eq!(doc.sections[0].title, "Map here");
    }

    #[test]
    fn page_at_maps_offsets_to_one_based_pages() {
        let doc = Doc {
            text: "aaaa\n\nbbbb\n\n".into(),
            page_starts: vec![0, 6],
            sections: vec![],
            figures: vec![],
        };
        assert_eq!(doc.page_at(0), Some(1));
        assert_eq!(doc.page_at(5), Some(1));
        assert_eq!(doc.page_at(6), Some(2));
        assert_eq!(Doc::plain("x".into()).page_at(0), None);
    }
}
