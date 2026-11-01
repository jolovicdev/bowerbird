//! Downloads of the models that run on this machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{Error, Result};

/// Downloads each of `files`, a path under `base` with the SHA-256 its bytes
/// must have, to the same path under `dir`, skipping those already there. A
/// download with other bytes is discarded. `progress` gets the bytes done and
/// the total of the file being downloaded, as they arrive.
pub async fn files(
    dir: &Path,
    base: &str,
    files: &[(&str, &str)],
    progress: impl Fn(u64, u64),
) -> Result<()> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    for &(file, sha256) in files {
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
        let mut hash = Sha256::new();
        let mut done = 0;
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            out.write_all(&chunk).await?;
            hash.update(&chunk);
            done += chunk.len() as u64;
            progress(done, total);
        }
        out.flush().await?;
        if total > 0 && done != total {
            return Err(Error::Http(format!(
                "The download of {file} stopped at {done} of {total} bytes. Try again."
            )));
        }
        if format!("{:x}", hash.finalize()) != sha256 {
            tokio::fs::remove_file(&part).await?;
            return Err(Error::Http(format!(
                "{file} arrived different from the file Bowerbird expects, so it was discarded. Try again."
            )));
        }
        tokio::fs::rename(&part, &path).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers every request with `body`.
    async fn serve(body: &'static str) -> String {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                let mut request = [0; 2048];
                let _ = conn.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = conn.write_all(response.as_bytes()).await;
            }
        });
        base
    }

    #[tokio::test]
    async fn a_download_is_kept_only_with_the_expected_bytes() {
        let body = "satin bowerbird, regent bowerbird";
        let base = serve(body).await;
        let dir = std::env::temp_dir().join(format!("bowerbird-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let expected = format!("{:x}", Sha256::digest(body));
        files(&dir, &base, &[("birds.txt", &expected)], |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("birds.txt")).unwrap(),
            body
        );
        let other = "0".repeat(64);
        assert!(
            files(&dir, &base, &[("swapped.txt", &other)], |_, _| {})
                .await
                .is_err()
        );
        assert!(!dir.join("swapped.txt").exists());
        assert!(!dir.join("swapped.txt.part").exists());
    }
}
