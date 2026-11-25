use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use bowerbird_core::ask::{self, Event};
use bowerbird_core::embed::{self, Embedder};
use bowerbird_core::index::Index;
use bowerbird_core::notebook::{self, Notebook};
use bowerbird_core::retrieve;
use bowerbird_core::settings::{Settings, Thinking};
use bowerbird_core::youtube;
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
    /// Add files, folders, .zip archives, or web/YouTube URLs. "-" reads pasted text from stdin.
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
        /// Override the model from Settings / notebook.
        #[arg(long)]
        model: Option<String>,
        /// Override thinking effort (default, off, low, medium, high).
        #[arg(long)]
        thinking: Option<String>,
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
            thinking,
            deep,
        } => {
            run_ask(
                &notebook,
                &question,
                model.as_deref(),
                thinking.as_deref(),
                deep,
            )
            .await
        }
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

fn collect_folder_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_folder_files(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Adds `file`, found somewhere under the folder `root`, with the folders
/// from `root` down to it as the folder its title can carry.
fn add_from_folder(
    nb: &mut Notebook,
    root: &Path,
    file: &Path,
) -> bowerbird_core::Result<notebook::Source> {
    let inside = file.strip_prefix(root).unwrap_or(file);
    let folders: Vec<_> = root
        .file_name()
        .into_iter()
        .chain(inside.parent().into_iter().flat_map(Path::iter))
        .map(|part| part.to_string_lossy())
        .collect();
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled");
    nb.add_named_in(&folders.join("/"), name, &std::fs::read(file)?)
}

async fn add(name: &str, items: &[String], title: Option<&str>) -> CliResult {
    let mut nb = Notebook::open_or_create(&notebook_dir(name), name)?;
    for item in items {
        if item == "-" {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            let source = nb.add_text(title.unwrap_or("Pasted text"), &text)?;
            println!("{}  {}", source.letter, source.title);
        } else if let Some(list) = youtube::playlist_id(item) {
            let playlist = youtube::playlist(&list).await?;
            if playlist.cut_short {
                eprintln!(
                    "YouTube's playlist page couldn't be read, so only its first {} videos are added.",
                    playlist.videos.len()
                );
            }
            for video in playlist.videos {
                match nb.add_url(&video).await {
                    Ok(source) => println!("{}  {}", source.letter, source.title),
                    Err(e) => eprintln!("Skipped {video}: {e}"),
                }
            }
        } else if item.starts_with("http://") || item.starts_with("https://") {
            let source = nb.add_url(item).await?;
            println!("{}  {}", source.letter, source.title);
        } else {
            let path = Path::new(item);
            if path.is_dir() {
                let mut files = Vec::new();
                collect_folder_files(path, &mut files)?;
                for file in files {
                    match add_from_folder(&mut nb, path, &file) {
                        Ok(source) => println!("{}  {}", source.letter, source.title),
                        Err(e) => eprintln!("Skipped {}: {e}", file.display()),
                    }
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
            {
                let bytes = std::fs::read(path)?;
                let report = nb.add_zip(&bytes)?;
                for source in report.added {
                    println!("{}  {}", source.letter, source.title);
                }
                if !report.skipped.is_empty() {
                    eprintln!(
                        "Skipped {} unsupported file(s) in {}.",
                        report.skipped.len(),
                        path.display()
                    );
                }
            } else {
                let source = nb.add_file(path)?;
                println!("{}  {}", source.letter, source.title);
            }
        }
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

fn parse_thinking(raw: &str) -> Result<Thinking, Box<dyn std::error::Error>> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" => Ok(Thinking::Default),
        "off" | "none" | "0" => Ok(Thinking::Off),
        "low" => Ok(Thinking::Low),
        "medium" => Ok(Thinking::Medium),
        "high" => Ok(Thinking::High),
        other => Err(format!(
            "Unknown --thinking value \"{other}\". Use default, off, low, medium, or high."
        )
        .into()),
    }
}

async fn run_ask(
    name: &str,
    question: &str,
    model_arg: Option<&str>,
    thinking_arg: Option<&str>,
    deep: bool,
) -> CliResult {
    let started = Instant::now();
    let nb = open_existing(name)?;
    let (index, _) = Index::open_synced(&nb)?;
    let settings = Settings::load()?;
    let (model, thinking) = settings.for_notebook(&nb.chat);
    let model = model_arg.map_or(model, str::to_string);
    let thinking = thinking_arg
        .map(parse_thinking)
        .transpose()?
        .unwrap_or(thinking);
    let shared = Arc::new(RwLock::new(settings));
    let client = ask::client_with_thinking(shared.clone(), thinking)?;
    let (helper_client, helper_model) = ask::helper(&shared, &client, &model)?;
    let mut ctx = if deep {
        let ids: Vec<&str> = nb.included().map(|s| s.id.as_str()).collect();
        let outline = retrieve::outline(&index, &nb, &ids, true)?;
        let picks =
            ask::pick_sections(&helper_client, &helper_model, &nb, &outline, question).await?;
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
        &model,
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
                Event::Thinking(_) => Ok(()),
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
