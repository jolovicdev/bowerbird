//! Reads scanned PDF pages with PP-OCRv5: a shared text detector paired with
//! a script-specific recognizer and dictionary. The models are downloaded once,
//! on first use for each script.

use std::path::{Path, PathBuf};

use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::utils::create_rgb_image;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, download, pdf, settings};

/// A fixed release, so the files can't change under a pinned model.
const RELEASE: &str = "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0";
const DETECTOR: (&str, &str) = (
    "pp-ocrv5_mobile_det.onnx",
    "1eb7b4f7ab657ebd1c66d5f79bca7497f29768a2e3c15e52daecbba1a8e4a039",
);

/// Writing system used to read scanned PDFs. Each script uses the shared
/// PP-OCRv5 detector and its own recognizer and character dictionary.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OcrScript {
    /// English and European Latin scripts, accented letters included.
    #[default]
    Latin,
    /// Russian, Ukrainian, Bulgarian, Serbian, and other Cyrillic scripts.
    Cyrillic,
    /// Chinese, Japanese, and English (general PP-OCRv5 mobile recognizer).
    Multilingual,
    /// Korean (Hangul and English).
    Korean,
    /// Arabic script.
    Arabic,
    /// Hindi, Marathi, Nepali, Sanskrit, and other Devanagari scripts.
    Devanagari,
    /// Greek script.
    Greek,
    /// Thai script.
    Thai,
    /// Tamil script.
    Tamil,
    /// Telugu script.
    Telugu,
}

impl OcrScript {
    pub const ALL: [OcrScript; 10] = [
        OcrScript::Latin,
        OcrScript::Cyrillic,
        OcrScript::Multilingual,
        OcrScript::Korean,
        OcrScript::Arabic,
        OcrScript::Devanagari,
        OcrScript::Greek,
        OcrScript::Thai,
        OcrScript::Tamil,
        OcrScript::Telugu,
    ];

    pub fn is_default(&self) -> bool {
        matches!(self, OcrScript::Latin)
    }

    /// (detector, recognizer, dictionary) with pinned SHA-256 checksums.
    pub fn files(self) -> [(&'static str, &'static str); 3] {
        let (rec, dict) = match self {
            OcrScript::Latin => (
                (
                    "latin_pp-ocrv5_mobile_rec.onnx",
                    "e3a6bfeea1c8a01d6fccfd480a0bd363fd907f8c65931e228bb2736f5c3e142f",
                ),
                (
                    "ppocrv5_latin_dict.txt",
                    "ccbcc45730b3fbbd9050c5bc74db6a99067141ef1035e3d14889a84a6b9b1aff",
                ),
            ),
            OcrScript::Cyrillic => (
                (
                    "cyrillic_pp-ocrv5_mobile_rec.onnx",
                    "a18d96d7c8d73d90f2ed056549caa1de3a8e6cb744cccba16cd593ea8cd2d569",
                ),
                (
                    "ppocrv5_cyrillic_dict.txt",
                    "db40aa52ceb112055be80c694afdf655d5d2c4f7873704524cc16a447ca913ba",
                ),
            ),
            OcrScript::Multilingual => (
                (
                    "pp-ocrv5_mobile_rec.onnx",
                    "243a0f06d826761323e9045e9b113ab2c191c3aa50565585e628300b8eda0224",
                ),
                (
                    "ppocrv5_dict.txt",
                    "d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b",
                ),
            ),
            OcrScript::Korean => (
                (
                    "korean_pp-ocrv5_mobile_rec.onnx",
                    "2d7ed96308065a86103325d22af07a88c4d06afc009f21602a4882342c0cc054",
                ),
                (
                    "ppocrv5_korean_dict.txt",
                    "a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7",
                ),
            ),
            OcrScript::Arabic => (
                (
                    "arabic_pp-ocrv5_mobile_rec.onnx",
                    "2768206d9a0ce48eba45b59619184e18161dde8f44115f029920ca17a9dc0384",
                ),
                (
                    "ppocrv5_arabic_dict.txt",
                    "7f92f7dbb9b75a4787a83bfb4f6d14a8ab515525130c9d40a9036f61cf6999e9",
                ),
            ),
            OcrScript::Devanagari => (
                (
                    "devanagari_pp-ocrv5_mobile_rec.onnx",
                    "b3d50774dfbec6ae02249ff79a925431a4381c8c6f86d342ff6e7b63e5fefa77",
                ),
                (
                    "ppocrv5_devanagari_dict.txt",
                    "09c7440bfc5477e5c41052304b6b185aff8c4a5e8b2b4c23c1c706f6fe1ee9fc",
                ),
            ),
            OcrScript::Greek => (
                (
                    "el_pp-ocrv5_mobile_rec.onnx",
                    "5a4a020e48e8783e035e1af135423c2161a363acab9ef16e48238c3d181f0f71",
                ),
                (
                    "ppocrv5_el_dict.txt",
                    "31defc62c0c3ad3674a82da6192226a2ba98ef4ff014a7045cb88d59f9c3de31",
                ),
            ),
            OcrScript::Thai => (
                (
                    "th_pp-ocrv5_mobile_rec.onnx",
                    "5f6ee21242691681261fee01bc39867da9cc8ff9b889f2f048b3cb7f74380217",
                ),
                (
                    "ppocrv5_th_dict.txt",
                    "57f5406f94bb6688fb7077f7be65f08bbd71cecf48c01ea26c522cb5c4836b7a",
                ),
            ),
            OcrScript::Tamil => (
                (
                    "ta_pp-ocrv5_mobile_rec.onnx",
                    "508d07ac0e1806a8b8857ebf20bd8837d68d962c3fdba030cf0022238ba819b8",
                ),
                (
                    "ppocrv5_ta_dict.txt",
                    "85b541352ae18dc6ba6d47152d8bf8adff6b0266e605d2eef2990c1bf466117b",
                ),
            ),
            OcrScript::Telugu => (
                (
                    "te_pp-ocrv5_mobile_rec.onnx",
                    "0957fc03425324d30afcd4342a5708687fd0c885a5c0987cae418e96dd761a60",
                ),
                (
                    "ppocrv5_te_dict.txt",
                    "42f83f5d3fdb50778e4fa5b66c58d99a59ab7792151c5e74f34b8ffd7b61c9d6",
                ),
            ),
        };
        [DETECTOR, rec, dict]
    }
}

pub fn dir() -> PathBuf {
    settings::models_dir().join("ocr")
}

/// Whether the models for `script` are already in `dir`.
pub fn ready(dir: &Path, script: OcrScript) -> bool {
    script
        .files()
        .iter()
        .all(|(file, _)| dir.join(file).is_file())
}

/// Downloads the models for `script` into `dir`, reporting as `download::files`.
pub async fn download(dir: &Path, script: OcrScript, progress: impl Fn(u64, u64)) -> Result<()> {
    let files = script.files();
    download::files(dir, RELEASE, &files, progress).await
}

pub struct Reader(OAROCR);

impl Reader {
    pub fn load(dir: &Path, script: OcrScript) -> Result<Reader> {
        let [(det, _), (rec, _), (dict, _)] = script.files();
        OAROCRBuilder::new(dir.join(det), dir.join(rec), dir.join(dict))
            .build()
            .map(Reader)
            .map_err(|e| Error::Model(format!("The text reader failed to load: {e}")))
    }

    /// The text of each page of a scanned PDF, lines in reading order,
    /// pages separated by form feeds. `progress` gets (pages done, pages).
    pub fn read_pdf(&self, path: &Path, progress: impl Fn(usize, usize)) -> Result<String> {
        let mut pages = Vec::new();
        pdf::render_pages(path, |i, total, rgb, width, height| {
            pages.push(self.read_page(rgb, width, height)?);
            progress(i + 1, total);
            Ok(())
        })?;
        Ok(pages.join("\u{c}"))
    }

    fn read_page(&self, rgb: Vec<u8>, width: u32, height: u32) -> Result<String> {
        let image = create_rgb_image(width, height, rgb)
            .ok_or_else(|| Error::Pdf("A page rendered at the wrong size.".into()))?;
        let results = self
            .0
            .predict(vec![image])
            .map_err(|e| Error::Model(format!("Reading the page failed: {e}")))?;
        Ok(results
            .iter()
            .flat_map(|r| r.text_regions.iter())
            .filter_map(|region| region.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use crate::notebook::Notebook;
    use crate::pdf::fixture::{Page, build, scanned};

    /// Downloads the models, so it runs only with BOWERBIRD_LIVE=1.
    #[tokio::test]
    async fn a_scanned_pdf_is_read_and_indexed() {
        if std::env::var("BOWERBIRD_LIVE").as_deref() != Ok("1") {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to download the OCR models");
            return;
        }
        let tmp = std::env::temp_dir().join(format!("bowerbird-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // A page of real text, rendered to pixels and saved as an image-only PDF.
        let text = build(
            &[Page {
                lines: vec![
                    (18.0, 700.0, "The Peace of Westphalia"),
                    (
                        12.0,
                        660.0,
                        r"Signed at Osnabr\374ck and M\374nster in 1648.",
                    ),
                ],
            }],
            &[],
        );
        std::fs::write(tmp.join("text.pdf"), text).unwrap();
        let mut pages = Vec::new();
        pdf::render_pages(&tmp.join("text.pdf"), |_, _, rgb, w, h| {
            pages.push((rgb, w, h));
            Ok(())
        })
        .unwrap();
        std::fs::write(tmp.join("scan.pdf"), scanned(&pages)).unwrap();

        let mut nb = Notebook::open_or_create(&tmp.join("nb"), "Scans").unwrap();
        let source = nb.add_file(&tmp.join("scan.pdf")).unwrap();
        let (_, report) = Index::open_synced(&nb).unwrap();
        assert_eq!(report.scanned, std::slice::from_ref(&source.id));

        download(&dir(), OcrScript::Latin, |_, _| {}).await.unwrap();
        let reader = Reader::load(&dir(), OcrScript::Latin).unwrap();
        let read = reader
            .read_pdf(&nb.source_path(&source), |_, _| {})
            .unwrap();
        assert!(read.contains("Westphalia"), "{read:?}");
        assert!(read.contains("1648"), "{read:?}");
        assert!(
            read.contains("Osnabrück") && read.contains("Münster"),
            "{read:?}"
        );

        std::fs::write(nb.ocr_path(&source), read).unwrap();
        let (index, report) = Index::open_synced(&nb).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(index.pages(&source.id).unwrap(), Some(1));
        assert!(!index.search("Westphalia treaty", 5).unwrap().is_empty());
    }

    #[test]
    fn every_ocr_script_has_valid_pinned_checksums() {
        let mut recognizers = std::collections::HashSet::new();
        for script in OcrScript::ALL {
            let [det, rec, dict] = script.files();
            assert_eq!(det, DETECTOR);
            assert!(
                recognizers.insert(rec.0),
                "duplicate recognizer for {script:?}"
            );
            for (name, sha) in [det, rec, dict] {
                assert!(!name.is_empty());
                assert_eq!(sha.len(), 64, "bad sha length for {name}");
                assert!(
                    sha.bytes().all(|b| b.is_ascii_hexdigit()),
                    "non-hex sha for {name}"
                );
            }
        }
    }
}
