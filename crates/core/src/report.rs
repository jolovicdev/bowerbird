//! Reports written from the sources: a study guide, a briefing, a timeline,
//! an FAQ, or a format the user describes. They cite like answers and are
//! saved as notes.

use serde::{Deserialize, Serialize};

use crate::ask::CITE_RULES;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    StudyGuide,
    Briefing,
    Timeline,
    Faq,
    /// The user's own instructions.
    Custom,
}

/// Longest custom report instructions, in characters.
pub const INSTRUCTIONS_MAX: usize = 5_000;

/// The request line sent after the passages.
pub const REQUEST: &str = "Write the report.";

fn format(kind: Kind) -> &'static str {
    match kind {
        Kind::StudyGuide => {
            "Write a study guide. Start with a short summary of the main ideas. Then a glossary under the heading ## Key terms: each term in bold, a colon, and a one-sentence definition. Then ## Review questions: eight to ten short-answer questions, each followed by a two- or three-sentence answer. End with ## Essay questions: three open questions without answers."
        }
        Kind::Briefing => {
            "Write a briefing document for someone who must understand the sources quickly. Start with ## Summary, a paragraph on the main points. Then one ## section per major theme, with the most important facts, figures, and short quotations. End with ## Open questions: what the sources leave unresolved or disagree about."
        }
        Kind::Timeline => {
            "Write a timeline of the events in the sources. Under ## Timeline, list the events in chronological order, one list item each, starting with the date in bold followed by what happened. Then under ## People, list each person who matters, in bold, with a one-sentence description of their role."
        }
        Kind::Faq => {
            "Write a list of eight to twelve frequently asked questions about the sources, each as a ### heading followed by a concise answer of two to four sentences. Order them from the most basic to the most advanced."
        }
        Kind::Custom => "",
    }
}

/// The system prompt for a report of this kind. Custom reports need instructions.
pub fn system(kind: Kind, instructions: &str) -> Result<String> {
    let instructions = instructions.trim();
    let format = match kind {
        Kind::Custom if instructions.is_empty() => {
            return Err(Error::Invalid(
                "Describe the report you want, such as \"a one-page summary for a museum visitor\"."
                    .into(),
            ));
        }
        Kind::Custom => format!("Write the report the user describes:\n{instructions}"),
        _ => format(kind).to_string(),
    };
    Ok(format!(
        "You write reports using only the passages below, taken from the user's own sources.
{format}
Use Markdown. Don't add a title; the report has one already.
{CITE_RULES}
Write in the language of the sources."
    ))
}

/// The note title for a report.
pub fn title(kind: Kind, topic: &str, notebook: &str) -> String {
    let name = match kind {
        Kind::StudyGuide => "Study guide",
        Kind::Briefing => "Briefing",
        Kind::Timeline => "Timeline",
        Kind::Faq => "FAQ",
        Kind::Custom => "Report",
    };
    match topic.trim() {
        "" => format!("{name}: {notebook}"),
        topic => format!("{name}: {topic}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_asks_for_its_format_and_keeps_the_citation_rules() {
        let guide = system(Kind::StudyGuide, "").unwrap();
        assert!(guide.contains("## Key terms") && guide.contains("[[A3|exact words]]"));
        let timeline = system(Kind::Timeline, "ignored").unwrap();
        assert!(timeline.contains("chronological") && !timeline.contains("ignored"));
        let custom = system(Kind::Custom, "A summary for a museum visitor.").unwrap();
        assert!(custom.contains("museum visitor") && custom.contains("[[A3|exact words]]"));
        assert!(system(Kind::Custom, "  ").is_err());
        assert_eq!(
            title(Kind::Faq, "", "Thirty Years War"),
            "FAQ: Thirty Years War"
        );
        assert_eq!(
            title(Kind::Timeline, " the Swedish phase ", "x"),
            "Timeline: the Swedish phase"
        );
    }
}
