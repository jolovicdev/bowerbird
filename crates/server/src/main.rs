use std::net::SocketAddr;
use std::path::PathBuf;

use bowerbird_core::notebook;
use bowerbird_core::settings::{self, Settings};
use clap::Parser;

/// Serves Bowerbird's web app and API.
#[derive(Parser)]
#[command(name = "bowerbird-server", version)]
struct Args {
    /// Address to listen on. Anything other than loopback exposes your notebooks to the network, protected only by the token.
    #[arg(long, default_value = "127.0.0.1:7477")]
    addr: SocketAddr,
    /// Built web app to serve.
    #[arg(long, default_value = "ui/build")]
    ui: PathBuf,
    /// Notebook library folder. Defaults to $BOWERBIRD_HOME, else ~/Bowerbird.
    #[arg(long)]
    library: Option<PathBuf>,
}

/// Fixed token instead of a fresh one per start, e.g. for tests.
const TOKEN_ENV: &str = "BOWERBIRD_TOKEN";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let token = std::env::var(TOKEN_ENV).unwrap_or_else(|_| bowerbird_server::new_token());
    let ui_dir = args
        .ui
        .join(bowerbird_server::UI_SHELL)
        .exists()
        .then_some(args.ui.clone());
    if ui_dir.is_none() {
        eprintln!("No web app at {}; serving the API only.", args.ui.display());
    }
    let settings_path = settings::path();
    let settings = Settings::load_from(&settings_path)
        .map_err(|e| std::io::Error::other(format!("{}: {e}", settings_path.display())))?;
    let app = bowerbird_server::router(bowerbird_server::Config {
        library: args.library.unwrap_or_else(notebook::library_root),
        token: token.clone(),
        ui_dir,
        settings_path,
        settings,
    });
    let listener = tokio::net::TcpListener::bind(args.addr).await?;
    if !args.addr.ip().is_loopback() {
        eprintln!(
            "Listening on {} without TLS. Anyone who can reach it and has the token can read your notebooks.",
            args.addr
        );
    }
    println!(
        "Bowerbird is running at http://{}/#token={token}",
        args.addr
    );
    axum::serve(listener, app).await
}
