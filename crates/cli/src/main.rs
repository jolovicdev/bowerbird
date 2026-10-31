use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use bowerbird_core::ask::{self, DEFAULT_MODEL, Event};
use bowerbird_core::embed::{self, Embedder};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::{self, Notebook};
use bowerbird_core::retrieve;
use bowerbird_core::settings::Settings;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "bowerbird",
    version,
    about = "Ask your own sources questions and get answers you can check."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add PDF, Word (.docx), EPUB, Markdown, or text files, or web page URLs. "-" reads pasted text from stdin.
    Add {
        notebook: String,
        #[arg(required = true)]
        items: Vec<String>,
        /// Title for pasted text.
        #[arg(long)]
        title: Option<String>,
    },
    /// Ask a question and get an answer with checked citations.
    Ask {
        notebook: String,
        question: String,
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,
        /// Let the model pick sections from each source's outline first. Costs one extra model call.
        #[arg(long)]
        deep: bool,
    },
    /// Show the keyword search hits for a query.
    Search {
        notebook: String,
        query: String,
        #[arg(long, default_value_t = retrieve::HITS)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Add {
            notebook,
            items,
            title,
        } => add(&notebook, &items, title.as_deref()).await,
        Command::Ask {
            notebook,
            question,
            model,
            deep,
        } => run_ask(&notebook, &question, &model, deep).await,
        Command::Search {
            notebook,
            query,
            limit,
        } => search(&notebook, &query, limit),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

pub type CliResult = Result<(), Box<dyn std::error::Error>>;

fn notebook_dir(name: &str) -> PathBuf {
    notebook::library_root().join(notebook::slug(name))
}

fn open_existing(name: &str) -> Result<Notebook, Box<dyn std::error::Error>> {
    let dir = notebook_dir(name);
    if !dir.join("notebook.toml").exists() {
        return Err(format!(
            "No notebook named \"{name}\" in {}. Add a source to create it.",
            notebook::library_root().display()
        )
        .into());
    }
    Ok(Notebook::open(&dir)?)
}

async fn add(name: &str, items: &[String], title: Option<&str>) -> CliResult {
    let mut nb = Notebook::open_or_create(&notebook_dir(name), name)?;
    for item in items {
        let source = if item == "-" {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            nb.add_text(title.unwrap_or("Pasted text"), &text)?
        } else if item.starts_with("http://") || item.starts_with("https://") {
            nb.add_url(item).await?
        } else {
            nb.add_file(Path::new(item))?
        };
        println!("{}  {}", source.letter, source.title);
    }
    let started = Instant::now();
    let (mut index, report) = Index::open_synced(&nb)?;
    for (id, err) in &report.failed {
        let letter = nb.source(id).map_or("?", |s| s.letter.as_str());
        eprintln!("{letter}  not indexed: {err}");
    }
    let added: usize = report.added.iter().map(|(_, n)| n).sum();
    println!(
        "Indexed {} source(s), {added} chunks, in {:.0} ms. Notebook has {} chunks.",
        report.added.len(),
        started.elapsed().as_secs_f64() * 1000.0,
        index.chunk_count()?
    );
    if index.vector_count()? < index.chunk_count()? {
        embed_new_chunks(&mut index).await?;
    }
    Ok(())
}

/// Embeds the chunks that have no vector, downloading the search model once.
async fn embed_new_chunks(index: &mut Index) -> CliResult {
    let dir = embed::dir();
    if !embed::ready(&dir) {
        eprintln!("Downloading the search model into {}", dir.display());
        embed::download(&dir, |done, total| {
            eprint!("\r{} of {} MB", done / 1_000_000, total / 1_000_000)
        })
        .await?;
        eprintln!();
    }
    let embedder = Embedder::load(&dir)?;
    let started = Instant::now();
    embed::fill(index, &embedder, |done, total| {
        eprint!("\rEmbedded {done} of {total} chunks for search by meaning")
    })?;
    eprintln!(" in {:.0} s.", started.elapsed().as_secs_f64());
    Ok(())
}

/// A model client that reads keys saved in Settings, else the environment.
fn client() -> bowerbird_core::Result<genai::Client> {
    ask::client(Arc::new(RwLock::new(Settings::load()?)))
}

async fn run_ask(name: &str, question: &str, model: &str, deep: bool) -> CliResult {
    let started = Instant::now();
    let nb = open_existing(name)?;
    let (index, _) = Index::open_synced(&nb)?;
    let client = client()?;
    let mut ctx = if deep {
        let ids: Vec<&str> = nb.included().map(|s| s.id.as_str()).collect();
        let outline = retrieve::outline(&index, &nb, &ids, true)?;
        let picks = ask::pick_sections(&client, model, &nb, &outline, question).await?;
        retrieve::gather_with_sections(&index, &nb, question, &picks)?
    } else {
        retrieve::gather(&index, &nb, question)?
    };
    if ctx.passages.is_empty() {
        ctx = retrieve::gather_overview(&index, &nb)?;
    }
    if ctx.passages.is_empty() {
        println!(
            "This notebook has no readable text yet. Add a PDF, a link, or a text file first."
        );
        return Ok(());
    }
    let prompt = ask::prompt(&index, &nb, &ctx, question)?;

    let mut out = std::io::stdout().lock();
    let answer = ask::answer(
        &client,
        model,
        &ask::system_prompt(&nb.chat),
        started,
        question,
        prompt,
        &ctx,
        &[],
        |event| {
            let _ = match event {
                Event::Text(t) => write!(out, "{t}"),
                Event::Citation(c) => {
                    let page = c.page.map(|p| format!(" p{p}")).unwrap_or_default();
                    let mark = if c.verified { "" } else { "?" };
                    write!(out, "[{}{page}{mark}]", c.letter)
                }
            };
            let _ = out.flush();
        },
    )
    .await?;
    writeln!(out, "\n")?;

    for c in &answer.citations {
        let title = c
            .source_id
            .as_deref()
            .and_then(|id| nb.source(id))
            .map_or("unknown source", |s| s.title.as_str());
        let status = if c.verified {
            "verified"
        } else {
            "NOT FOUND in source"
        };
        let page = c.page.map(|p| format!(", page {p}")).unwrap_or_default();
        writeln!(
            out,
            "  [{}] {title}{page}: \"{}\" ({status})",
            c.letter, c.quote
        )?;
    }
    let t = &answer.timings;
    writeln!(
        out,
        "\nrequest sent after {:.0} ms, first token {}, done in {:.1} s ({model})",
        t.request_ms,
        t.first_token_ms
            .map_or("n/a".into(), |ms| format!("{:.1} s", ms / 1000.0)),
        t.total_ms / 1000.0
    )?;
    nb.append_chat(notebook::FIRST_CHAT, &answer)?;
    Ok(())
}

fn search(name: &str, query: &str, limit: usize) -> CliResult {
    let nb = open_existing(name)?;
    let (index, _) = Index::open_synced(&nb)?;
    let started = Instant::now();
    let hits = index.search(query, limit)?;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    for hit in hits {
        let Some(row) = index.chunk(&hit.source_id, hit.ord)? else {
            continue;
        };
        let letter = nb.source(&hit.source_id).map_or("?", |s| s.letter.as_str());
        let page = row.page_start.map(|p| format!(" p{p}")).unwrap_or_default();
        let preview: String = row
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(110)
            .collect();
        println!(
            "{letter}{}{page}  {:.2}  {}\n    {preview}",
            hit.ord, hit.score, row.section
        );
    }
    println!("({elapsed:.1} ms)");
    Ok(())
}
