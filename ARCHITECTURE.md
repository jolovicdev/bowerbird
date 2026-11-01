# Architecture

Bowerbird is a Rust workspace with a SvelteKit front end. One library holds
everything a notebook does; a local HTTP server exposes it; the web app, the
desktop app and the Android app all talk to that server.

```text
crates/core      the library: notebooks, extraction, index, retrieval, answers, study material
crates/server    HTTP API (axum) over the core, and the built UI from the same origin
crates/cli       the `bowerbird` command: add, ask, search
crates/speech    speech on this machine: Kokoro read-aloud, Whisper transcription
crates/desktop   Tauri 2 shell for Linux, Windows and Android, running the server in-process
ui/              SvelteKit single-page app (no server rendering), built to ui/build
```

## Notebooks are folders

A notebook is a folder of plain files (layout in the README). `notebook.toml`
lists the sources with their letters; the originals sit in `sources/`, notes
are Markdown, conversations are JSON lines. `.bowerbird/index.sqlite` is a
cache built from those files and rebuilt when it is missing or from another
version (`index::VERSION`).

A folder may come from someone else, so `Notebook::open` refuses a
`notebook.toml` whose source ids or file names would reach outside the
folder, and ids of notes, chats and study sets are checked the same way
before they become paths.

Source ids are the first 12 hex digits of the stored file's SHA-256, so adding
the same file twice adds it once. Letters are never reused, so a citation in a
saved answer keeps naming the same source after another is removed.

## Adding a source

1. The file, link or text is checked (`notebook::check_file`) and stored in
   `sources/`. Links are fetched once by `web::fetch`; a YouTube link becomes
   its caption track (`youtube`), a link to a PDF becomes the PDF.
2. `Notebook::extract` turns it into a `Doc`: text with page boundaries and a
   heading tree. PDFs go through PDFium (`pdf`), Word and EPUB through their
   zip packages (`zipdoc`), web pages through a readability pass (`web`),
   recordings through their transcript (`transcript`).
3. `chunk` splits the text into passages that never cross a section boundary,
   and `index` stores them in SQLite with an FTS5 index. The source is
   searchable as soon as this finishes.
4. Background jobs in the server fill in what takes longer: vectors for search
   by meaning (`server::embedding`, `embed`), OCR for scanned PDFs
   (`server::scans`, `ocr`), transcripts for recordings (`server::speech`,
   `speech::listen`). OCR text and transcripts are written beside the source
   in `sources/`, so they survive an index rebuild; vectors live in the index
   and are filled in again after one.

## Answering a question

1. `retrieve::rank` searches by keyword (FTS5, BM25) and, once vectors exist,
   by meaning (EmbeddingGemma vectors, scored against every stored vector),
   and fuses the two lists by weighted reciprocal rank. The best hit of each
   source among the candidates is kept even outside the top 8.
2. `retrieve::gather` picks passages within a character budget: the hits,
   then their neighbors in the same section. When no keyword matches, an
   overview of every source fills the room the nearest passages by meaning
   leave, which serves both "Summarize the key points" and questions worded
   unlike the sources. Deep search lets the model pick whole sections from
   the outline first (`ask::pick_sections`).
3. `ask::prompt` labels each passage with its source letter and position
   (`B12`) and asks the model, through `genai`, to cite as
   `[[B12|exact words]]`. Any provider genai supports works; nothing in the
   prompt or the checks depends on one.
4. The answer streams to the browser over server-sent events. `cite::Parser`
   pulls citation markers out of the stream, and `ask::resolve` checks each
   quote with `cite::verify` against the cited passage, then the rest of the
   same source. The check compares letters and digits only, since PDF text
   layers split words unpredictably, and limits how much an ellipsis may
   skip.

Flashcards, quizzes, mind maps, tables, slides, infographics and audio
overview scripts (`study`) are written the same way: passages in, JSON out,
with each quote checked before the set is saved. Reports (`report`) become
notes, whose citations are checked like an answer's. Slide decks are written
as Office Open XML by hand (`pptx`).

## The server

Every `/api` request needs the bearer token printed at startup. Request bodies
are capped at 200 MB. The server listens on `127.0.0.1` unless `--addr` says
otherwise; when it listens beyond this machine, links added through the API
may only reach the public internet (`web::Reach::Public`), so a token holder
can't make the server read its own machine or local network. Addresses a web
page chooses, such as its pictures, are held to the public internet always.

Work that changes a notebook takes that notebook's lock, so two requests never
rewrite `notebook.toml` or index the same source at once. Speech and OCR
each use every core, so they take turns; vectors are filled for one notebook
at a time.

## Desktop and Android

`crates/desktop` builds the same server router into the app, binds it to
`127.0.0.1:7478` (or a free port), and opens a webview at that address with
the token in the URL fragment. Links to other sites open in the system
browser. The built UI is embedded in the binary and unpacked to the app's
cache folder at launch. On Android the same crate builds as a library with a
mobile entry point.

## Models on this machine

EmbeddingGemma (search), PP-OCRv5 (OCR), Kokoro (speech) and Whisper
(transcription) download on first use into the models folder named in the
README, through `download`, and run on ONNX Runtime or whisper.cpp. Their
licenses are listed in THIRD_PARTY_NOTICES.md.
