//! Word documents: paragraphs, headings by style, tables and pictures from
//! `word/document.xml`.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use super::{
    Blocks, Converted, INFLATE_MAX, Package, attr, entity, first_text, invalid, join, local,
    xml_error,
};
use crate::Result;

pub fn docx(bytes: &[u8]) -> Result<Converted> {
    let mut package = Package::open(bytes, "Word document", INFLATE_MAX)?;
    let body = package
        .part("word/document.xml")?
        .ok_or_else(|| invalid("Word document", "it has no word/document.xml"))?;
    let levels = match package.part("word/styles.xml")? {
        Some(styles) => heading_styles(&styles)?,
        None => HashMap::new(),
    };
    let title = package
        .part("docProps/core.xml")?
        .and_then(|core| first_text(&core, "title"));
    let rels = match package.part("word/_rels/document.xml.rels")? {
        Some(rels) => relationships(&rels)?,
        None => HashMap::new(),
    };
    let blocks = docx_body(&body, &levels, &rels)?;
    Ok(Converted {
        title,
        markdown: blocks.out,
        images: blocks.images,
    })
}

/// Pictures a Word document links by relationship id, as package paths.
fn relationships(xml: &str) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/_rels/document.xml.rels", e))?
        {
            Event::Eof => return Ok(out),
            Event::Start(e) | Event::Empty(e) if local(&e) == "Relationship" => {
                let external = attr(&e, "TargetMode").is_some_and(|m| m == "External");
                if let (Some(id), Some(target), false) =
                    (attr(&e, "Id"), attr(&e, "Target"), external)
                {
                    let path = match target.strip_prefix('/') {
                        Some(absolute) => absolute.to_string(),
                        None => join("word", &target),
                    };
                    out.insert(id, path);
                }
            }
            _ => {}
        }
    }
}

/// Heading level of each paragraph style, from its name ("heading 2"; Word
/// writes built-in names in English whatever the interface language) or its
/// outline level, following `basedOn` for styles that inherit one.
fn heading_styles(xml: &str) -> Result<HashMap<String, usize>> {
    #[derive(Default)]
    struct Style {
        level: Option<usize>,
        based_on: Option<String>,
    }
    let mut styles: HashMap<String, Style> = HashMap::new();
    let mut current: Option<String> = None;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/styles.xml", e))?
        {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                if local(&e) == "style" {
                    current = (attr(&e, "type").as_deref() == Some("paragraph"))
                        .then(|| attr(&e, "styleId"))
                        .flatten();
                    if let Some(id) = &current {
                        styles.entry(id.clone()).or_default();
                    }
                    continue;
                }
                let Some(style) = current.as_ref().and_then(|id| styles.get_mut(id)) else {
                    continue;
                };
                match local(&e) {
                    "name" => {
                        let name = attr(&e, "val").unwrap_or_default().to_lowercase();
                        if name == "title" {
                            style.level = Some(1);
                        } else if let Some(n) = name.strip_prefix("heading ") {
                            style.level = n.parse().ok().filter(|n| (1..=6).contains(n));
                        }
                    }
                    "outlineLvl" => {
                        if let Some(n) = attr(&e, "val").and_then(|v| v.parse::<usize>().ok()) {
                            style.level = (n < 6).then_some(n + 1);
                        }
                    }
                    "basedOn" => style.based_on = attr(&e, "val"),
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().into_inner() == "style" => current = None,
            _ => {}
        }
    }
    let mut levels = HashMap::new();
    for id in styles.keys() {
        let mut at = id;
        // A style chain longer than this is a loop.
        for _ in 0..10 {
            let Some(style) = styles.get(at) else { break };
            if let Some(level) = style.level {
                levels.insert(id.clone(), level);
                break;
            }
            let Some(parent) = &style.based_on else { break };
            at = parent;
        }
    }
    Ok(levels)
}

fn docx_body(
    xml: &str,
    levels: &HashMap<String, usize>,
    rels: &HashMap<String, String>,
) -> Result<Blocks> {
    let mut blocks = Blocks::default();
    // A drawing's alt text comes before the picture it describes.
    let mut alt = String::new();
    let mut reader = Reader::from_str(xml);
    let (mut paragraphs, mut runs, mut tables) = (0, 0, 0);
    let mut in_text = false;
    let mut list_level: Option<usize> = None;
    let mut table: Vec<Vec<String>> = Vec::new();
    loop {
        match reader
            .read_event()
            .map_err(|e| xml_error("word/document.xml", e))?
        {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if local(&e) == "docPr" => {
                alt = attr(&e, "descr").unwrap_or_default();
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "blip" => {
                if let Some(path) = attr(&e, "embed").and_then(|id| rels.get(&id)) {
                    blocks.image(path.clone(), std::mem::take(&mut alt));
                }
            }
            Event::Start(e) => match local(&e) {
                "p" => {
                    paragraphs += 1;
                    if paragraphs == 1 {
                        list_level = None;
                    }
                }
                "r" => runs += 1,
                "t" if runs > 0 => in_text = true,
                "tbl" => {
                    tables += 1;
                    if tables == 1 {
                        blocks.end();
                        table.clear();
                    }
                }
                "tr" if tables == 1 => table.push(Vec::new()),
                "tc" if tables == 1 => {
                    if let Some(row) = table.last_mut() {
                        row.push(String::new());
                    }
                }
                // Word stores a text box twice, as a drawing and as a VML
                // fallback, and tracked moves keep the text at its old place
                // too; reading only one copy keeps the text once.
                "Fallback" | "moveFrom" => {
                    reader
                        .read_to_end(e.name())
                        .map_err(|e| xml_error("word/document.xml", e))?;
                }
                _ => {}
            },
            Event::Empty(e) => match local(&e) {
                "pStyle" if paragraphs == 1 => {
                    blocks.level = attr(&e, "val")
                        .and_then(|id| levels.get(&id).copied())
                        .unwrap_or(0);
                }
                "outlineLvl" if paragraphs == 1 => {
                    if let Some(n) = attr(&e, "val").and_then(|v| v.parse::<usize>().ok()) {
                        blocks.level = if n < 6 { n + 1 } else { 0 };
                    }
                }
                "ilvl" if paragraphs == 1 => {
                    list_level = attr(&e, "val").and_then(|v| v.parse().ok());
                }
                "tab" if runs > 0 => blocks.push(" "),
                "br" | "cr" if runs > 0 => blocks.line_break(),
                "noBreakHyphen" if runs > 0 => blocks.push("-"),
                _ => {}
            },
            Event::Text(t) if in_text => blocks.push(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => blocks.push(&entity(&r)),
            Event::End(e) => match e.local_name().into_inner() {
                "t" => in_text = false,
                "r" => runs = (runs - 1).max(0),
                "p" => {
                    paragraphs -= 1;
                    if paragraphs > 0 {
                        blocks.push(" ");
                    } else if tables > 0 {
                        let text = std::mem::take(&mut blocks.line);
                        blocks.space = false;
                        if let Some(cell) = table.last_mut().and_then(|row| row.last_mut()) {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(text.trim());
                        }
                    } else {
                        if blocks.level == 0
                            && let Some(depth) = list_level
                        {
                            blocks.prefix = format!("{}- ", "  ".repeat(depth));
                        }
                        blocks.end();
                        blocks.level = 0;
                        blocks.prefix.clear();
                    }
                }
                "tbl" => {
                    tables -= 1;
                    if tables == 0 {
                        blocks.level = 0;
                        if let Some(markdown) = markdown_table(&table) {
                            blocks.raw(&markdown);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(blocks)
}

fn markdown_table(rows: &[Vec<String>]) -> Option<String> {
    let width = rows.iter().map(Vec::len).max().filter(|&w| w > 0)?;
    if rows.iter().flatten().all(|cell| cell.is_empty()) {
        return None;
    }
    let line = |row: &[String]| {
        let cells: Vec<String> = (0..width)
            .map(|i| row.get(i).map_or(String::new(), |c| c.replace('|', "\\|")))
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    out.extend(rows[1..].iter().map(|row| line(row)));
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::MARK;
    use crate::zipdoc::Image;
    use crate::zipdoc::tests::*;

    #[test]
    fn marks_word_pictures_through_their_relationships() {
        let drawing = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Map of the Empire"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let document = format!(
            r#"<w:document {W} xmlns:wp="wp" xmlns:a="a" xmlns:pic="pic" xmlns:r="r"><w:body>{}{drawing}{}</w:body></w:document>"#,
            paragraph("", "Before"),
            paragraph("", "After")
        );
        let rels = r#"<Relationships><Relationship Id="rId5" Target="media/image1.png"/><Relationship Id="rId9" Target="https://example.org" TargetMode="External"/></Relationships>"#;
        let converted = docx(&package(&[
            ("word/document.xml", &document),
            ("word/_rels/document.xml.rels", rels),
        ]))
        .unwrap();
        assert_eq!(
            converted.images,
            vec![Image {
                path: "word/media/image1.png".into(),
                alt: "Map of the Empire".into()
            }]
        );
        assert_eq!(
            converted.markdown,
            format!("Before\n\n{MARK}0{MARK}\n\nAfter")
        );
    }

    #[test]
    fn word_pictures_with_child_elements_are_marked_too() {
        let drawing = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Map of the Empire"><a:hlinkClick r:id="rId9"/></wp:docPr><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5" cstate="print"><a:extLst><a:ext uri="{28A0092B-C50C-407E-A947-70E740481C1C}"><a14:useLocalDpi val="0"/></a:ext></a:extLst></a:blip></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let document = format!(
            r#"<w:document {W} xmlns:wp="wp" xmlns:a="a" xmlns:a14="a14" xmlns:pic="pic" xmlns:r="r"><w:body>{drawing}</w:body></w:document>"#
        );
        let rels =
            r#"<Relationships><Relationship Id="rId5" Target="media/image1.png"/></Relationships>"#;
        let converted = docx(&package(&[
            ("word/document.xml", &document),
            ("word/_rels/document.xml.rels", rels),
        ]))
        .unwrap();
        assert_eq!(
            converted.images,
            vec![Image {
                path: "word/media/image1.png".into(),
                alt: "Map of the Empire".into()
            }]
        );
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

    fn paragraph(style: &str, text: &str) -> String {
        let style = if style.is_empty() {
            String::new()
        } else {
            format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#)
        };
        format!(r#"<w:p>{style}<w:r><w:t>{text}</w:t></w:r></w:p>"#)
    }

    #[test]
    fn reads_a_word_document() {
        let styles = format!(
            r#"<w:styles {W}>
            <w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style>
            <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
            <w:style w:type="paragraph" w:styleId="berschrift2"><w:name w:val="heading 2"/></w:style>
            <w:style w:type="paragraph" w:styleId="Chapter"><w:name w:val="Chapter"/><w:basedOn w:val="Heading1"/></w:style>
            <w:style w:type="paragraph" w:styleId="Loop"><w:name w:val="Loop"/><w:basedOn w:val="Loop"/></w:style>
            <w:style w:type="character" w:styleId="Heading1Char"><w:name w:val="heading 1 Char"/></w:style>
            </w:styles>"#
        );
        let body = [
            paragraph("Title", "The Peace of Westphalia"),
            paragraph("Heading1", "Background"),
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t xml:space="preserve">Signed in </w:t></w:r><w:r><w:t>Münster</w:t></w:r><w:r><w:tab/><w:t>&amp; Osnabrück</w:t></w:r><w:del><w:r><w:delText>in 1650</w:delText></w:r></w:del><w:moveFrom><w:r><w:t xml:space="preserve"> in 1648.</w:t></w:r></w:moveFrom><w:moveTo><w:r><w:t xml:space="preserve"> in 1648.</w:t></w:r></w:moveTo></w:p>"#.into(),
            paragraph("berschrift2", "Terms"),
            r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Sovereignty</w:t></w:r></w:p>"#.into(),
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>for princes</w:t></w:r></w:p>"#.into(),
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Treaty</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Place</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Osnabrück</w:t></w:r></w:p><w:p><w:r><w:t>(Sweden)</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>a|b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#.into(),
            r#"<w:p><w:r><mc:AlternateContent><mc:Choice><w:drawing><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>note</w:t></w:r></w:p>"#.into(),
            paragraph("Chapter", "Aftermath"),
            paragraph("Loop", "# not a heading"),
            r#"<w:p><w:r><w:t>Line one</w:t><w:br/><w:t>line two</w:t></w:r></w:p>"#.into(),
        ]
        .concat();
        let document = format!(
            r#"<?xml version="1.0"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#
        );
        let core = r#"<cp:coreProperties xmlns:cp="c" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Westphalia  notes</dc:title></cp:coreProperties>"#;
        let bytes = package(&[
            ("word/document.xml", &document),
            ("word/styles.xml", &styles),
            ("docProps/core.xml", core),
        ]);
        let converted = docx(&bytes).unwrap();
        assert_eq!(converted.title.as_deref(), Some("Westphalia notes"));
        assert_eq!(
            converted.markdown,
            "# The Peace of Westphalia\n\n# Background\n\n\
             Signed in Münster & Osnabrück in 1648.\n\n## Terms\n\n- Sovereignty\n\n  - for princes\n\n\
             | Treaty | Place |\n| --- | --- |\n| Osnabrück (Sweden) | a\\|b |\n\n\
             Boxed note\n\n# Aftermath\n\n\\# not a heading\n\nLine one\nline two"
        );
    }
}
