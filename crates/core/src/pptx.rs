//! Slide decks as PowerPoint files, written as Office Open XML: one master,
//! one layout, and a notes page for every slide with speaker notes. Text
//! stays editable in PowerPoint, Keynote, LibreOffice, and Google Slides.

use std::io::{Cursor, Write};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::Result;
use crate::ask::Citation;
use crate::study::Slide;

/// 16:9 at PowerPoint's default size, in EMU (914,400 to the inch).
const WIDTH: i64 = 12_192_000;
const HEIGHT: i64 = 6_858_000;
const MARGIN: i64 = 853_440;
const INK: &str = "1F1F1D";
const INK_2: &str = "5F5E59";
const RUBRIC: &str = "A3161F";
const PAPER: &str = "FAFAF8";
const RULE: &str = "D3D4CE";
/// Literata isn't installed on most machines that open the file; Georgia is.
const FONT: &str = "Georgia";

const DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const OFFICE: &str = "application/vnd.openxmlformats-officedocument";
const GROUP: &str =
    r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#;
const CLR_MAP: &str = r#"bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink""#;
const QUOTE_MAX: usize = 80;

/// Writes the deck. `place` names where a citation points, such as
/// "Peace of Westphalia, p. 3".
pub fn deck(title: &str, slides: &[Slide], place: impl Fn(&Citation) -> String) -> Result<Vec<u8>> {
    let mut package = Package::default();
    package.add(
        "_rels/.rels",
        None,
        rels(&[
            ("rId1", format!("{REL}/officeDocument"), "ppt/presentation.xml".into()),
            (
                "rId2",
                "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties".into(),
                "docProps/core.xml".into(),
            ),
        ]),
    );
    package.add(
        "docProps/core.xml",
        Some("application/vnd.openxmlformats-package.core-properties+xml"),
        format!(
            r#"{DECL}<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title></cp:coreProperties>"#,
            esc(title)
        ),
    );

    let mut presentation_rels = vec![
        (
            "rId1",
            format!("{REL}/slideMaster"),
            "slideMasters/slideMaster1.xml".to_string(),
        ),
        (
            "rId2",
            format!("{REL}/notesMaster"),
            "notesMasters/notesMaster1.xml".into(),
        ),
        ("rId3", format!("{REL}/theme"), "theme/theme1.xml".into()),
        ("rId4", format!("{REL}/presProps"), "presProps.xml".into()),
        ("rId5", format!("{REL}/viewProps"), "viewProps.xml".into()),
        (
            "rId6",
            format!("{REL}/tableStyles"),
            "tableStyles.xml".into(),
        ),
    ];
    let slide_ids: Vec<String> = (1..=slides.len())
        .map(|n| format!("rId{}", 100 + n))
        .collect();
    let mut list = String::new();
    for (i, id) in slide_ids.iter().enumerate() {
        list.push_str(&format!(r#"<p:sldId id="{}" r:id="{id}"/>"#, 256 + i));
        presentation_rels.push((
            id,
            format!("{REL}/slide"),
            format!("slides/slide{}.xml", i + 1),
        ));
    }
    package.add(
        "ppt/presentation.xml",
        Some(&format!("{OFFICE}.presentationml.presentation.main+xml")),
        format!(
            r#"{DECL}<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="rId2"/></p:notesMasterIdLst><p:sldIdLst>{list}</p:sldIdLst><p:sldSz cx="{WIDTH}" cy="{HEIGHT}"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#
        ),
    );
    package.add(
        "ppt/_rels/presentation.xml.rels",
        None,
        rels(&presentation_rels),
    );
    package.add(
        "ppt/presProps.xml",
        Some(&format!("{OFFICE}.presentationml.presProps+xml")),
        format!("{DECL}<p:presentationPr {NS}/>"),
    );
    package.add(
        "ppt/viewProps.xml",
        Some(&format!("{OFFICE}.presentationml.viewProps+xml")),
        format!("{DECL}<p:viewPr {NS}/>"),
    );
    package.add(
        "ppt/tableStyles.xml",
        Some(&format!("{OFFICE}.presentationml.tableStyles+xml")),
        format!(
            r#"{DECL}<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
        ),
    );
    for n in 1..=2 {
        package.add(
            &format!("ppt/theme/theme{n}.xml"),
            Some(&format!("{OFFICE}.theme+xml")),
            theme(),
        );
    }
    package.add(
        "ppt/slideMasters/slideMaster1.xml",
        Some(&format!("{OFFICE}.presentationml.slideMaster+xml")),
        master(),
    );
    package.add(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        None,
        rels(&[
            (
                "rId1",
                format!("{REL}/slideLayout"),
                "../slideLayouts/slideLayout1.xml".into(),
            ),
            ("rId2", format!("{REL}/theme"), "../theme/theme1.xml".into()),
        ]),
    );
    package.add(
        "ppt/slideLayouts/slideLayout1.xml",
        Some(&format!("{OFFICE}.presentationml.slideLayout+xml")),
        format!(
            r#"{DECL}<p:sldLayout {NS} type="obj" preserve="1"><p:cSld name="Title and points"><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
            placeholder(2, "Title", r#"type="title""#, "", "<a:bodyPr/>", EMPTY),
            placeholder(3, "Points", r#"type="body" idx="1""#, "", "<a:bodyPr/>", EMPTY),
        ),
    );
    package.add(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        None,
        rels(&[(
            "rId1",
            format!("{REL}/slideMaster"),
            "../slideMasters/slideMaster1.xml".into(),
        )]),
    );
    package.add(
        "ppt/notesMasters/notesMaster1.xml",
        Some(&format!("{OFFICE}.presentationml.notesMaster+xml")),
        format!(
            r#"{DECL}<p:notesMaster {NS}><p:cSld><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/><p:notesStyle><a:lvl1pPr><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:notesStyle></p:notesMaster>"#,
            slide_image(&xfrm(381_000, 685_800, 6_096_000, 3_429_000)),
            placeholder(3, "Notes", r#"type="body" idx="1""#, &xfrm(685_800, 4_343_400, 5_486_400, 4_114_800), "<a:bodyPr/>", EMPTY),
        ),
    );
    package.add(
        "ppt/notesMasters/_rels/notesMaster1.xml.rels",
        None,
        rels(&[("rId1", format!("{REL}/theme"), "../theme/theme2.xml".into())]),
    );

    for (i, s) in slides.iter().enumerate() {
        let n = i + 1;
        let mut slide_rels = vec![(
            "rId1",
            format!("{REL}/slideLayout"),
            "../slideLayouts/slideLayout1.xml".to_string(),
        )];
        if !s.notes.trim().is_empty() {
            slide_rels.push((
                "rId2",
                format!("{REL}/notesSlide"),
                format!("../notesSlides/notesSlide{n}.xml"),
            ));
            package.add(
                &format!("ppt/notesSlides/notesSlide{n}.xml"),
                Some(&format!("{OFFICE}.presentationml.notesSlide+xml")),
                format!(
                    r#"{DECL}<p:notes {NS}><p:cSld><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#,
                    slide_image(""),
                    placeholder(3, "Notes", r#"type="body" idx="1""#, "", "<a:bodyPr/>", &paragraphs(s.notes.lines(), "")),
                ),
            );
            package.add(
                &format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels"),
                None,
                rels(&[
                    (
                        "rId1",
                        format!("{REL}/notesMaster"),
                        "../notesMasters/notesMaster1.xml".into(),
                    ),
                    (
                        "rId2",
                        format!("{REL}/slide"),
                        format!("../slides/slide{n}.xml"),
                    ),
                ]),
            );
        }
        package.add(
            &format!("ppt/slides/slide{n}.xml"),
            Some(&format!("{OFFICE}.presentationml.slide+xml")),
            slide(s, n == 1, &place),
        );
        package.add(
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            None,
            rels(&slide_rels),
        );
    }

    package.finish()
}

const EMPTY: &str = r#"<a:p><a:endParaRPr lang="en-US"/></a:p>"#;

#[derive(Default)]
struct Package {
    parts: Vec<(String, String)>,
    /// Content type of every part but relationships, as (part name, type).
    types: Vec<(String, String)>,
}

impl Package {
    fn add(&mut self, name: &str, content_type: Option<&str>, body: String) {
        if let Some(t) = content_type {
            self.types.push((name.to_string(), t.to_string()));
        }
        self.parts.push((name.to_string(), body));
    }

    fn finish(self) -> Result<Vec<u8>> {
        let overrides: String = self
            .types
            .iter()
            .map(|(name, t)| format!(r#"<Override PartName="/{name}" ContentType="{t}"/>"#))
            .collect();
        let types = format!(
            r#"{DECL}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{overrides}</Types>"#
        );
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in
            std::iter::once(("[Content_Types].xml".to_string(), types)).chain(self.parts)
        {
            zip.start_file(name, options)
                .map_err(std::io::Error::from)?;
            zip.write_all(body.as_bytes())?;
        }
        Ok(zip.finish().map_err(std::io::Error::from)?.into_inner())
    }
}

fn rels(items: &[(&str, String, String)]) -> String {
    let list: String = items
        .iter()
        .map(|(id, kind, target)| {
            format!(r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#)
        })
        .collect();
    format!(
        r#"{DECL}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{list}</Relationships>"#
    )
}

fn xfrm(x: i64, y: i64, cx: i64, cy: i64) -> String {
    format!(
        r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#
    )
}

fn placeholder(id: u32, name: &str, ph: &str, geometry: &str, body_pr: &str, text: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph {ph}/></p:nvPr></p:nvSpPr><p:spPr>{geometry}</p:spPr><p:txBody>{body_pr}<a:lstStyle/>{text}</p:txBody></p:sp>"#
    )
}

fn slide_image(geometry: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr>{geometry}</p:spPr></p:sp>"#
    )
}

/// One paragraph per line; `run` is extra run properties, such as a size.
fn paragraphs<'a>(lines: impl Iterator<Item = &'a str>, run: &str) -> String {
    let text: String = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            format!(
                r#"<a:p><a:r><a:rPr lang="en-US"{run}/><a:t>{}</a:t></a:r></a:p>"#,
                esc(l.trim())
            )
        })
        .collect();
    if text.is_empty() {
        EMPTY.to_string()
    } else {
        text
    }
}

const TITLE_Y: i64 = 640_080;
const TITLE_H: i64 = 1_463_040;
const BODY_Y: i64 = 2_286_000;
const BODY_H: i64 = 2_834_640;
const RULE_Y: i64 = 5_303_520;
const SOURCES_Y: i64 = 5_394_960;
const SOURCES_H: i64 = 914_400;

fn slide(s: &Slide, opening: bool, place: &impl Fn(&Citation) -> String) -> String {
    let wide = WIDTH - 2 * MARGIN;
    let size = if opening { 6000 } else { 4400 };
    let mut tree = placeholder(
        2,
        "Title",
        r#"type="title""#,
        "",
        "<a:bodyPr/>",
        &paragraphs(
            std::iter::once(s.title.as_str()),
            &format!(r#" sz="{size}""#),
        ),
    );
    if !s.points.is_empty() {
        tree.push_str(&placeholder(
            3,
            "Points",
            r#"type="body" idx="1""#,
            "",
            "<a:bodyPr><a:normAutofit/></a:bodyPr>",
            &paragraphs(s.points.iter().map(String::as_str), ""),
        ));
    }
    if !s.citations.is_empty() {
        let lines: String = s
            .citations
            .iter()
            .map(|c| {
                let siglum = format!("{}{}", c.letter, if c.verified { "" } else { "†" });
                let quote: String = c.quote.chars().take(QUOTE_MAX).collect();
                let more = if c.quote.chars().count() > QUOTE_MAX { "…" } else { "" };
                format!(
                    r#"<a:p><a:r><a:rPr lang="en-US" sz="1200" b="1" i="1"><a:solidFill><a:srgbClr val="{RUBRIC}"/></a:solidFill></a:rPr><a:t>{}</a:t></a:r><a:r><a:rPr lang="en-US" sz="1200"><a:solidFill><a:srgbClr val="{INK_2}"/></a:solidFill></a:rPr><a:t>  {}: “{}{more}”</a:t></a:r></a:p>"#,
                    esc(&siglum),
                    esc(&place(c)),
                    esc(&quote),
                )
            })
            .collect();
        tree.push_str(&format!(
            r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="4" name="Rule"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="{MARGIN}" y="{RULE_Y}"/><a:ext cx="{wide}" cy="0"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="9525"><a:solidFill><a:srgbClr val="{RULE}"/></a:solidFill></a:ln></p:spPr></p:cxnSp><p:sp><p:nvSpPr><p:cNvPr id="5" name="Sources"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" tIns="0" rIns="0" bIns="0"><a:normAutofit/></a:bodyPr><a:lstStyle/>{lines}</p:txBody></p:sp>"#,
            xfrm(MARGIN, SOURCES_Y, wide, SOURCES_H),
        ));
    }
    format!(
        r#"{DECL}<p:sld {NS}><p:cSld><p:spTree>{GROUP}{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn master() -> String {
    let wide = WIDTH - 2 * MARGIN;
    format!(
        r#"{DECL}<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="{PAPER}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:defRPr sz="4400" i="1"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:spcBef><a:spcPts val="1200"/></a:spcBef><a:buClr><a:srgbClr val="{RUBRIC}"/></a:buClr><a:buFont typeface="Arial"/><a:buChar char="•"/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
        placeholder(
            2,
            "Title",
            r#"type="title""#,
            &xfrm(MARGIN, TITLE_Y, wide, TITLE_H),
            r#"<a:bodyPr anchor="b"/>"#,
            EMPTY
        ),
        placeholder(
            3,
            "Points",
            r#"type="body" idx="1""#,
            &xfrm(MARGIN, BODY_Y, wide, BODY_H),
            "<a:bodyPr/>",
            EMPTY
        ),
    )
}

fn theme() -> String {
    let fill = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#;
    let line = format!(r#"<a:ln w="9525">{fill}</a:ln>"#);
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    let font = format!(r#"<a:latin typeface="{FONT}"/><a:ea typeface=""/><a:cs typeface=""/>"#);
    let color = |name: &str, hex: &str| format!(r#"<a:{name}><a:srgbClr val="{hex}"/></a:{name}>"#);
    let colors: String = [
        ("dk1", INK),
        ("lt1", PAPER),
        ("dk2", INK_2),
        ("lt2", "EDEDE8"),
        ("accent1", RUBRIC),
        ("accent2", "2F5D8A"),
        ("accent3", "4F7A4A"),
        ("accent4", "B07A1F"),
        ("accent5", "6B4C8A"),
        ("accent6", "3E7C83"),
        ("hlink", RUBRIC),
        ("folHlink", "7A1219"),
    ]
    .iter()
    .map(|(name, hex)| color(name, hex))
    .collect();
    format!(
        r#"{DECL}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Bowerbird"><a:themeElements><a:clrScheme name="Bowerbird">{colors}</a:clrScheme><a:fontScheme name="Bowerbird"><a:majorFont>{font}</a:majorFont><a:minorFont>{font}</a:minorFont></a:fontScheme><a:fmtScheme name="Bowerbird"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{line}{line}{line}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
    )
}

/// Escapes text for XML. Control characters have no XML 1.0 form and one
/// would make the whole file unreadable, so they are dropped.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;

    use quick_xml::events::Event;
    use quick_xml::{Reader, XmlVersion};

    use super::*;

    fn citation(letter: &str, verified: bool) -> Citation {
        Citation {
            label: format!("{letter}1"),
            letter: letter.into(),
            source_id: None,
            page: Some(3),
            quote: "Swedish invasion".into(),
            verified,
        }
    }

    fn unpack(bytes: Vec<u8>) -> BTreeMap<String, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut parts = BTreeMap::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let mut body = String::new();
            file.read_to_string(&mut body).unwrap();
            parts.insert(file.name().unwrap().into_owned(), body);
        }
        parts
    }

    /// Every attribute value named `attr` in the part.
    fn values(xml: &str, attr: &str) -> Vec<String> {
        let mut reader = Reader::from_str(xml);
        let mut out = Vec::new();
        loop {
            match reader.read_event().unwrap_or_else(|e| panic!("{e}: {xml}")) {
                Event::Eof => return out,
                Event::Start(e) | Event::Empty(e) => {
                    for a in e.attributes() {
                        let a = a.unwrap();
                        if AsRef::<str>::as_ref(&a.key) == attr {
                            out.push(
                                a.normalized_value(XmlVersion::Explicit1_0)
                                    .unwrap()
                                    .into_owned(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn deck_of_two() -> BTreeMap<String, String> {
        let slides = vec![
            Slide {
                title: "A religious quarrel became a European war".into(),
                points: vec!["Began in 1618".into(), "Ended in 1648".into()],
                notes: "Open with the defenestration.\nThen the war.".into(),
                citations: vec![citation("A", true)],
            },
            Slide {
                title: "<Münster & \"Osnabrück\">\u{7}".into(),
                points: vec![],
                notes: String::new(),
                citations: vec![citation("B", false)],
            },
        ];
        unpack(
            deck("Thirty years", &slides, |c| {
                format!("Source {}, p. {}", c.letter, c.page.unwrap())
            })
            .unwrap(),
        )
    }

    #[test]
    fn every_part_is_well_formed_typed_and_linked() {
        let parts = deck_of_two();
        let typed = values(&parts["[Content_Types].xml"], "PartName");
        for (name, body) in &parts {
            values(body, "Id");
            if name.ends_with(".xml") && name != "[Content_Types].xml" {
                assert!(
                    typed.contains(&format!("/{name}")),
                    "{name} has no content type"
                );
            }
            if let Some(rels_of) = name.strip_suffix(".rels") {
                let base = rels_of.replace("_rels/", "");
                let dir = base.rsplit_once('/').map_or("", |(dir, _)| dir);
                for target in values(body, "Target") {
                    let mut path: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
                    for piece in target.split('/') {
                        if piece == ".." {
                            path.pop();
                        } else {
                            path.push(piece);
                        }
                    }
                    let resolved = path.join("/");
                    assert!(
                        parts.contains_key(&resolved),
                        "{name} points at missing {resolved}"
                    );
                }
            }
        }
    }

    #[test]
    fn writes_text_notes_and_apparatus() {
        let parts = deck_of_two();
        let first = &parts["ppt/slides/slide1.xml"];
        assert!(first.contains("Began in 1618") && first.contains("Ended in 1648"));
        assert!(first.contains("Source A, p. 3: “Swedish invasion”"));
        let notes = &parts["ppt/notesSlides/notesSlide1.xml"];
        assert!(notes.contains("Open with the defenestration.") && notes.contains("Then the war."));
        assert!(!parts.contains_key("ppt/notesSlides/notesSlide2.xml"));
        let second = &parts["ppt/slides/slide2.xml"];
        assert!(second.contains("&lt;Münster &amp; &quot;Osnabrück&quot;&gt;</a:t>"));
        assert!(second.contains(">B†</a:t>"));
        assert!(!second.contains("Points"));
    }
}
