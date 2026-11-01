//! The citation markup the model writes, `[[A3|exact words]]`, and the check
//! that the quoted words really appear in the cited passage.

/// Longest text held back while waiting for a marker's closing `]]`.
const MAX_MARKUP: usize = 600;
/// Shorter quotes match too easily to prove anything.
const MIN_QUOTE_CHARS: usize = 8;
/// Shortest piece of a quote with an ellipsis, in letters and digits. Shorter
/// pieces can be strung together from scattered words of a long passage.
const MIN_PIECE_CHARS: usize = 10;
/// Most letters and digits an ellipsis may stand for: about two sentences.
const MAX_ELISION: usize = 300;

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Cite { label: String, quote: String },
}

/// Turns streamed model text into text and citation pieces. Markers may be
/// split across stream chunks.
#[derive(Default)]
pub struct Parser {
    pending: String,
}

impl Parser {
    pub fn feed(&mut self, chunk: &str) -> Vec<Piece> {
        self.pending.push_str(chunk);
        self.drain(false)
    }

    pub fn finish(&mut self) -> Vec<Piece> {
        self.drain(true)
    }

    /// Emits every piece that more text can't change. The pieces never depend
    /// on where the stream split the text, so a saved answer parses exactly
    /// as it streamed.
    fn drain(&mut self, at_end: bool) -> Vec<Piece> {
        let mut out = Vec::new();
        loop {
            let Some(open) = self.pending.find("[[") else {
                let keep = usize::from(!at_end && self.pending.ends_with('['));
                let emit = self.pending.len() - keep;
                if emit > 0 {
                    out.push(Piece::Text(self.pending.drain(..emit).collect()));
                }
                break;
            };
            if open > 0 {
                out.push(Piece::Text(self.pending.drain(..open).collect()));
            }
            let close = self.pending[2..]
                .find("]]")
                .map(|at| at + 2)
                .filter(|&at| at <= MAX_MARKUP);
            match close {
                Some(close) => {
                    // A quote can end in a bracket of its own, as in
                    // `[[B3|… [2]]]`, so the marker ends at the last `]]` of the run.
                    let run = self.pending[close..]
                        .bytes()
                        .take_while(|&b| b == b']')
                        .count();
                    let end = close + run;
                    if end == self.pending.len() && !at_end {
                        break;
                    }
                    let marker: String = self.pending.drain(..end).collect();
                    let inner = &marker[2..marker.len() - 2];
                    out.push(parse_marker(inner).unwrap_or(Piece::Text(marker.clone())));
                }
                None if self.pending.len() < MAX_MARKUP + 2 && !at_end => break,
                None => out.push(Piece::Text(self.pending.drain(..2).collect())),
            }
        }
        merge_text(out)
    }
}

fn merge_text(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match (out.last_mut(), piece) {
            (Some(Piece::Text(prev)), Piece::Text(next)) => prev.push_str(&next),
            (_, piece) => out.push(piece),
        }
    }
    out
}

fn parse_marker(inner: &str) -> Option<Piece> {
    let (label, quote) = inner.split_once('|')?;
    let label = label.trim();
    split_label(label)?;
    Some(Piece::Cite {
        label: label.to_string(),
        quote: quote
            .trim()
            .trim_matches(['"', '“', '”'])
            .trim()
            .to_string(),
    })
}

/// "AB12" -> ("AB", 12)
pub fn split_label(label: &str) -> Option<(&str, u32)> {
    let letters = label.bytes().take_while(u8::is_ascii_uppercase).count();
    if letters == 0 || letters == label.len() {
        return None;
    }
    Some((&label[..letters], label[letters..].parse().ok()?))
}

/// Keeps only letters and digits, lowercased, with ligatures expanded. PDF
/// text layers split words, subscripts, and hyphenated line ends
/// unpredictably; comparing letter-and-digit sequences still requires the
/// quote's words, contiguous and in order.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ﬁ' => out.push_str("fi"),
            'ﬂ' => out.push_str("fl"),
            'ﬀ' => out.push_str("ff"),
            'ﬃ' => out.push_str("ffi"),
            'ﬄ' => out.push_str("ffl"),
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => {}
        }
    }
    out
}

/// True when every part of `quote` (split on ellipses) appears in `passage`,
/// in order, each part close after the one before it.
pub fn verify(quote: &str, passage: &str) -> bool {
    let passage = normalize(passage);
    let parts: Vec<String> = quote
        .split(['…'])
        .flat_map(|p| p.split("..."))
        .map(normalize)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let total: usize = parts.iter().map(String::len).sum();
    if parts.is_empty() || total < MIN_QUOTE_CHARS {
        return false;
    }
    if parts.len() > 1 && parts.iter().any(|p| p.chars().count() < MIN_PIECE_CHARS) {
        return false;
    }
    in_order(&passage, &parts, 0, false)
}

/// Finds `parts` in `passage` from byte `from` on. After the first part, each
/// must start within MAX_ELISION of the previous one's end. Every place a part
/// appears is tried, since its first appearance may be too far from the next.
fn in_order(passage: &str, parts: &[String], from: usize, elided: bool) -> bool {
    let Some((part, rest)) = parts.split_first() else {
        return true;
    };
    let tail = &passage[from..];
    tail.match_indices(part.as_str())
        .take_while(|&(at, _)| !elided || tail[..at].chars().count() <= MAX_ELISION)
        .any(|(at, _)| in_order(passage, rest, from + at + part.len(), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSAGE: &str =
        "The thresh-\nold was lowered to 3 percent\nfor all “filers” in the ﬁscal year.";

    #[test]
    fn verify_tolerates_pdf_layout_noise() {
        assert!(verify("threshold was lowered to 3 percent", PASSAGE));
        assert!(verify("for all \"filers\" in the fiscal year", PASSAGE));
        assert!(verify("The threshold was … for all filers", PASSAGE));
    }

    #[test]
    fn verify_rejects_words_not_in_the_passage() {
        assert!(!verify("threshold was raised to 3 percent", PASSAGE));
        assert!(!verify("for all filers ... threshold was lowered", PASSAGE));
        assert!(!verify("the", PASSAGE));
    }

    #[test]
    fn verify_rejects_quotes_stitched_from_scattered_pieces() {
        assert!(!verify("The threshold … to 3 … fiscal", PASSAGE));
        let far = format!(
            "The council met in spring. {}The tax was abolished.",
            "Nothing else happened that year. ".repeat(12)
        );
        assert!(verify("The council met in spring", &far));
        assert!(!verify(
            "The council met in spring … The tax was abolished",
            &far
        ));
    }

    #[test]
    fn verify_tries_every_place_the_first_piece_appears() {
        let passage = format!(
            "The levy was raised. {}The levy was raised again, and the towns paid it.",
            "Wars came and went. ".repeat(20)
        );
        assert!(verify("The levy was raised … the towns paid it", &passage));
    }

    #[test]
    fn parser_handles_markers_split_across_chunks() {
        let mut parser = Parser::default();
        let mut pieces = Vec::new();
        for chunk in [
            "Rates fell [",
            "[A3|rates fell",
            " sharply]] and rose [[B12|x y z]].",
            " Tail [[not a cite]]",
        ] {
            pieces.extend(parser.feed(chunk));
        }
        pieces.extend(parser.finish());
        assert_eq!(
            merge_text(pieces),
            vec![
                Piece::Text("Rates fell ".into()),
                Piece::Cite {
                    label: "A3".into(),
                    quote: "rates fell sharply".into()
                },
                Piece::Text(" and rose ".into()),
                Piece::Cite {
                    label: "B12".into(),
                    quote: "x y z".into()
                },
                Piece::Text(". Tail [[not a cite]]".into()),
            ]
        );
    }

    fn parse_in_chunks(text: &str, size: usize) -> Vec<Piece> {
        let mut parser = Parser::default();
        let mut pieces = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut cut = size.min(rest.len());
            while !rest.is_char_boundary(cut) {
                cut += 1;
            }
            pieces.extend(parser.feed(&rest[..cut]));
            rest = &rest[cut..];
        }
        pieces.extend(parser.finish());
        merge_text(pieces)
    }

    #[test]
    fn pieces_do_not_depend_on_how_the_stream_was_split() {
        let long = "word ".repeat(150);
        let texts = [
            format!("An over-long quote [[A3|{long}]] then text."),
            format!("Unclosed [[A3|quote] {long} and [[B2|a real quote]]."),
            "Cited [[B32|as CuratedTrec (CT) [2]]], then more.".to_string(),
            "Ends on a marker [[C1|the last words]]".to_string(),
        ];
        for text in &texts {
            let whole = parse_in_chunks(text, text.len());
            for size in [1, 2, 3, 7, 64, 599, 601] {
                assert_eq!(parse_in_chunks(text, size), whole, "chunk size {size}");
            }
        }
        let cites = |pieces: &[Piece]| {
            pieces
                .iter()
                .filter(|p| matches!(p, Piece::Cite { .. }))
                .count()
        };
        assert_eq!(cites(&parse_in_chunks(&texts[0], 5)), 0);
        assert_eq!(
            parse_in_chunks(&texts[1], 5).last(),
            Some(&Piece::Text(".".into()))
        );
        assert_eq!(cites(&parse_in_chunks(&texts[1], 5)), 1);
    }

    #[test]
    fn a_quote_ending_in_a_reference_keeps_its_bracket() {
        assert_eq!(
            parse_in_chunks("Cited [[B32|as CuratedTrec (CT) [2]]], then more.", 4),
            vec![
                Piece::Text("Cited ".into()),
                Piece::Cite {
                    label: "B32".into(),
                    quote: "as CuratedTrec (CT) [2]".into()
                },
                Piece::Text(", then more.".into()),
            ]
        );
    }
}
