//! Downloads of the models that run on this machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::{Error, Result};

/// Downloads each of `files`, a path under `base`, to the same path under
/// `dir`, skipping those already there. `progress` gets the bytes done and
/// the total of the file being downloaded, as they arrive.
pub async fn files(
    dir: &Path,
    base: &str,
    files: &[&str],
    progress: impl Fn(u64, u64),
) -> Result<()> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    for file in files {
        let path = dir.join(file);
        if path.is_file() {
            continue;
        }
        let failed = |e: reqwest::Error| Error::Http(format!("Couldn't download {file}: {e}"));
        let mut response = http
            .get(format!("{base}/{file}"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(failed)?;
        let total = response.content_length().unwrap_or(0);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Written beside the real name and renamed when complete, so an
        // interrupted download is never mistaken for a finished one.
        let part = PathBuf::from(format!("{}.part", path.display()));
        let mut out = tokio::fs::File::create(&part).await?;
        let mut done = 0;
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            out.write_all(&chunk).await?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        out.flush().await?;
        if total > 0 && done != total {
            return Err(Error::Http(format!(
                "The download of {file} stopped at {done} of {total} bytes. Try again."
            )));
        }
        tokio::fs::rename(&part, &path).await?;
    }
    Ok(())
}
