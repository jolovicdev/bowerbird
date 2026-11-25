//! Web pages: fetch the HTML once, keep it as the source file, and extract the
//! readable article as Markdown, without link targets and images.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use crate::{Error, Result, youtube};

mod extract;
mod figures;

pub use extract::extract;
pub use figures::{fetch_images, figure_sources};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Largest page or linked file read: the size of the largest upload.
const PAGE_BYTES_MAX: usize = 200 * 1024 * 1024;

/// Where a fetch may connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Anywhere, this machine and its local network included.
    Any,
    /// The public internet only. For addresses a page chooses, and for links
    /// added through a server that doesn't allow private links, so neither
    /// reaches what only the server can, such as a cloud metadata service.
    Public,
}

/// A fetched page, kept as bytes since a link may point at a PDF.
pub struct Page {
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Set instead of the page for a YouTube video: its captions.
    pub transcript: Option<youtube::Transcript>,
}

impl Page {
    pub fn is_pdf(&self) -> bool {
        self.content_type.starts_with("application/pdf") || self.bytes.starts_with(b"%PDF-")
    }

    pub fn html(&self) -> Result<String> {
        let ct = &self.content_type;
        if ct.is_empty() || ct.starts_with("text/") || ct.contains("html") || ct.contains("xml") {
            Ok(decode(&self.bytes, ct))
        } else {
            Err(Error::Invalid(format!(
                "The link points at a {ct} file. Bowerbird reads web pages and PDFs."
            )))
        }
    }
}

/// How far into a page a `<meta>` tag can declare its charset, as for browsers.
const META_CHARSET_BYTES: usize = 1024;

/// A page's text in the charset its Content-Type declares, else its `<meta>`
/// tag, else UTF-8. A byte order mark wins over them all.
fn decode(bytes: &[u8], content_type: &str) -> String {
    let head =
        String::from_utf8_lossy(&bytes[..bytes.len().min(META_CHARSET_BYTES)]).to_ascii_lowercase();
    let encoding = charset_in(content_type)
        .or_else(|| {
            head.split("<meta")
                .skip(1)
                .find_map(|tag| charset_in(tag.split('>').next()?))
        })
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// The value after `charset=`, without its quotes.
fn charset_in(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("charset=")?;
    rest.trim_start_matches(['"', '\''])
        .split(['"', '\'', ';', ' ', '/'])
        .next()
        .filter(|label| !label.is_empty())
}

/// Whether `url` is an http or https address.
pub fn is_web_address(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// Last path segment of a URL, as a fallback title for a linked file.
pub fn file_name(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()?
                .rev()
                .find(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| url.to_string())
}

pub async fn fetch(url: &str, reach: Reach) -> Result<Page> {
    fetch_within(url, reach, TIMEOUT).await
}

/// `patience` is how long connecting, or waiting for more bytes, may take. The
/// whole download has no limit: a large file over a slow line takes its time.
async fn fetch_within(url: &str, reach: Reach, patience: Duration) -> Result<Page> {
    if let Some(id) = youtube::video_id(url) {
        return Ok(Page {
            content_type: String::new(),
            bytes: Vec::new(),
            transcript: Some(youtube::transcript(&id).await?),
        });
    }
    if !host_allowed(url, reach) {
        return Err(Error::Http(private_message(&host(url))));
    }
    let client = client(reach)
        .connect_timeout(patience)
        .read_timeout(patience)
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    let response = crate::retry::with_retries(
        || async {
            client
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
        },
        |e: &reqwest::Error| crate::retry::transient_status(e.status().map(|s| s.as_u16())),
    )
    .await
    .map_err(|e| fetch_error(url, &e))?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let bytes = body(response, PAGE_BYTES_MAX)
        .await
        .map_err(|e| fetch_error(url, &e))?
        .ok_or_else(|| {
            Error::Invalid(format!(
                "The file at {} is larger than {} MB, the most Bowerbird reads from a link.",
                host(url),
                PAGE_BYTES_MAX >> 20
            ))
        })?;
    Ok(Page {
        content_type,
        bytes,
        transcript: None,
    })
}

/// Reads a response body, or `None` once it passes `max` bytes, never
/// holding more than that.
async fn body(mut response: reqwest::Response, max: usize) -> reqwest::Result<Option<Vec<u8>>> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > max {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

/// A client that connects only where `reach` allows. Host names are checked
/// as they resolve, which covers redirects and DNS answers that change;
/// redirects to an address written out as numbers are checked as they come.
fn client(reach: Reach) -> reqwest::ClientBuilder {
    let builder =
        crate::http::client().user_agent(concat!("Bowerbird/", env!("CARGO_PKG_VERSION")));
    match reach {
        Reach::Any => builder,
        Reach::Public => {
            builder
                .dns_resolver(PublicOnly)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 10 {
                        attempt.error("too many redirects")
                    } else if host_allowed(attempt.url().as_str(), Reach::Public) {
                        attempt.follow()
                    } else {
                        attempt.error(PrivateAddress)
                    }
                }))
        }
    }
}

/// False for a URL whose host is an address off the public internet, written
/// out as numbers, when only the public internet may be reached. Host names
/// are checked by `PublicOnly` instead.
fn host_allowed(url: &str, reach: Reach) -> bool {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string));
    match host.map(|h| h.trim_matches(['[', ']']).parse::<IpAddr>()) {
        Some(Ok(ip)) => reach == Reach::Any || is_public(ip),
        _ => true,
    }
}

/// Whether an address is on the public internet: not this machine, a local
/// network, or a range reserved for something else.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
                || a == 0
                || a >= 240
                // Carrier-grade NAT, and benchmarking.
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public(v4.into());
            }
            // NAT64 carries an IPv4 address in its last 32 bits.
            if let [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] = ip.segments() {
                return is_public(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)).into());
            }
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local())
        }
    }
}

/// Resolves host names to their public addresses only.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(PrivateAddress.into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A connection refused because it would leave the public internet.
#[derive(Debug)]
struct PrivateAddress;

impl std::fmt::Display for PrivateAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the address is on this machine or a private network")
    }
}

impl std::error::Error for PrivateAddress {}

fn private_message(host: &str) -> String {
    format!(
        "{host} is on this machine or a private network, which links can't reach unless the server runs with --allow-private-links."
    )
}

fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// Says what went wrong in words a reader can act on, naming the site.
fn fetch_error(url: &str, error: &reqwest::Error) -> Error {
    let host = host(url);
    let private = std::iter::successors(std::error::Error::source(error), |e| e.source())
        .any(|e| e.is::<PrivateAddress>());
    let message = if private {
        private_message(&host)
    } else if error.is_builder() {
        format!("“{url}” isn't a web address Bowerbird can open.")
    } else if error.is_timeout() {
        format!(
            "{host} took longer than {} seconds to answer.",
            TIMEOUT.as_secs()
        )
    } else if let Some(status) = error.status() {
        match status.as_u16() {
            401 | 403 => format!("{host} refused access to this page ({status})."),
            404 | 410 => format!("{host} has no page at this address ({status})."),
            code => format!("{host} answered with an error ({code})."),
        }
    } else if error.is_connect() {
        format!("Couldn't connect to {host}. Check the address and your internet connection.")
    } else {
        format!("Couldn't read the page from {host}: {error}")
    };
    Error::Http(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// A server on a free loopback port answering every request with `body`,
    /// without a length, so the body ends when the connection closes.
    async fn serve(content_type: &'static str, body: Vec<u8>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0; 4096];
                    let _ = stream.read(&mut request).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        port
    }

    #[test]
    fn only_internet_addresses_count_as_public() {
        for ip in [
            "127.0.0.1",
            "10.0.0.8",
            "172.20.1.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "64:ff9b::808:808",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn links_off_the_internet_are_fetched_only_when_any_address_may_be() {
        let port = serve("text/html", b"<html><title>Local</title></html>".to_vec()).await;
        for url in [
            format!("http://127.0.0.1:{port}/"),
            format!("http://localhost:{port}/"),
        ] {
            let refused = fetch(&url, Reach::Public)
                .await
                .err()
                .map(|e| e.to_string());
            assert!(
                refused.is_some_and(|e| e.contains("private network")),
                "{url}"
            );
            assert!(fetch(&url, Reach::Any).await.is_ok(), "{url}");
        }
    }

    #[tokio::test]
    async fn pictures_off_the_internet_come_only_from_a_local_page_s_own_host() {
        let port = serve("image/png", b"not really a png".to_vec()).await;
        let src = vec![format!("http://127.0.0.1:{port}/plate.png")];
        let page = format!("http://127.0.0.1:{port}/bowers.html");
        assert_eq!(fetch_images(&src, &page, Reach::Any).await.len(), 1);
        assert!(fetch_images(&src, &page, Reach::Public).await.is_empty());
        let elsewhere = format!("http://localhost:{port}/bowers.html");
        assert!(fetch_images(&src, &elsewhere, Reach::Any).await.is_empty());
    }

    #[test]
    fn pages_are_read_in_the_charset_they_declare() {
        let html = |content_type: &str, bytes: &[u8]| {
            Page {
                content_type: content_type.to_string(),
                bytes: bytes.to_vec(),
                transcript: None,
            }
            .html()
            .unwrap()
        };
        let mueller = b"<p>Der M\xfcller</p>";
        assert!(html("text/html; charset=windows-1252", mueller).contains("Müller"));
        assert!(html("text/html;charset=\"iso-8859-1\"", mueller).contains("Müller"));
        assert!(html("text/html; charset=windows-1251", b"<p>\xcc\xe8\xf0</p>").contains("Мир"));
        assert!(
            html(
                "text/html",
                b"<head><meta charset=\"windows-1250\"></head><p>\x9aljiva</p>"
            )
            .contains("šljiva")
        );
        assert!(
            html(
                "text/html",
                b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=ISO-8859-2\"><p>\xb9ljiva</p>"
            )
            .contains("šljiva")
        );
        let utf8 = "<p>Der Müller</p>".as_bytes();
        assert!(html("text/html", utf8).contains("Müller"));
        assert!(html("text/html; charset=nonsense", utf8).contains("Müller"));
    }

    /// A server on a free loopback port sending `pieces` of a page, each `gap`
    /// after the last. It then closes the connection, or if `stall`, stays silent.
    async fn serve_slowly(gap: Duration, pieces: usize, stall: bool) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = [0; 4096];
                    let _ = stream.read(&mut request).await;
                    let head =
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;
                    for _ in 0..pieces {
                        tokio::time::sleep(gap).await;
                        let _ = stream.write_all(b"<p>more</p>").await;
                    }
                    if stall {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn a_download_lasts_while_bytes_keep_coming_and_stops_when_they_stop() {
        let patience = Duration::from_millis(1500);
        let steady = serve_slowly(Duration::from_millis(400), 6, false).await;
        let url = format!("http://127.0.0.1:{steady}/");
        let page = fetch_within(&url, Reach::Any, patience).await.unwrap();
        assert_eq!(page.bytes.len(), 6 * "<p>more</p>".len());

        let stalled = serve_slowly(Duration::ZERO, 1, true).await;
        let url = format!("http://127.0.0.1:{stalled}/");
        let error = fetch_within(&url, Reach::Any, patience)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("took longer"), "{error}");
    }

    /// A server on a free loopback port redirecting every request to `to`.
    async fn redirect_to(to: String) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await;
                let head = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(head.as_bytes()).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn a_picture_can_t_redirect_off_the_page_s_own_host_to_a_private_address() {
        let elsewhere = serve("image/png", b"not really a png".to_vec()).await;
        // `localhost` is another host than the page's `127.0.0.1`.
        let away = redirect_to(format!("http://localhost:{elsewhere}/plate.png")).await;
        let src = vec![format!("http://127.0.0.1:{away}/plate.png")];
        let page = format!("http://127.0.0.1:{away}/bowers.html");
        assert!(fetch_images(&src, &page, Reach::Any).await.is_empty());

        let nearby = redirect_to(format!("http://127.0.0.1:{elsewhere}/plate.png")).await;
        let src = vec![format!("http://127.0.0.1:{nearby}/plate.png")];
        let page = format!("http://127.0.0.1:{nearby}/bowers.html");
        let saved = fetch_images(&src, &page, Reach::Any).await;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].0, src[0]);
    }

    #[tokio::test]
    async fn bodies_stop_at_their_limit() {
        let port = serve("text/html", vec![b'x'; 5000]).await;
        let url = format!("http://127.0.0.1:{port}/");
        let over = body(reqwest::get(&url).await.unwrap(), 4096).await.unwrap();
        assert!(over.is_none());
        let within = body(reqwest::get(&url).await.unwrap(), 8192).await.unwrap();
        assert_eq!(within.map(|b| b.len()), Some(5000));
    }
}
