//! Spoken sources as Markdown: what was said in paragraphs of about a
//! minute, each opening with its time, under a heading for each chapter.
//! YouTube captions and transcribed recordings both come out this way.

/// A new paragraph starts after this many seconds, or sooner at the end of a
/// sentence once half of it has passed.
const PARAGRAPH_SECONDS: u64 = 60;

/// `said` is what was said, in order, with the second it starts at;
/// `chapters` are (second, title) from the start, when there are any.
pub fn markdown(
    title: &str,
    byline: &str,
    said: &[(u64, String)],
    chapters: &[(u64, String)],
) -> String {
    let mut out = format!("# {}\n", title.trim());
    if !byline.trim().is_empty() {
        out.push_str(&format!("\n{}\n", byline.trim()));
    }
    let mut next_chapter = 0;
    let mut paragraph = String::new();
    let mut started = 0;
    let flush = |out: &mut String, paragraph: &mut String, started: u64| {
        if !paragraph.is_empty() {
            out.push_str(&format!("\n[{}] {}\n", clock(started), paragraph));
            paragraph.clear();
        }
    };
    for (at, text) in said {
        let at = *at;
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        if next_chapter < chapters.len() && at >= chapters[next_chapter].0 {
            flush(&mut out, &mut paragraph, started);
            while next_chapter < chapters.len() && at >= chapters[next_chapter].0 {
                next_chapter += 1;
            }
            out.push_str(&format!("\n## {}\n", chapters[next_chapter - 1].1));
        } else if !paragraph.is_empty() {
            let elapsed = at.saturating_sub(started);
            let sentence_ended = paragraph.ends_with(['.', '?', '!']);
            if elapsed >= PARAGRAPH_SECONDS || (sentence_ended && elapsed >= PARAGRAPH_SECONDS / 2)
            {
                flush(&mut out, &mut paragraph, started);
            }
        }
        if paragraph.is_empty() {
            started = at;
        } else {
            paragraph.push(' ');
        }
        paragraph.push_str(&text);
    }
    flush(&mut out, &mut paragraph, started);
    out
}

/// "1:02:03" or "2:45".
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_timed_paragraphs_under_chapter_headings() {
        let said: Vec<(u64, String)> = [
            (0, "Welcome."),
            (1, "\n"),
            (20, "Today, the war."),
            (40, "It began\nin Prague"),
            (70, "in 1618"),
            (130, "with a window."),
            (165, "Then the war spread."),
            (3725, "Westphalia ended it."),
        ]
        .into_iter()
        .map(|(at, text)| (at, text.to_string()))
        .collect();
        let chapters = [
            (0, "Introduction".to_string()),
            (40, "Prague".to_string()),
            (3723, "Peace".to_string()),
        ];
        assert_eq!(
            markdown(
                "The Thirty Years War",
                "A video by History Lectures on YouTube.",
                &said,
                &chapters
            ),
            "# The Thirty Years War\n\nA video by History Lectures on YouTube.\n\n\
             ## Introduction\n\n[0:00] Welcome. Today, the war.\n\n\
             ## Prague\n\n[0:40] It began in Prague in 1618\n\n[2:10] with a window.\n\n[2:45] Then the war spread.\n\n\
             ## Peace\n\n[1:02:05] Westphalia ended it.\n"
        );
        assert_eq!(markdown("Untitled", "", &[], &[]), "# Untitled\n");
    }
}
