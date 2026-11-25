//! Turns a fetched page into the Markdown a source is indexed from: the
//! article's text, its fact boxes as lists, and its pictures as figures.

use dom_smoothie::{Config, Readability, TextMode};

use super::figures::{Image, figures};
use crate::doc::{self, Doc, MARK};
use crate::{Error, Result};

/// Returns the article title and its text as a Markdown `Doc`, with the
/// pictures worth showing as figures whose files aren't saved yet.
pub fn extract(html: &str, url: Option<&str>) -> Result<(String, Doc)> {
    let config = Config {
        text_mode: TextMode::Markdown,
        ..Default::default()
    };
    let article = Readability::new(html, url, Some(config))
        .and_then(|mut r| r.parse())
        .map_err(|e| Error::Invalid(format!("Couldn't find readable text on this page: {e}")))?;
    let mut images = Vec::new();
    let mut doc = doc::markdown(clean(&article.text_content, &mut images));
    doc.figures = figures(&mut doc, &images, html, url);
    Ok((article.title, doc))
}

/// Extracted Markdown made fit for reading and prompting. Infoboxes are read
/// after `tidy`, which turns the `<br>` inside their cells into line breaks.
fn clean(md: &str, images: &mut Vec<Image>) -> String {
    infoboxes(&tidy(md, images))
}

/// Wikipedia's infoboxes come out of extraction as a table whose header row
/// holds every label and whose rows each hold one value, in the same order,
/// so nothing reads as paired. Each becomes "label: value" lines, with the
/// image caption, the one value more than there are labels, on its own line.
/// Tables with several cells to a row are left as they are.
fn infoboxes(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        match infobox(&lines, i) {
            Some((block, next)) => {
                out.push(block);
                i = next;
            }
            None => {
                out.push(lines[i].to_string());
                i += 1;
            }
        }
    }
    out.join("\n")
}

/// The infobox whose header row starts at line `at`, rewritten, and the
/// index of the line after it.
fn infobox(lines: &[&str], at: usize) -> Option<(String, usize)> {
    let (header, after_header) = table_row(lines, at)?;
    let divider = lines.get(after_header)?.trim();
    if !divider.starts_with('|')
        || !divider.contains('-')
        || !divider.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
    {
        return None;
    }
    let labels: Vec<String> = header.split('|').map(one_line).collect();
    let mut values = Vec::new();
    let mut at = after_header + 1;
    while let Some((cell, next)) = table_row(lines, at) {
        if cell.contains('|') {
            return None;
        }
        values.push(one_line(&cell));
        at = next;
    }
    let caption = match values.len().checked_sub(labels.len())? {
        0 => None,
        1 => Some(values.remove(0)),
        _ => return None,
    };
    let mut block = String::new();
    if let Some(caption) = caption.filter(|c| !c.is_empty()) {
        block.push_str(&caption);
        block.push_str("\n\n");
    }
    for (label, value) in labels.iter().zip(&values) {
        block.push_str(&format!("- {label}: {value}\n"));
    }
    Some((block.trim_end().to_string(), at))
}

/// The cells of a table row starting at line `at`, without its outer pipes,
/// and the index of the line after it. A cell may run over several lines.
fn table_row(lines: &[&str], at: usize) -> Option<(String, usize)> {
    let mut row = lines.get(at)?.trim_start().strip_prefix('|')?.to_string();
    let mut next = at + 1;
    while !row.trim_end().ends_with('|') {
        row.push('\n');
        row.push_str(lines.get(next)?);
        next += 1;
    }
    let row = row.trim_end();
    Some((row[..row.len() - 1].to_string(), next))
}

/// A cell's lines joined; the items of a "+ " list inside it are separated
/// with semicolons, other line breaks become spaces.
fn one_line(cell: &str) -> String {
    let lines: Vec<&str> = cell
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let bullet = |l: &&str| l.starts_with(['+', '-', '*']);
    if lines.iter().any(bullet) {
        lines
            .iter()
            .map(|l| l.trim_start_matches(['+', '-', '*']).trim())
            .collect::<Vec<_>>()
            .join("; ")
    } else {
        lines.join(" ")
    }
}

/// Keeps the words of extracted Markdown and drops what only a browser needs:
/// link targets, footnote links, backslash escapes, and `<br>`. Left in, they
/// clutter passages, add tokens to every prompt, and split quotes so they no
/// longer match the text the model saw. Each image becomes a mark (see
/// `doc::MARK`) numbering it in `images`.
fn tidy(md: &str, images: &mut Vec<Image>) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(at) = rest.find(['[', '!', '\\', '<']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(tail) = edit_link(rest) {
            rest = tail.strip_prefix("\n\n").unwrap_or(tail);
        } else if let Some(after) = rest.strip_prefix('!')
            && let Some((alt, target, tail)) = link(after)
        {
            let src = target.split_whitespace().next().unwrap_or_default();
            out.push_str(&format!("{MARK}{}{MARK}", images.len()));
            images.push(Image {
                alt: tidy(alt, &mut Vec::new()),
                src: src.to_string(),
            });
            rest = tail;
        } else if let Some((text, target, tail)) = link(rest) {
            if !target.starts_with("#cite_") {
                out.push_str(&tidy(text, images));
            }
            rest = tail;
        } else if let Some(c) = rest
            .strip_prefix('\\')
            .and_then(|r| r.chars().next())
            .filter(char::is_ascii_punctuation)
        {
            out.push(c);
            rest = &rest[1 + c.len_utf8()..];
        } else if let Some(tail) = ["<br>", "<br/>", "<br />"]
            .iter()
            .find_map(|tag| rest.strip_prefix(tag))
        {
            out.push('\n');
            rest = tail;
        } else {
            let c = rest.chars().next().expect("found a character");
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.push_str(rest);
    out
}

/// What follows a MediaWiki section's `[edit]` link at the start of `s`,
/// which extraction writes with its brackets escaped.
fn edit_link(s: &str) -> Option<&str> {
    let (_, target, tail) = link(s.strip_prefix("\\[")?)?;
    target
        .contains("action=edit")
        .then(|| tail.strip_prefix("\\]"))
        .flatten()
}

/// A Markdown link at the start of `s`: its text, its target, and what follows.
/// Brackets in the text may be escaped, and parentheses in the target nest, as
/// in Wikipedia's `Foo_(bar)` pages.
fn link(s: &str) -> Option<(&str, &str, &str)> {
    let body = s.strip_prefix('[')?;
    let mut depth = 1;
    let mut escaped = false;
    let mut close = None;
    for (i, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    let close = close?;
    let target_part = body[close + 1..].strip_prefix('(')?;
    let mut depth = 1;
    for (i, c) in target_part.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let target = target_part[..i].split_whitespace().next().unwrap_or("");
                    return Some((&body[..close], target, &target_part[i + 1..]));
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_keeps_link_words_and_drops_link_targets() {
        let md = "Built after [Berlin](https://en.wikipedia.org/wiki/Berlin \"Berlin\") was split\\.[\\[28\\]](#cite_note-28)\n\
                  ![](https://upload.example/flag.gif) [French Guards](https://en.wikipedia.org/wiki/Gardes_Fran%C3%A7aises \"Gardes\")<br>\
                  [Mercury (planet)](https://en.wikipedia.org/wiki/Mercury_(planet)) and [not a link] (1961)";
        let mut images = Vec::new();
        assert_eq!(
            tidy(md, &mut images),
            format!(
                "Built after Berlin was split.\n{MARK}0{MARK} French Guards\nMercury (planet) and [not a link] (1961)"
            )
        );
        assert_eq!(images[0].src, "https://upload.example/flag.gif");
        assert_eq!(
            tidy("[![logo](https://x/logo.png)](https://x)", &mut images),
            format!("{MARK}1{MARK}"),
            "a linked image leaves only its mark"
        );
        assert_eq!(images[1].alt, "logo");
    }

    #[test]
    fn infoboxes_pair_each_label_with_its_value() {
        let md = "Intro.\n\n| Date | Location | Territorial  \nchanges |\n| ---- | -------- | ---- |\n\
                  | 23 May 1618 – 24 October 1648 |\n| Central Europe |\n| + France annexes\n+ Sweden gains\n |\n\nBelligerents";
        assert_eq!(
            infoboxes(md),
            "Intro.\n\n- Date: 23 May 1618 – 24 October 1648\n- Location: Central Europe\n\
             - Territorial changes: France annexes; Sweden gains\n\nBelligerents"
        );
        assert_eq!(
            clean(
                "| Date | Territorial <br>changes |\n| -- | -- |\n| [1648](https://x/1648) |\n| + Alsace<br>+ Wismar |",
                &mut Vec::new()
            ),
            "- Date: 1648\n- Territorial changes: Alsace; Wismar"
        );
        let captioned =
            "| Date |\n| ---- |\n| A leaflet of 1618 |\n| + 30 July 1419\n+ 23 May 1618\n |\n";
        assert_eq!(
            infoboxes(captioned),
            "A leaflet of 1618\n\n- Date: 30 July 1419; 23 May 1618\n"
        );
        for table in [
            "| Year | Event |\n| --- | --- |\n| 1618 | Revolt |\n| 1648 | Peace |",
            "| Name |\n| --- |\n| a |\n| b |\n| c |",
            "| not | a table |\nbecause no divider",
        ] {
            assert_eq!(infoboxes(table), table);
        }
    }

    #[test]
    fn mediawiki_edit_links_are_left_out() {
        let md = "## Background: 1556 to 1618\n\n\\[[edit](https://en.wikipedia.org/w/index.php?title=Thirty_Years%27_War&action=edit&section=2 \"Edit section: Background: 1556 to 1618\")\\]\n\nMajor developments in the war.";
        assert_eq!(
            tidy(md, &mut Vec::new()),
            "## Background: 1556 to 1618\n\nMajor developments in the war."
        );
    }
}
