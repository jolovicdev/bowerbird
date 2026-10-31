//! Reads scanned PDF pages with PP-OCRv5: a text detector and a recognizer
//! for Latin script, accented letters included. Cyrillic and other scripts
//! need a recognizer of their own and aren't read yet. The models are
//! downloaded once, on first use.

use std::path::{Path, PathBuf};

use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::utils::create_rgb_image;

use crate::{Error, Result, download, pdf, settings};

/// A fixed release, so the files can't change under a pinned model.
const RELEASE: &str = "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0";
const DETECTOR: &str = "pp-ocrv5_mobile_det.onnx";
const RECOGNIZER: &str = "latin_pp-ocrv5_mobile_rec.onnx";
const DICTIONARY: &str = "ppocrv5_latin_dict.txt";
const FILES: [&str; 3] = [DETECTOR, RECOGNIZER, DICTIONARY];

pub fn dir() -> PathBuf {
    settings::models_dir().join("ocr")
}

/// Whether the models are already in `dir`.
pub fn ready(dir: &Path) -> bool {
    FILES.iter().all(|file| dir.join(file).is_file())
}

/// Downloads the models into `dir`, reporting as `download::files`.
pub async fn download(dir: &Path, progress: impl Fn(u64, u64)) -> Result<()> {
    download::files(dir, RELEASE, &FILES, progress).await
}

pub struct Reader(OAROCR);

impl Reader {
    pub fn load(dir: &Path) -> Result<Reader> {
        OAROCRBuilder::new(
            dir.join(DETECTOR),
            dir.join(RECOGNIZER),
            dir.join(DICTIONARY),
        )
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

        download(&dir(), |_, _| {}).await.unwrap();
        let reader = Reader::load(&dir()).unwrap();
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
}
