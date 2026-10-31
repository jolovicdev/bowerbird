pub mod ask;
pub mod chunk;
pub mod cite;
pub mod doc;
pub mod download;
pub mod embed;
pub mod index;
pub mod notebook;
pub mod ocr;
pub mod pdf;
pub mod pptx;
pub mod report;
pub mod retrieve;
pub mod retry;
pub mod search;
pub mod settings;
pub mod study;
pub mod transcript;
pub mod web;
pub mod youtube;
pub mod zipdoc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Pdf(String),
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Model(String),
    #[error("notebook.toml is invalid: {0}")]
    Toml(String),
    #[error("{0}")]
    Invalid(String),
    /// A PDF of page images with no text layer; its text comes from OCR.
    #[error("Bowerbird is reading the scanned pages of this PDF.")]
    Scanned,
}

pub type Result<T> = std::result::Result<T, Error>;
