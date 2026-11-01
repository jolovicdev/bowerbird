//! Bowerbird as an app on desktops and phones: the server the web version
//! runs, started inside the app on a loopback port with a fresh token, shown
//! in a native window.

use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};

use bowerbird_core::settings::{self, Settings};
#[cfg(desktop)]
use bowerbird_core::{notebook, pdf};
#[cfg(desktop)]
use tauri::webview::DownloadEvent;
use tauri::webview::NewWindowResponse;
use tauri::{Manager, Url, WebviewUrl, WebviewWindowBuilder};

/// The same port every launch keeps the window's origin, and with it what
/// the page stores locally (the chosen theme). Any free port is used when
/// something else holds this one.
const PORT: u16 = 7478;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let ui = unpack_ui(app, &app.path().app_cache_dir()?.join("ui"))?;
            #[cfg(desktop)]
            let (library, settings_path) = {
                pdf::use_bundled_dir(app.path().resource_dir()?.join("pdfium"));
                (notebook::library_root(), settings::path())
            };
            // A phone app keeps everything in its own data folder; PDFium
            // comes with the app's native libraries, where the system finds it.
            #[cfg(mobile)]
            let (library, settings_path) = {
                let data = app.path().app_data_dir()?;
                settings::use_models_dir(data.join("models"));
                (data.join("Bowerbird"), data.join("settings.toml"))
            };
            let settings = Settings::load_from(&settings_path)
                .map_err(|e| format!("{}: {e}", settings_path.display()))?;
            let token = bowerbird_server::new_token();
            let router = bowerbird_server::router(bowerbird_server::Config {
                library,
                token: token.clone(),
                ui_dir: ui.join(bowerbird_server::UI_SHELL).exists().then_some(ui),
                settings_path,
                settings,
                local_only: true,
            });
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, PORT))
                .or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))?;
            listener.set_nonblocking(true)?;
            let origin = format!("http://{}", listener.local_addr()?);
            let home: Url = format!("{origin}/#token={token}").parse()?;
            tauri::async_runtime::spawn(async move {
                let served = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => axum::serve(listener, router).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = served {
                    eprintln!("Bowerbird's server stopped: {e}");
                }
            });
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(home))
                .on_navigation(move |url| {
                    let elsewhere = matches!(url.scheme(), "http" | "https" | "mailto")
                        && url.origin().ascii_serialization() != origin;
                    if elsewhere {
                        open_in_browser(url);
                    }
                    !elsewhere
                })
                .on_new_window(|url, _| {
                    open_in_browser(&url);
                    NewWindowResponse::Deny
                });
            #[cfg(desktop)]
            let window = {
                let downloads = app.path().download_dir().ok();
                window
                    .title("Bowerbird")
                    .inner_size(1280.0, 860.0)
                    .min_inner_size(360.0, 560.0)
                    // The page takes dropped files itself, as it does in a browser.
                    .disable_drag_drop_handler()
                    .on_download(move |_, event| {
                        if let (DownloadEvent::Requested { destination, .. }, Some(dir)) =
                            (event, &downloads)
                            && let Some(name) = destination.file_name().map(PathBuf::from)
                        {
                            *destination = unused_path(dir, &name);
                        }
                        true
                    })
            };
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Bowerbird couldn't start");
}

/// Writes the UI the app carries into `dir`, replacing what an earlier
/// version left there, for the server to serve.
fn unpack_ui<R: tauri::Runtime>(app: &tauri::App<R>, dir: &Path) -> std::io::Result<PathBuf> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let assets = app.asset_resolver();
    for (key, _) in assets.iter() {
        let Some(asset) = assets.get(key.to_string()) else {
            continue;
        };
        let path = dir.join(key.trim_start_matches('/'));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, asset.bytes())?;
    }
    Ok(dir.to_path_buf())
}

/// `name` in `dir`, numbered like "deck (2).pptx" when that file exists, so
/// a download never replaces a file.
#[cfg(desktop)]
fn unused_path(dir: &Path, name: &Path) -> PathBuf {
    let stem = name
        .file_stem()
        .unwrap_or(name.as_os_str())
        .to_string_lossy();
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// Links to other sites open in the system browser; the window shows only
/// Bowerbird.
fn open_in_browser(url: &Url) {
    #[cfg(target_os = "windows")]
    let openers: &[&[&str]] = &[&["rundll32", "url.dll,FileProtocolHandler"]];
    #[cfg(target_os = "macos")]
    let openers: &[&[&str]] = &[&["open"]];
    // WSL has no xdg-open unless installed; its wslview and explorer.exe
    // open the Windows browser.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let openers: &[&[&str]] = &[&["xdg-open"], &["wslview"], &["explorer.exe"]];
    for opener in openers {
        let spawned = std::process::Command::new(opener[0])
            .args(&opener[1..])
            .arg(url.as_str())
            .spawn();
        if spawned.is_ok() {
            return;
        }
    }
    eprintln!("No program to open {url} with.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_never_replace_a_file() {
        let dir = std::env::temp_dir().join(format!("bowerbird-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = Path::new("The war.pptx");
        assert_eq!(unused_path(&dir, name), dir.join("The war.pptx"));
        std::fs::write(dir.join("The war.pptx"), "").unwrap();
        std::fs::write(dir.join("The war (2).pptx"), "").unwrap();
        assert_eq!(unused_path(&dir, name), dir.join("The war (3).pptx"));
        assert_eq!(unused_path(&dir, Path::new("notes")), dir.join("notes"));
    }
}
